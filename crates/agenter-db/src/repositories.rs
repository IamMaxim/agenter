use agenter_core::{
    AgentProviderId, AgentTurnSettings, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
    ApprovalRequest, ApprovalStatus as UniversalApprovalStatus, CommandId, ItemId, RunnerId,
    SessionId, SessionSnapshot, SessionStatus, SessionUsageSnapshot, TurnId,
    UniversalEventEnvelope, UniversalEventKind, UniversalEventSource, UniversalSeq, UserId,
    WorkspaceId,
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgRow, PgPool, Postgres, Result, Row, Transaction};
use uuid::Uuid;

use crate::models::{
    AgentEvent, AgentSession, AgentSessionWithWorkspace, BrowserAuthSession, CachedEvent,
    CommandIdempotencyRecord, CommandIdempotencyStatus, ConnectorAccount, ConnectorLinkCode,
    OidcLoginState, OidcProvider, PendingApproval, Runner, StoredSessionSnapshot,
    UniversalAppendOutcome, User, Workspace,
};

#[derive(Clone, Debug)]
pub struct UpsertOidcProvider<'a> {
    pub provider_id: &'a str,
    pub display_name: &'a str,
    pub issuer_url: &'a str,
    pub client_id: &'a str,
    pub client_secret_ciphertext: Option<&'a str>,
    pub scopes: &'a [String],
    pub enabled: bool,
}

pub async fn create_user(pool: &PgPool, email: &str, display_name: Option<&str>) -> Result<User> {
    let row = sqlx::query(
        "insert into users (email, display_name)
         values ($1, $2)
         returning user_id, email, display_name, created_at, updated_at",
    )
    .bind(email)
    .bind(display_name)
    .fetch_one(pool)
    .await?;

    user_from_row(&row)
}

pub async fn create_user_with_password_credential(
    pool: &PgPool,
    email: &str,
    display_name: Option<&str>,
    password_hash: &str,
) -> Result<User> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "insert into users (email, display_name)
         values ($1, $2)
         returning user_id, email, display_name, created_at, updated_at",
    )
    .bind(email)
    .bind(display_name)
    .fetch_one(&mut *tx)
    .await?;
    let user = user_from_row(&row)?;

    sqlx::query(
        "insert into auth_identities (user_id, provider_kind, provider_id, subject)
         values ($1, 'password', 'local', $2)",
    )
    .bind(user.user_id.as_uuid())
    .bind(email)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "insert into password_credentials (user_id, password_hash)
         values ($1, $2)",
    )
    .bind(user.user_id.as_uuid())
    .bind(password_hash)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(user)
}

pub async fn bootstrap_password_admin(
    pool: &PgPool,
    email: &str,
    display_name: Option<&str>,
    password_hash: &str,
) -> Result<User> {
    let mut tx = pool.begin().await?;
    let existing: Option<PgRow> = sqlx::query(
        "select u.user_id, u.email, u.display_name, u.created_at, u.updated_at
         from users u
         join auth_identities ai on ai.user_id = u.user_id
         where ai.provider_kind = 'password'
           and ai.provider_id = 'local'
           and ai.subject = $1",
    )
    .bind(email)
    .fetch_optional(&mut *tx)
    .await?;

    let user = if let Some(row) = existing {
        let user = user_from_row(&row)?;
        sqlx::query(
            "update password_credentials
             set password_hash = $2,
                 password_updated_at = now(),
                 updated_at = now()
             where user_id = $1",
        )
        .bind(user.user_id.as_uuid())
        .bind(password_hash)
        .execute(&mut *tx)
        .await?;
        user
    } else {
        let row = sqlx::query(
            "insert into users (email, display_name)
             values ($1, $2)
             returning user_id, email, display_name, created_at, updated_at",
        )
        .bind(email)
        .bind(display_name)
        .fetch_one(&mut *tx)
        .await?;
        let user = user_from_row(&row)?;
        sqlx::query(
            "insert into auth_identities (user_id, provider_kind, provider_id, subject)
             values ($1, 'password', 'local', $2)",
        )
        .bind(user.user_id.as_uuid())
        .bind(email)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "insert into password_credentials (user_id, password_hash)
             values ($1, $2)",
        )
        .bind(user.user_id.as_uuid())
        .bind(password_hash)
        .execute(&mut *tx)
        .await?;
        user
    };

    tx.commit().await?;
    Ok(user)
}

pub async fn find_password_credential_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<(User, String)>> {
    let row = sqlx::query(
        "select u.user_id, u.email, u.display_name, u.created_at, u.updated_at,
                pc.password_hash
         from users u
         join password_credentials pc on pc.user_id = u.user_id
         where u.email = $1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;

    row.as_ref()
        .map(|row| Ok((user_from_row(row)?, row.try_get("password_hash")?)))
        .transpose()
}

pub async fn upsert_oidc_provider(
    pool: &PgPool,
    provider: UpsertOidcProvider<'_>,
) -> Result<OidcProvider> {
    let row = sqlx::query(
        "insert into oidc_providers (
            oidc_provider_id,
            display_name,
            issuer_url,
            client_id,
            client_secret_ciphertext,
            scopes,
            enabled
         )
         values ($1, $2, $3, $4, $5, $6, $7)
         on conflict (oidc_provider_id)
         do update set display_name = excluded.display_name,
                       issuer_url = excluded.issuer_url,
                       client_id = excluded.client_id,
                       client_secret_ciphertext = excluded.client_secret_ciphertext,
                       scopes = excluded.scopes,
                       enabled = excluded.enabled,
                       updated_at = now()
         returning oidc_provider_id, display_name, issuer_url, client_id,
             client_secret_ciphertext, scopes, enabled, created_at, updated_at",
    )
    .bind(provider.provider_id)
    .bind(provider.display_name)
    .bind(provider.issuer_url)
    .bind(provider.client_id)
    .bind(provider.client_secret_ciphertext)
    .bind(provider.scopes)
    .bind(provider.enabled)
    .fetch_one(pool)
    .await?;

    oidc_provider_from_row(&row)
}

pub async fn find_oidc_provider(pool: &PgPool, provider_id: &str) -> Result<Option<OidcProvider>> {
    let row = sqlx::query(
        "select oidc_provider_id, display_name, issuer_url, client_id,
                client_secret_ciphertext, scopes, enabled, created_at, updated_at
         from oidc_providers
         where oidc_provider_id = $1 and enabled = true",
    )
    .bind(provider_id)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(oidc_provider_from_row).transpose()
}

pub async fn create_oidc_login_state(
    pool: &PgPool,
    state: &str,
    provider_id: &str,
    nonce: &str,
    pkce_verifier: Option<&str>,
    return_to: Option<&str>,
    expires_at: DateTime<Utc>,
) -> Result<OidcLoginState> {
    let row = sqlx::query(
        "insert into oidc_login_states (
            state,
            provider_id,
            nonce,
            pkce_verifier,
            return_to,
            expires_at
         )
         values ($1, $2, $3, $4, $5, $6)
         returning state, provider_id, nonce, pkce_verifier, return_to,
             expires_at, consumed_at, created_at",
    )
    .bind(state)
    .bind(provider_id)
    .bind(nonce)
    .bind(pkce_verifier)
    .bind(return_to)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;

    oidc_login_state_from_row(&row)
}

pub async fn consume_oidc_login_state(
    pool: &PgPool,
    provider_id: &str,
    state: &str,
    now: DateTime<Utc>,
) -> Result<Option<OidcLoginState>> {
    let row = sqlx::query(
        "update oidc_login_states
         set consumed_at = $2
         where state = $1
           and provider_id = $3
           and consumed_at is null
           and expires_at > $2
         returning state, provider_id, nonce, pkce_verifier, return_to,
             expires_at, consumed_at, created_at",
    )
    .bind(state)
    .bind(now)
    .bind(provider_id)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(oidc_login_state_from_row).transpose()
}

pub async fn upsert_oidc_identity(
    pool: &PgPool,
    provider_id: &str,
    subject: &str,
    email: &str,
    display_name: Option<&str>,
) -> Result<User> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query(
        "insert into users (email, display_name)
         values ($1, $2)
         on conflict (email)
         do update set display_name = coalesce(excluded.display_name, users.display_name),
                       updated_at = now()
         returning user_id, email, display_name, created_at, updated_at",
    )
    .bind(email)
    .bind(display_name)
    .fetch_one(&mut *tx)
    .await?;
    let candidate_user = user_from_row(&row)?;

    sqlx::query(
        "insert into auth_identities (user_id, provider_kind, provider_id, subject)
         values ($1, 'oidc', $2, $3)
         on conflict (provider_kind, provider_id, subject) do nothing",
    )
    .bind(candidate_user.user_id.as_uuid())
    .bind(provider_id)
    .bind(subject)
    .execute(&mut *tx)
    .await?;

    let row = sqlx::query(
        "select u.user_id, u.email, u.display_name, u.created_at, u.updated_at
         from users u
         join auth_identities ai on ai.user_id = u.user_id
         where ai.provider_kind = 'oidc'
           and ai.provider_id = $1
           and ai.subject = $2",
    )
    .bind(provider_id)
    .bind(subject)
    .fetch_one(&mut *tx)
    .await?;
    let user = user_from_row(&row)?;

    tx.commit().await?;
    Ok(user)
}

pub async fn update_password_credential(
    pool: &PgPool,
    user_id: UserId,
    email: &str,
    password_hash: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "insert into auth_identities (user_id, provider_kind, provider_id, subject)
         values ($1, 'password', 'local', $2)
         on conflict (provider_kind, provider_id, subject)
         do update set user_id = excluded.user_id, updated_at = now()",
    )
    .bind(user_id.as_uuid())
    .bind(email)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "insert into password_credentials (user_id, password_hash)
         values ($1, $2)
         on conflict (user_id)
         do update set password_hash = excluded.password_hash,
                       password_updated_at = now(),
                       updated_at = now()",
    )
    .bind(user_id.as_uuid())
    .bind(password_hash)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn create_browser_auth_session(
    pool: &PgPool,
    session_token_hash: &str,
    user_id: UserId,
    expires_at: DateTime<Utc>,
) -> Result<BrowserAuthSession> {
    let row = sqlx::query(
        "insert into browser_auth_sessions (session_token_hash, user_id, expires_at)
         values ($1, $2, $3)
         returning session_token_hash, user_id, expires_at, revoked_at, created_at, last_seen_at",
    )
    .bind(session_token_hash)
    .bind(user_id.as_uuid())
    .bind(expires_at)
    .fetch_one(pool)
    .await?;

    browser_auth_session_from_row(&row)
}

pub async fn find_browser_auth_session_user(
    pool: &PgPool,
    session_token_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<User>> {
    let row = sqlx::query(
        "with active_session as (
            update browser_auth_sessions
            set last_seen_at = $2
            where session_token_hash = $1
              and revoked_at is null
              and expires_at > $2
            returning user_id
         )
         select u.user_id, u.email, u.display_name, u.created_at, u.updated_at
         from users u
         join active_session s on s.user_id = u.user_id",
    )
    .bind(session_token_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(user_from_row).transpose()
}

pub async fn revoke_browser_auth_session(pool: &PgPool, session_token_hash: &str) -> Result<bool> {
    let result = sqlx::query(
        "update browser_auth_sessions
         set revoked_at = now()
         where session_token_hash = $1
           and revoked_at is null",
    )
    .bind(session_token_hash)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

pub async fn register_runner(pool: &PgPool, name: &str, version: Option<&str>) -> Result<Runner> {
    let row = sqlx::query(
        "insert into runners (name, version, last_seen_at)
         values ($1, $2, now())
         returning runner_id, name, version, last_seen_at, created_at, updated_at",
    )
    .bind(name)
    .bind(version)
    .fetch_one(pool)
    .await?;

    runner_from_row(&row)
}

pub async fn upsert_runner_with_id(
    pool: &PgPool,
    runner_id: RunnerId,
    name: &str,
    version: Option<&str>,
) -> Result<Runner> {
    let row = sqlx::query(
        "insert into runners (runner_id, name, version, last_seen_at)
         values ($1, $2, $3, now())
         on conflict (runner_id)
         do update set name = excluded.name,
                       version = excluded.version,
                       last_seen_at = now(),
                       updated_at = now()
         returning runner_id, name, version, last_seen_at, created_at, updated_at",
    )
    .bind(runner_id.as_uuid())
    .bind(name)
    .bind(version)
    .fetch_one(pool)
    .await?;

    runner_from_row(&row)
}

pub async fn upsert_workspace(
    pool: &PgPool,
    runner_id: RunnerId,
    path: &str,
    display_name: Option<&str>,
) -> Result<Workspace> {
    let row = sqlx::query(
        "insert into workspaces (runner_id, path, display_name)
         values ($1, $2, $3)
         on conflict (runner_id, path)
         do update set display_name = excluded.display_name, updated_at = now()
         returning workspace_id, runner_id, path, display_name, created_at, updated_at",
    )
    .bind(runner_id.as_uuid())
    .bind(path)
    .bind(display_name)
    .fetch_one(pool)
    .await?;

    workspace_from_row(&row)
}

pub async fn upsert_workspace_with_id(
    pool: &PgPool,
    workspace_id: WorkspaceId,
    runner_id: RunnerId,
    path: &str,
    display_name: Option<&str>,
) -> Result<Workspace> {
    let row = sqlx::query(
        "insert into workspaces (workspace_id, runner_id, path, display_name)
         values ($1, $2, $3, $4)
         on conflict (workspace_id)
         do update set runner_id = excluded.runner_id,
                       path = excluded.path,
                       display_name = excluded.display_name,
                       updated_at = now()
         returning workspace_id, runner_id, path, display_name, created_at, updated_at",
    )
    .bind(workspace_id.as_uuid())
    .bind(runner_id.as_uuid())
    .bind(path)
    .bind(display_name)
    .fetch_one(pool)
    .await?;

    workspace_from_row(&row)
}

pub async fn create_session(
    pool: &PgPool,
    owner_user_id: UserId,
    runner_id: RunnerId,
    workspace_id: WorkspaceId,
    provider_id: AgentProviderId,
    external_session_id: Option<&str>,
    title: Option<&str>,
) -> Result<AgentSession> {
    let row = sqlx::query(
        "insert into agent_sessions (
            owner_user_id,
            runner_id,
            workspace_id,
            provider_id,
            external_session_id,
            status,
            title
         )
         values ($1, $2, $3, $4, $5, $6, $7)
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(owner_user_id.as_uuid())
    .bind(runner_id.as_uuid())
    .bind(workspace_id.as_uuid())
    .bind(provider_id.as_str())
    .bind(external_session_id)
    .bind(session_status_to_db(&SessionStatus::Starting))
    .bind(title)
    .fetch_one(pool)
    .await?;

    session_from_row(&row)
}

#[derive(Clone, Debug)]
pub struct CreateSessionRecord {
    pub session_id: SessionId,
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace_id: WorkspaceId,
    pub provider_id: AgentProviderId,
    pub external_session_id: Option<String>,
    pub title: Option<String>,
    pub status: SessionStatus,
    pub usage_snapshot: Option<SessionUsageSnapshot>,
    pub turn_settings: Option<AgentTurnSettings>,
}

pub async fn create_session_with_id(
    pool: &PgPool,
    record: CreateSessionRecord,
) -> Result<AgentSession> {
    let row = sqlx::query(
        "insert into agent_sessions (
            session_id,
            owner_user_id,
            runner_id,
            workspace_id,
            provider_id,
            external_session_id,
            status,
            title,
            usage_snapshot,
            turn_settings
         )
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(record.session_id.as_uuid())
    .bind(record.owner_user_id.as_uuid())
    .bind(record.runner_id.as_uuid())
    .bind(record.workspace_id.as_uuid())
    .bind(record.provider_id.as_str())
    .bind(record.external_session_id)
    .bind(session_status_to_db(&record.status))
    .bind(record.title)
    .bind(
        record
            .usage_snapshot
            .map(|usage| serde_json::to_value(usage).expect("serialize usage snapshot")),
    )
    .bind(
        record
            .turn_settings
            .map(|settings| serde_json::to_value(settings).expect("serialize turn settings")),
    )
    .fetch_one(pool)
    .await?;

    session_from_row(&row)
}

pub struct UpsertSessionByExternalId<'a> {
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace_id: WorkspaceId,
    pub provider_id: AgentProviderId,
    pub external_session_id: &'a str,
    pub title: Option<&'a str>,
    pub updated_at: Option<DateTime<Utc>>,
}

pub async fn upsert_session_by_external_id(
    pool: &PgPool,
    record: UpsertSessionByExternalId<'_>,
) -> Result<AgentSession> {
    let row = sqlx::query(
        "insert into agent_sessions (
            owner_user_id,
            runner_id,
            workspace_id,
            provider_id,
            external_session_id,
            status,
            title,
            updated_at
         )
         values ($1, $2, $3, $4, $5, $6, $7, coalesce($8, now()))
         on conflict (runner_id, workspace_id, provider_id, external_session_id)
         where external_session_id is not null
         do update set title = coalesce(excluded.title, agent_sessions.title),
                       status = case
                           when agent_sessions.status = 'archived' then agent_sessions.status
                           else excluded.status
                       end,
                       updated_at = coalesce(excluded.updated_at, agent_sessions.updated_at)
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(record.owner_user_id.as_uuid())
    .bind(record.runner_id.as_uuid())
    .bind(record.workspace_id.as_uuid())
    .bind(record.provider_id.as_str())
    .bind(record.external_session_id)
    .bind(session_status_to_db(&SessionStatus::Idle))
    .bind(record.title)
    .bind(record.updated_at)
    .fetch_one(pool)
    .await?;

    session_from_row(&row)
}

pub async fn list_sessions_for_user(
    pool: &PgPool,
    owner_user_id: UserId,
) -> Result<Vec<AgentSessionWithWorkspace>> {
    let rows = sqlx::query(
        "select
            s.session_id,
            s.owner_user_id,
            s.runner_id,
            s.workspace_id,
            s.provider_id,
            s.external_session_id,
            s.status,
            s.title,
            s.usage_snapshot,
            s.turn_settings,
            s.created_at,
            s.updated_at,
            w.workspace_id as w_workspace_id,
            w.runner_id as w_runner_id,
            w.path as w_path,
            w.display_name as w_display_name,
            w.created_at as w_created_at,
            w.updated_at as w_updated_at
         from agent_sessions s
         join workspaces w on w.workspace_id = s.workspace_id
         where s.owner_user_id = $1
         order by s.updated_at desc, s.created_at desc",
    )
    .bind(owner_user_id.as_uuid())
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(session_with_workspace_from_row)
        .collect::<Result<Vec<_>>>()
}

pub async fn find_session_for_user(
    pool: &PgPool,
    owner_user_id: UserId,
    session_id: SessionId,
) -> Result<Option<AgentSessionWithWorkspace>> {
    let row = sqlx::query(
        "select
            s.session_id,
            s.owner_user_id,
            s.runner_id,
            s.workspace_id,
            s.provider_id,
            s.external_session_id,
            s.status,
            s.title,
            s.usage_snapshot,
            s.turn_settings,
            s.created_at,
            s.updated_at,
            w.workspace_id as w_workspace_id,
            w.runner_id as w_runner_id,
            w.path as w_path,
            w.display_name as w_display_name,
            w.created_at as w_created_at,
            w.updated_at as w_updated_at
         from agent_sessions s
         join workspaces w on w.workspace_id = s.workspace_id
         where s.owner_user_id = $1 and s.session_id = $2",
    )
    .bind(owner_user_id.as_uuid())
    .bind(session_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    row.as_ref()
        .map(session_with_workspace_from_row)
        .transpose()
}

pub async fn update_session_turn_settings(
    pool: &PgPool,
    owner_user_id: UserId,
    session_id: SessionId,
    settings: Option<&AgentTurnSettings>,
) -> Result<Option<AgentSession>> {
    let settings = settings.map(|settings| {
        serde_json::to_value(settings).expect("turn settings must serialize to JSON")
    });
    let row = sqlx::query(
        "update agent_sessions
         set turn_settings = $3,
             updated_at = now()
         where owner_user_id = $1 and session_id = $2
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(owner_user_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(settings)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(session_from_row).transpose()
}

pub async fn update_session_title(
    pool: &PgPool,
    owner_user_id: UserId,
    session_id: SessionId,
    title: Option<&str>,
) -> Result<Option<AgentSession>> {
    let row = sqlx::query(
        "update agent_sessions
         set title = $3,
             updated_at = now()
         where owner_user_id = $1 and session_id = $2
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(owner_user_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(title)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(session_from_row).transpose()
}

pub async fn update_session_status(
    pool: &PgPool,
    owner_user_id: UserId,
    session_id: SessionId,
    status: SessionStatus,
) -> Result<Option<AgentSession>> {
    let row = sqlx::query(
        "update agent_sessions
         set status = $3,
             updated_at = now()
         where owner_user_id = $1 and session_id = $2
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(owner_user_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(session_status_to_db(&status))
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(session_from_row).transpose()
}

pub async fn update_session_status_by_id(
    pool: &PgPool,
    session_id: SessionId,
    status: SessionStatus,
) -> Result<Option<AgentSession>> {
    let row = sqlx::query(
        "update agent_sessions
         set status = $2,
             updated_at = now()
         where session_id = $1
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(session_id.as_uuid())
    .bind(session_status_to_db(&status))
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(session_from_row).transpose()
}

pub async fn update_session_usage_snapshot(
    pool: &PgPool,
    session_id: SessionId,
    usage_snapshot: &SessionUsageSnapshot,
) -> Result<Option<AgentSession>> {
    let usage_snapshot =
        serde_json::to_value(usage_snapshot).expect("usage snapshot must serialize to JSON");
    let row = sqlx::query(
        "update agent_sessions
         set usage_snapshot = $2,
             updated_at = now()
         where session_id = $1
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, usage_snapshot, turn_settings, created_at, updated_at",
    )
    .bind(session_id.as_uuid())
    .bind(usage_snapshot)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(session_from_row).transpose()
}

pub async fn session_turn_settings(
    pool: &PgPool,
    owner_user_id: UserId,
    session_id: SessionId,
) -> Result<Option<AgentTurnSettings>> {
    let row = sqlx::query(
        "select turn_settings
         from agent_sessions
         where owner_user_id = $1 and session_id = $2",
    )
    .bind(owner_user_id.as_uuid())
    .bind(session_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };
    let settings: Option<serde_json::Value> = row.try_get("turn_settings")?;
    Ok(settings.and_then(|settings| serde_json::from_value(settings).ok()))
}

pub async fn list_event_cache(pool: &PgPool, session_id: SessionId) -> Result<Vec<CachedEvent>> {
    let rows = sqlx::query(
        "select event_id, session_id, event_index, event_type, payload, created_at
         from event_cache
         where session_id = $1
         order by event_index asc",
    )
    .bind(session_id.as_uuid())
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(cached_event_from_row)
        .collect::<Result<Vec<_>>>()
}

pub async fn clear_event_cache(pool: &PgPool, session_id: SessionId) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("delete from event_cache where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from event_cache_cursors where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Clears Agenter's compatibility projection for a session before rewriting history
/// from native-agent discovery. Native harness history remains the source of truth.
pub async fn clear_session_event_projection(pool: &PgPool, session_id: SessionId) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("select 1 from agent_sessions where session_id = $1 for update")
        .bind(session_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("delete from event_cache where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from event_cache_cursors where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from recent_turn_caches where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from session_snapshots where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from agent_events where session_id = $1")
        .bind(session_id.as_uuid())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn append_event_cache(
    pool: &PgPool,
    session_id: SessionId,
    event: &AppEvent,
) -> Result<CachedEvent> {
    let payload = serde_json::to_value(event).map_err(sqlx::Error::decode)?;
    let event_type = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let mut tx = pool.begin().await?;
    let event_index: i64 = sqlx::query_scalar(
        "insert into event_cache_cursors (session_id, next_event_index)
         values ($1, 2)
         on conflict (session_id)
         do update set next_event_index = event_cache_cursors.next_event_index + 1
         returning next_event_index - 1",
    )
    .bind(session_id.as_uuid())
    .fetch_one(&mut *tx)
    .await?;
    let row = sqlx::query(
        "insert into event_cache (session_id, event_index, event_type, payload)
         values ($1, $2, $3, $4)
         returning event_id, session_id, event_index, event_type, payload, created_at",
    )
    .bind(session_id.as_uuid())
    .bind(event_index)
    .bind(&event_type)
    .bind(payload)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    cached_event_from_row(&row)
}

pub async fn append_universal_event(
    pool: &PgPool,
    workspace_id: WorkspaceId,
    envelope: UniversalEventEnvelope,
    command_id: Option<CommandId>,
) -> Result<AgentEvent> {
    let mut tx = pool.begin().await?;
    let event = insert_universal_event_tx(&mut tx, workspace_id, &envelope, command_id).await?;
    tx.commit().await?;
    Ok(event)
}

pub async fn append_universal_event_reducing_snapshot<F>(
    pool: &PgPool,
    workspace_id: WorkspaceId,
    envelope: UniversalEventEnvelope,
    command_id: Option<CommandId>,
    legacy_event: Option<&AppEvent>,
    reduce: F,
) -> Result<UniversalAppendOutcome>
where
    F: FnOnce(&mut SessionSnapshot, &UniversalEventEnvelope) + Send,
{
    let mut tx = pool.begin().await?;
    lock_session_for_projection_tx(&mut tx, envelope.session_id).await?;
    let event = insert_universal_event_tx(&mut tx, workspace_id, &envelope, command_id).await?;
    if let UniversalEventKind::ApprovalRequested { approval } = &event.event {
        if is_resolved_universal_approval_status(&approval.status) {
            resolve_pending_approval_from_universal_tx(&mut tx, approval).await?;
        } else {
            materialize_pending_approval_tx(&mut tx, approval).await?;
        }
    }
    let mut snapshot = load_session_snapshot_for_update_tx(&mut tx, event.session_id).await?;
    reduce(&mut snapshot, &event.envelope());
    snapshot.latest_seq = Some(event.seq);
    let snapshot = store_session_snapshot_tx(&mut tx, &snapshot).await?;
    let cached_event = if let Some(legacy_event) = legacy_event {
        Some(insert_event_cache_tx(&mut tx, event.event_id, event.session_id, legacy_event).await?)
    } else {
        None
    };
    tx.commit().await?;
    Ok(UniversalAppendOutcome {
        event,
        snapshot,
        cached_event,
    })
}

pub async fn list_universal_events_after(
    pool: &PgPool,
    session_id: SessionId,
    after_seq: Option<UniversalSeq>,
    limit: usize,
) -> Result<Vec<AgentEvent>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let after_seq = after_seq.unwrap_or_else(UniversalSeq::zero);
    let rows = sqlx::query(
        "select seq, event_id, workspace_id, session_id, turn_id, item_id,
                event_type, event_json, native_json, source, command_id, created_at
         from agent_events
         where session_id = $1 and seq > $2
         order by seq asc
         limit $3",
    )
    .bind(session_id.as_uuid())
    .bind(after_seq.as_i64())
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(agent_event_from_row)
        .collect::<Result<Vec<_>>>()
}

pub async fn load_stored_session_snapshot(
    pool: &PgPool,
    session_id: SessionId,
) -> Result<Option<StoredSessionSnapshot>> {
    let row = sqlx::query(
        "select session_id, latest_seq, snapshot_json, updated_at
         from session_snapshots
         where session_id = $1",
    )
    .bind(session_id.as_uuid())
    .fetch_optional(pool)
    .await?;

    row.as_ref()
        .map(stored_session_snapshot_from_row)
        .transpose()
}

pub async fn load_session_snapshot(
    pool: &PgPool,
    session_id: SessionId,
) -> Result<Option<SessionSnapshot>> {
    Ok(load_stored_session_snapshot(pool, session_id)
        .await?
        .map(|snapshot| snapshot.snapshot))
}

pub async fn store_session_snapshot(
    pool: &PgPool,
    snapshot: &SessionSnapshot,
) -> Result<StoredSessionSnapshot> {
    let mut tx = pool.begin().await?;
    let stored = store_session_snapshot_tx(&mut tx, snapshot).await?;
    tx.commit().await?;
    Ok(stored)
}

pub async fn begin_command_idempotency(
    pool: &PgPool,
    idempotency_key: &str,
    command_id: CommandId,
    session_id: Option<SessionId>,
    command_json: serde_json::Value,
) -> Result<(CommandIdempotencyRecord, bool)> {
    let response_json = serde_json::json!({
        "command": command_json,
        "response": null,
    });
    let row = sqlx::query(
        "insert into agent_event_idempotency (
            idempotency_key,
            command_id,
            session_id,
            status,
            response_json
         )
         values ($1, $2, $3, 'pending', $4)
         on conflict (idempotency_key) do nothing
         returning idempotency_key, command_id, session_id, status, response_json, created_at, updated_at",
    )
    .bind(idempotency_key)
    .bind(command_id.as_uuid())
    .bind(session_id.map(SessionId::as_uuid))
    .bind(response_json)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = row {
        return command_idempotency_from_row(&row).map(|record| (record, true));
    }

    load_command_idempotency(pool, idempotency_key)
        .await?
        .map(|record| (record, false))
        .ok_or_else(|| sqlx::Error::RowNotFound)
}

pub async fn load_command_idempotency(
    pool: &PgPool,
    idempotency_key: &str,
) -> Result<Option<CommandIdempotencyRecord>> {
    let row = sqlx::query(
        "select idempotency_key, command_id, session_id, status, response_json, created_at, updated_at
         from agent_event_idempotency
         where idempotency_key = $1",
    )
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(command_idempotency_from_row).transpose()
}

pub async fn finish_command_idempotency(
    pool: &PgPool,
    idempotency_key: &str,
    status: CommandIdempotencyStatus,
    command_json: serde_json::Value,
    response_json: serde_json::Value,
) -> Result<CommandIdempotencyRecord> {
    let response_json = serde_json::json!({
        "command": command_json,
        "response": response_json,
    });
    let row = sqlx::query(
        "update agent_event_idempotency
         set status = $2,
             response_json = $3,
             updated_at = now()
         where idempotency_key = $1
         returning idempotency_key, command_id, session_id, status, response_json, created_at, updated_at",
    )
    .bind(idempotency_key)
    .bind(command_idempotency_status_to_db(&status))
    .bind(response_json)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(sqlx::Error::RowNotFound);
    };

    command_idempotency_from_row(&row)
}

pub async fn delete_command_idempotency(pool: &PgPool, idempotency_key: &str) -> Result<()> {
    sqlx::query(
        "delete from agent_event_idempotency
         where idempotency_key = $1",
    )
    .bind(idempotency_key)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn create_approval(
    pool: &PgPool,
    session_id: SessionId,
    kind: ApprovalKind,
    title: &str,
    details: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
    provider_payload: Option<serde_json::Value>,
) -> Result<PendingApproval> {
    let row = sqlx::query(
        "insert into pending_approvals (
            session_id,
            kind,
            title,
            details,
            expires_at,
            provider_payload
         )
         values ($1, $2, $3, $4, $5, $6)
         returning approval_id, session_id, kind, title, details, provider_payload,
             universal_status, native_request_id, canonical_options, risk, subject,
             native_summary, native_json,
             expires_at, resolved_decision, resolved_by_user_id, resolved_at, created_at, updated_at",
    )
    .bind(session_id.as_uuid())
    .bind(approval_kind_to_db(&kind))
    .bind(title)
    .bind(details)
    .bind(expires_at)
    .bind(provider_payload)
    .fetch_one(pool)
    .await?;

    approval_from_row(&row)
}

pub async fn resolve_approval(
    pool: &PgPool,
    approval_id: ApprovalId,
    decision: ApprovalDecision,
    resolved_by_user_id: Option<UserId>,
) -> Result<Option<PendingApproval>> {
    let universal_status = decision_universal_status(&decision);
    let decision = serde_json::to_value(decision).map_err(sqlx::Error::decode)?;
    let resolved_by_user_id = resolved_by_user_id.map(UserId::as_uuid);
    let row = sqlx::query(
        "update pending_approvals
         set resolved_decision = $2,
             resolved_by_user_id = $3,
             resolved_at = now(),
             universal_status = $4,
             updated_at = now()
         where approval_id = $1 and resolved_at is null
         returning approval_id, session_id, kind, title, details, provider_payload,
             universal_status, native_request_id, canonical_options, risk, subject,
             native_summary, native_json,
             expires_at, resolved_decision, resolved_by_user_id, resolved_at, created_at, updated_at",
    )
    .bind(approval_id.as_uuid())
    .bind(decision)
    .bind(resolved_by_user_id)
    .bind(universal_approval_status_to_db(&universal_status))
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(approval_from_row).transpose()
}

pub async fn create_connector_link_code(
    pool: &PgPool,
    code: &str,
    connector_id: &str,
    external_account_id: &str,
    display_name: Option<&str>,
    expires_at: DateTime<Utc>,
) -> Result<ConnectorLinkCode> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "update connector_link_codes
         set consumed_at = now()
         where connector_id = $1
           and external_account_id = $2
           and consumed_at is null",
    )
    .bind(connector_id)
    .bind(external_account_id)
    .execute(&mut *tx)
    .await?;

    let row = sqlx::query(
        "insert into connector_link_codes (
            code,
            connector_id,
            external_account_id,
            display_name,
            expires_at
         )
         values ($1, $2, $3, $4, $5)
         returning code, user_id, connector_id, external_account_id, display_name,
             expires_at, consumed_at, created_at",
    )
    .bind(code)
    .bind(connector_id)
    .bind(external_account_id)
    .bind(display_name)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    connector_link_code_from_row(&row)
}

pub async fn consume_connector_link_code(
    pool: &PgPool,
    code: &str,
    user_id: UserId,
    now: DateTime<Utc>,
) -> Result<Option<ConnectorAccount>> {
    let mut tx = pool.begin().await?;
    let existing_account: Option<PgRow> = sqlx::query(
        "select ca.connector_account_id, ca.user_id, ca.connector_id,
                ca.external_account_id, ca.display_name, ca.linked_at,
                ca.created_at, ca.updated_at
         from connector_link_codes clc
         join connector_accounts ca
           on ca.connector_id = clc.connector_id
          and ca.external_account_id = clc.external_account_id
          and ca.user_id = clc.user_id
         where clc.code = $1
           and clc.user_id = $2
           and clc.consumed_at is not null",
    )
    .bind(code)
    .bind(user_id.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(row) = existing_account {
        let account = connector_account_from_row(&row)?;
        tx.commit().await?;
        return Ok(Some(account));
    }

    let row = sqlx::query(
        "update connector_link_codes
         set user_id = $2,
             consumed_at = $3
         where code = $1
           and user_id is null
           and consumed_at is null
           and expires_at > $3
         returning code, user_id, connector_id, external_account_id, display_name,
             expires_at, consumed_at, created_at",
    )
    .bind(code)
    .bind(user_id.as_uuid())
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(link_code) = row.as_ref().map(connector_link_code_from_row).transpose()? else {
        tx.commit().await?;
        return Ok(None);
    };

    let row = sqlx::query(
        "insert into connector_accounts (
            user_id,
            connector_id,
            external_account_id,
            display_name,
            linked_at
         )
         values ($1, $2, $3, $4, $5)
         on conflict (connector_id, external_account_id)
         do update set user_id = excluded.user_id,
                       display_name = excluded.display_name,
                       linked_at = excluded.linked_at,
                       updated_at = now()
         returning connector_account_id, user_id, connector_id, external_account_id,
             display_name, linked_at, created_at, updated_at",
    )
    .bind(user_id.as_uuid())
    .bind(&link_code.connector_id)
    .bind(&link_code.external_account_id)
    .bind(&link_code.display_name)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let account = connector_account_from_row(&row)?;
    tx.commit().await?;
    Ok(Some(account))
}

async fn insert_universal_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: WorkspaceId,
    envelope: &UniversalEventEnvelope,
    command_id: Option<CommandId>,
) -> Result<AgentEvent> {
    let event_id = parse_control_plane_event_id(&envelope.event_id)?;
    let event_json = serde_json::to_value(&envelope.event).map_err(sqlx::Error::decode)?;
    let native_json = envelope
        .native
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(sqlx::Error::decode)?;
    let row = sqlx::query(
        "insert into agent_events (
            event_id,
            workspace_id,
            session_id,
            turn_id,
            item_id,
            event_type,
            event_json,
            native_json,
            source,
            command_id,
            created_at
         )
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         returning seq, event_id, workspace_id, session_id, turn_id, item_id,
             event_type, event_json, native_json, source, command_id, created_at",
    )
    .bind(event_id)
    .bind(workspace_id.as_uuid())
    .bind(envelope.session_id.as_uuid())
    .bind(envelope.turn_id.map(TurnId::as_uuid))
    .bind(envelope.item_id.map(ItemId::as_uuid))
    .bind(universal_event_type(&envelope.event))
    .bind(event_json)
    .bind(native_json)
    .bind(universal_event_source_to_db(&envelope.source))
    .bind(command_id.map(CommandId::as_uuid))
    .bind(envelope.ts)
    .fetch_one(&mut **tx)
    .await?;

    agent_event_from_row(&row)
}

async fn lock_session_for_projection_tx(
    tx: &mut Transaction<'_, Postgres>,
    session_id: SessionId,
) -> Result<()> {
    sqlx::query("select 1 from agent_sessions where session_id = $1 for update")
        .bind(session_id.as_uuid())
        .fetch_one(&mut **tx)
        .await?;
    Ok(())
}

async fn load_session_snapshot_for_update_tx(
    tx: &mut Transaction<'_, Postgres>,
    session_id: SessionId,
) -> Result<SessionSnapshot> {
    let row = sqlx::query(
        "select session_id, latest_seq, snapshot_json, updated_at
         from session_snapshots
         where session_id = $1
         for update",
    )
    .bind(session_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;

    if let Some(row) = row {
        return stored_session_snapshot_from_row(&row).map(|stored| stored.snapshot);
    }

    Ok(SessionSnapshot {
        session_id,
        ..SessionSnapshot::default()
    })
}

async fn store_session_snapshot_tx(
    tx: &mut Transaction<'_, Postgres>,
    snapshot: &SessionSnapshot,
) -> Result<StoredSessionSnapshot> {
    let latest_seq = snapshot.latest_seq.unwrap_or_else(UniversalSeq::zero);
    let snapshot_json = serde_json::to_value(snapshot).map_err(sqlx::Error::decode)?;
    let row = sqlx::query(
        "insert into session_snapshots (session_id, latest_seq, snapshot_json)
         values ($1, $2, $3)
         on conflict (session_id)
         do update set latest_seq = excluded.latest_seq,
                       snapshot_json = excluded.snapshot_json,
                       updated_at = now()
         where session_snapshots.latest_seq <= excluded.latest_seq
         returning session_id, latest_seq, snapshot_json, updated_at",
    )
    .bind(snapshot.session_id.as_uuid())
    .bind(latest_seq.as_i64())
    .bind(snapshot_json)
    .fetch_optional(&mut **tx)
    .await?;

    let Some(row) = row else {
        return Err(sqlx::Error::ColumnDecode {
            index: "latest_seq".to_owned(),
            source: format!(
                "session snapshot latest_seq regression for session {}",
                snapshot.session_id
            )
            .into(),
        });
    };

    stored_session_snapshot_from_row(&row)
}

async fn insert_event_cache_tx(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    session_id: SessionId,
    event: &AppEvent,
) -> Result<CachedEvent> {
    let payload = serde_json::to_value(event).map_err(sqlx::Error::decode)?;
    let event_type = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let event_index: i64 = sqlx::query_scalar(
        "insert into event_cache_cursors (session_id, next_event_index)
         values ($1, 2)
         on conflict (session_id)
         do update set next_event_index = event_cache_cursors.next_event_index + 1
         returning next_event_index - 1",
    )
    .bind(session_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    let row = sqlx::query(
        "insert into event_cache (event_id, session_id, event_index, event_type, payload)
         values ($1, $2, $3, $4, $5)
         returning event_id, session_id, event_index, event_type, payload, created_at",
    )
    .bind(event_id)
    .bind(session_id.as_uuid())
    .bind(event_index)
    .bind(&event_type)
    .bind(payload)
    .fetch_one(&mut **tx)
    .await?;

    cached_event_from_row(&row)
}

async fn materialize_pending_approval_tx(
    tx: &mut Transaction<'_, Postgres>,
    approval: &ApprovalRequest,
) -> Result<()> {
    let canonical_options = serde_json::to_value(&approval.options).map_err(sqlx::Error::decode)?;
    let native_json = approval
        .native
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(sqlx::Error::decode)?;
    let native_request_id = approval.native_request_id.as_deref().or_else(|| {
        approval
            .native
            .as_ref()
            .and_then(|native| native.native_id.as_deref())
    });
    let native_summary = approval
        .native
        .as_ref()
        .and_then(|native| native.summary.as_deref());
    sqlx::query(
        "insert into pending_approvals (
            approval_id,
            session_id,
            kind,
            title,
            details,
            provider_payload,
            universal_status,
            native_request_id,
            canonical_options,
            risk,
            subject,
            native_summary,
            native_json,
            resolved_at
         )
         values ($1, $2, $3, $4, $5, null, $6, $7, $8, $9, $10, $11, $12, $13)
         on conflict (approval_id)
         do update set kind = excluded.kind,
                       title = excluded.title,
                       details = excluded.details,
                       universal_status = case
                           when pending_approvals.resolved_at is not null then pending_approvals.universal_status
                           else excluded.universal_status
                       end,
                       native_request_id = excluded.native_request_id,
                       canonical_options = excluded.canonical_options,
                       risk = excluded.risk,
                       subject = excluded.subject,
                       native_summary = excluded.native_summary,
                       native_json = excluded.native_json,
                       resolved_at = coalesce(excluded.resolved_at, pending_approvals.resolved_at),
                       updated_at = now()",
    )
    .bind(approval.approval_id.as_uuid())
    .bind(approval.session_id.as_uuid())
    .bind(approval_kind_to_db(&approval.kind))
    .bind(&approval.title)
    .bind(&approval.details)
    .bind(universal_approval_status_to_db(&approval.status))
    .bind(native_request_id)
    .bind(canonical_options)
    .bind(&approval.risk)
    .bind(&approval.subject)
    .bind(native_summary)
    .bind(native_json)
    .bind(approval.resolved_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn resolve_pending_approval_from_universal_tx(
    tx: &mut Transaction<'_, Postgres>,
    approval: &ApprovalRequest,
) -> Result<()> {
    let canonical_options = serde_json::to_value(&approval.options).map_err(sqlx::Error::decode)?;
    let native_json = approval
        .native
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(sqlx::Error::decode)?;
    let native_request_id = approval.native_request_id.as_deref().or_else(|| {
        approval
            .native
            .as_ref()
            .and_then(|native| native.native_id.as_deref())
    });
    let native_summary = approval
        .native
        .as_ref()
        .and_then(|native| native.summary.as_deref());
    sqlx::query(
        "insert into pending_approvals (
            approval_id,
            session_id,
            kind,
            title,
            details,
            provider_payload,
            universal_status,
            native_request_id,
            canonical_options,
            risk,
            subject,
            native_summary,
            native_json,
            resolved_at
         )
         values ($1, $2, $3, $4, $5, null, $6, $7, $8, $9, $10, $11, $12, $13)
         on conflict (approval_id)
         do update set universal_status = excluded.universal_status,
                       native_request_id = coalesce(excluded.native_request_id, pending_approvals.native_request_id),
                       canonical_options = case
                           when excluded.canonical_options = '[]'::jsonb then pending_approvals.canonical_options
                           else excluded.canonical_options
                       end,
                       risk = coalesce(excluded.risk, pending_approvals.risk),
                       subject = coalesce(excluded.subject, pending_approvals.subject),
                       native_summary = coalesce(excluded.native_summary, pending_approvals.native_summary),
                       native_json = coalesce(excluded.native_json, pending_approvals.native_json),
                       resolved_at = coalesce(excluded.resolved_at, pending_approvals.resolved_at, now()),
                       updated_at = now()",
    )
    .bind(approval.approval_id.as_uuid())
    .bind(approval.session_id.as_uuid())
    .bind(approval_kind_to_db(&approval.kind))
    .bind(&approval.title)
    .bind(&approval.details)
    .bind(universal_approval_status_to_db(&approval.status))
    .bind(native_request_id)
    .bind(canonical_options)
    .bind(&approval.risk)
    .bind(&approval.subject)
    .bind(native_summary)
    .bind(native_json)
    .bind(approval.resolved_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn user_from_row(row: &PgRow) -> Result<User> {
    Ok(User {
        user_id: UserId::from_uuid(row.try_get("user_id")?),
        email: row.try_get("email")?,
        display_name: row.try_get("display_name")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn runner_from_row(row: &PgRow) -> Result<Runner> {
    Ok(Runner {
        runner_id: RunnerId::from_uuid(row.try_get("runner_id")?),
        name: row.try_get("name")?,
        version: row.try_get("version")?,
        last_seen_at: row.try_get("last_seen_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn workspace_from_row(row: &PgRow) -> Result<Workspace> {
    Ok(Workspace {
        workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id")?),
        runner_id: RunnerId::from_uuid(row.try_get("runner_id")?),
        path: row.try_get("path")?,
        display_name: row.try_get("display_name")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn session_from_row(row: &PgRow) -> Result<AgentSession> {
    let status: String = row.try_get("status")?;
    let provider_id: String = row.try_get("provider_id")?;
    let usage_snapshot: Option<serde_json::Value> = row.try_get("usage_snapshot")?;
    let turn_settings: Option<serde_json::Value> = row.try_get("turn_settings")?;
    Ok(AgentSession {
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        owner_user_id: UserId::from_uuid(row.try_get("owner_user_id")?),
        runner_id: RunnerId::from_uuid(row.try_get("runner_id")?),
        workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id")?),
        provider_id: AgentProviderId::from(provider_id),
        external_session_id: row.try_get("external_session_id")?,
        status: session_status_from_db(&status)?,
        title: row.try_get("title")?,
        usage_snapshot: usage_snapshot.and_then(|usage| serde_json::from_value(usage).ok()),
        turn_settings: turn_settings.and_then(|settings| serde_json::from_value(settings).ok()),
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn cached_event_from_row(row: &PgRow) -> Result<CachedEvent> {
    Ok(CachedEvent {
        event_id: row.try_get("event_id")?,
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        event_index: row.try_get("event_index")?,
        event_type: row.try_get("event_type")?,
        payload: row.try_get("payload")?,
        created_at: row.try_get("created_at")?,
    })
}

fn agent_event_from_row(row: &PgRow) -> Result<AgentEvent> {
    let turn_id: Option<Uuid> = row.try_get("turn_id")?;
    let item_id: Option<Uuid> = row.try_get("item_id")?;
    let command_id: Option<Uuid> = row.try_get("command_id")?;
    let source: String = row.try_get("source")?;
    let event_json: serde_json::Value = row.try_get("event_json")?;
    let native_json: Option<serde_json::Value> = row.try_get("native_json")?;
    Ok(AgentEvent {
        seq: UniversalSeq::new(row.try_get("seq")?),
        event_id: row.try_get("event_id")?,
        workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id")?),
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        turn_id: turn_id.map(TurnId::from_uuid),
        item_id: item_id.map(ItemId::from_uuid),
        event_type: row.try_get("event_type")?,
        event: serde_json::from_value(event_json).map_err(sqlx::Error::decode)?,
        native: native_json
            .map(serde_json::from_value)
            .transpose()
            .map_err(sqlx::Error::decode)?,
        source: universal_event_source_from_db(&source)?,
        command_id: command_id.map(CommandId::from_uuid),
        created_at: row.try_get("created_at")?,
    })
}

fn stored_session_snapshot_from_row(row: &PgRow) -> Result<StoredSessionSnapshot> {
    let snapshot_json: serde_json::Value = row.try_get("snapshot_json")?;
    Ok(StoredSessionSnapshot {
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        latest_seq: UniversalSeq::new(row.try_get("latest_seq")?),
        snapshot: serde_json::from_value(snapshot_json).map_err(sqlx::Error::decode)?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn command_idempotency_from_row(row: &PgRow) -> Result<CommandIdempotencyRecord> {
    let status: String = row.try_get("status")?;
    let session_id: Option<Uuid> = row.try_get("session_id")?;
    Ok(CommandIdempotencyRecord {
        idempotency_key: row.try_get("idempotency_key")?,
        command_id: CommandId::from_uuid(row.try_get("command_id")?),
        session_id: session_id.map(SessionId::from_uuid),
        status: command_idempotency_status_from_db(&status)?,
        response_json: row.try_get("response_json")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn session_with_workspace_from_row(row: &PgRow) -> Result<AgentSessionWithWorkspace> {
    let status: String = row.try_get("status")?;
    let provider_id: String = row.try_get("provider_id")?;
    let usage_snapshot: Option<serde_json::Value> = row.try_get("usage_snapshot")?;
    let turn_settings: Option<serde_json::Value> = row.try_get("turn_settings")?;
    Ok(AgentSessionWithWorkspace {
        session: AgentSession {
            session_id: SessionId::from_uuid(row.try_get("session_id")?),
            owner_user_id: UserId::from_uuid(row.try_get("owner_user_id")?),
            runner_id: RunnerId::from_uuid(row.try_get("runner_id")?),
            workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id")?),
            provider_id: AgentProviderId::from(provider_id),
            external_session_id: row.try_get("external_session_id")?,
            status: session_status_from_db(&status)?,
            title: row.try_get("title")?,
            usage_snapshot: usage_snapshot.and_then(|usage| serde_json::from_value(usage).ok()),
            turn_settings: turn_settings.and_then(|settings| serde_json::from_value(settings).ok()),
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        },
        workspace: Workspace {
            workspace_id: WorkspaceId::from_uuid(row.try_get("w_workspace_id")?),
            runner_id: RunnerId::from_uuid(row.try_get("w_runner_id")?),
            path: row.try_get("w_path")?,
            display_name: row.try_get("w_display_name")?,
            created_at: row.try_get("w_created_at")?,
            updated_at: row.try_get("w_updated_at")?,
        },
    })
}

fn approval_from_row(row: &PgRow) -> Result<PendingApproval> {
    let kind: String = row.try_get("kind")?;
    let universal_status: String = row.try_get("universal_status")?;
    let canonical_options: serde_json::Value = row.try_get("canonical_options")?;
    let native_json: Option<serde_json::Value> = row.try_get("native_json")?;
    let resolved_decision: Option<serde_json::Value> = row.try_get("resolved_decision")?;
    let resolved_by_user_id: Option<Uuid> = row.try_get("resolved_by_user_id")?;
    Ok(PendingApproval {
        approval_id: ApprovalId::from_uuid(row.try_get("approval_id")?),
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        kind: approval_kind_from_db(&kind)?,
        title: row.try_get("title")?,
        details: row.try_get("details")?,
        provider_payload: row.try_get("provider_payload")?,
        universal_status: universal_approval_status_from_db(&universal_status)?,
        native_request_id: row.try_get("native_request_id")?,
        canonical_options: serde_json::from_value(canonical_options)
            .map_err(sqlx::Error::decode)?,
        risk: row.try_get("risk")?,
        subject: row.try_get("subject")?,
        native_summary: row.try_get("native_summary")?,
        native: native_json
            .map(serde_json::from_value)
            .transpose()
            .map_err(sqlx::Error::decode)?,
        expires_at: row.try_get("expires_at")?,
        resolved_decision: resolved_decision
            .map(serde_json::from_value)
            .transpose()
            .map_err(sqlx::Error::decode)?,
        resolved_by_user_id: resolved_by_user_id.map(UserId::from_uuid),
        resolved_at: row.try_get("resolved_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn oidc_provider_from_row(row: &PgRow) -> Result<OidcProvider> {
    Ok(OidcProvider {
        provider_id: row.try_get("oidc_provider_id")?,
        display_name: row.try_get("display_name")?,
        issuer_url: row.try_get("issuer_url")?,
        client_id: row.try_get("client_id")?,
        client_secret_ciphertext: row.try_get("client_secret_ciphertext")?,
        scopes: row.try_get("scopes")?,
        enabled: row.try_get("enabled")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn oidc_login_state_from_row(row: &PgRow) -> Result<OidcLoginState> {
    Ok(OidcLoginState {
        state: row.try_get("state")?,
        provider_id: row.try_get("provider_id")?,
        nonce: row.try_get("nonce")?,
        pkce_verifier: row.try_get("pkce_verifier")?,
        return_to: row.try_get("return_to")?,
        expires_at: row.try_get("expires_at")?,
        consumed_at: row.try_get("consumed_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn browser_auth_session_from_row(row: &PgRow) -> Result<BrowserAuthSession> {
    Ok(BrowserAuthSession {
        session_token_hash: row.try_get("session_token_hash")?,
        user_id: UserId::from_uuid(row.try_get("user_id")?),
        expires_at: row.try_get("expires_at")?,
        revoked_at: row.try_get("revoked_at")?,
        created_at: row.try_get("created_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
    })
}

fn connector_account_from_row(row: &PgRow) -> Result<ConnectorAccount> {
    Ok(ConnectorAccount {
        connector_account_id: row.try_get("connector_account_id")?,
        user_id: UserId::from_uuid(row.try_get("user_id")?),
        connector_id: row.try_get("connector_id")?,
        external_account_id: row.try_get("external_account_id")?,
        display_name: row.try_get("display_name")?,
        linked_at: row.try_get("linked_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn connector_link_code_from_row(row: &PgRow) -> Result<ConnectorLinkCode> {
    let user_id: Option<Uuid> = row.try_get("user_id")?;
    Ok(ConnectorLinkCode {
        code: row.try_get("code")?,
        user_id: user_id.map(UserId::from_uuid),
        connector_id: row.try_get("connector_id")?,
        external_account_id: row.try_get("external_account_id")?,
        display_name: row.try_get("display_name")?,
        expires_at: row.try_get("expires_at")?,
        consumed_at: row.try_get("consumed_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn session_status_to_db(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Starting => "starting",
        SessionStatus::Running => "running",
        SessionStatus::WaitingForInput => "waiting_for_input",
        SessionStatus::WaitingForApproval => "waiting_for_approval",
        SessionStatus::Idle => "idle",
        SessionStatus::Stopped => "stopped",
        SessionStatus::Completed => "completed",
        SessionStatus::Interrupted => "interrupted",
        SessionStatus::Degraded => "degraded",
        SessionStatus::Failed => "failed",
        SessionStatus::Archived => "archived",
    }
}

fn session_status_from_db(value: &str) -> Result<SessionStatus> {
    match value {
        "starting" => Ok(SessionStatus::Starting),
        "running" => Ok(SessionStatus::Running),
        "waiting_for_input" => Ok(SessionStatus::WaitingForInput),
        "waiting_for_approval" => Ok(SessionStatus::WaitingForApproval),
        "idle" => Ok(SessionStatus::Idle),
        "stopped" => Ok(SessionStatus::Stopped),
        "completed" => Ok(SessionStatus::Completed),
        "interrupted" => Ok(SessionStatus::Interrupted),
        "degraded" => Ok(SessionStatus::Degraded),
        "failed" => Ok(SessionStatus::Failed),
        "archived" => Ok(SessionStatus::Archived),
        _ => Err(sqlx::Error::ColumnDecode {
            index: "status".to_owned(),
            source: format!("unknown session status {value:?}").into(),
        }),
    }
}

fn approval_kind_to_db(kind: &ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::Command => "command",
        ApprovalKind::FileChange => "file_change",
        ApprovalKind::Tool => "tool",
        ApprovalKind::ProviderSpecific => "provider_specific",
    }
}

fn approval_kind_from_db(value: &str) -> Result<ApprovalKind> {
    match value {
        "command" => Ok(ApprovalKind::Command),
        "file_change" => Ok(ApprovalKind::FileChange),
        "tool" => Ok(ApprovalKind::Tool),
        "provider_specific" => Ok(ApprovalKind::ProviderSpecific),
        _ => Err(sqlx::Error::ColumnDecode {
            index: "kind".to_owned(),
            source: format!("unknown approval kind {value:?}").into(),
        }),
    }
}

fn parse_control_plane_event_id(value: &str) -> Result<Uuid> {
    Uuid::parse_str(value).map_err(|error| sqlx::Error::ColumnDecode {
        index: "event_id".to_owned(),
        source: format!(
            "universal event_id must be a control-plane UUID; native stable IDs belong in NativeRef: {error}"
        )
        .into(),
    })
}

fn universal_event_type(event: &UniversalEventKind) -> &'static str {
    match event {
        UniversalEventKind::SessionCreated { .. } => "session.created",
        UniversalEventKind::TurnStarted { .. } => "turn.started",
        UniversalEventKind::TurnStatusChanged { .. } => "turn.status_changed",
        UniversalEventKind::TurnCompleted { .. } => "turn.completed",
        UniversalEventKind::TurnFailed { .. } => "turn.failed",
        UniversalEventKind::TurnCancelled { .. } => "turn.cancelled",
        UniversalEventKind::TurnInterrupted { .. } => "turn.interrupted",
        UniversalEventKind::TurnDetached { .. } => "turn.detached",
        UniversalEventKind::ItemCreated { .. } => "item.created",
        UniversalEventKind::ContentDelta { .. } => "content.delta",
        UniversalEventKind::ContentCompleted { .. } => "content.completed",
        UniversalEventKind::ApprovalRequested { .. } => "approval.requested",
        UniversalEventKind::PlanUpdated { .. } => "plan.updated",
        UniversalEventKind::DiffUpdated { .. } => "diff.updated",
        UniversalEventKind::ArtifactCreated { .. } => "artifact.created",
        UniversalEventKind::UsageUpdated { .. } => "usage.updated",
        UniversalEventKind::NativeUnknown { .. } => "native.unknown",
    }
}

fn universal_event_source_to_db(source: &UniversalEventSource) -> &'static str {
    match source {
        UniversalEventSource::ControlPlane => "control_plane",
        UniversalEventSource::Runner => "runner",
        UniversalEventSource::Browser => "browser",
        UniversalEventSource::Connector => "connector",
        UniversalEventSource::Native => "native",
    }
}

fn universal_event_source_from_db(value: &str) -> Result<UniversalEventSource> {
    match value {
        "control_plane" => Ok(UniversalEventSource::ControlPlane),
        "runner" => Ok(UniversalEventSource::Runner),
        "browser" => Ok(UniversalEventSource::Browser),
        "connector" => Ok(UniversalEventSource::Connector),
        "native" => Ok(UniversalEventSource::Native),
        _ => Err(sqlx::Error::ColumnDecode {
            index: "source".to_owned(),
            source: format!("unknown universal event source {value:?}").into(),
        }),
    }
}

fn decision_universal_status(decision: &ApprovalDecision) -> UniversalApprovalStatus {
    match decision {
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
            UniversalApprovalStatus::Approved
        }
        ApprovalDecision::Cancel => UniversalApprovalStatus::Cancelled,
        ApprovalDecision::Decline => UniversalApprovalStatus::Denied,
        ApprovalDecision::ProviderSpecific { payload } => {
            provider_specific_decision_status(payload).unwrap_or(UniversalApprovalStatus::Denied)
        }
    }
}

fn provider_specific_decision_status(
    payload: &serde_json::Value,
) -> Option<UniversalApprovalStatus> {
    let value = payload
        .pointer("/decision")
        .or_else(|| payload.pointer("/status"))
        .or_else(|| payload.pointer("/kind"))?
        .as_str()?;
    match value {
        "accept" | "approve" | "approved" | "allow" | "allowed" => {
            Some(UniversalApprovalStatus::Approved)
        }
        "cancel" | "cancelled" | "canceled" => Some(UniversalApprovalStatus::Cancelled),
        "decline" | "deny" | "denied" | "reject" | "rejected" => {
            Some(UniversalApprovalStatus::Denied)
        }
        _ => None,
    }
}

fn is_resolved_universal_approval_status(status: &UniversalApprovalStatus) -> bool {
    matches!(
        status,
        UniversalApprovalStatus::Approved
            | UniversalApprovalStatus::Denied
            | UniversalApprovalStatus::Cancelled
            | UniversalApprovalStatus::Expired
            | UniversalApprovalStatus::Orphaned
    )
}

fn universal_approval_status_to_db(status: &UniversalApprovalStatus) -> &'static str {
    match status {
        UniversalApprovalStatus::Pending => "pending",
        UniversalApprovalStatus::Presented => "presented",
        UniversalApprovalStatus::Resolving => "resolving",
        UniversalApprovalStatus::Approved => "approved",
        UniversalApprovalStatus::Denied => "denied",
        UniversalApprovalStatus::Cancelled => "cancelled",
        UniversalApprovalStatus::Expired => "expired",
        UniversalApprovalStatus::Orphaned => "orphaned",
    }
}

fn universal_approval_status_from_db(value: &str) -> Result<UniversalApprovalStatus> {
    match value {
        "pending" => Ok(UniversalApprovalStatus::Pending),
        "presented" => Ok(UniversalApprovalStatus::Presented),
        "resolving" => Ok(UniversalApprovalStatus::Resolving),
        "approved" => Ok(UniversalApprovalStatus::Approved),
        "denied" => Ok(UniversalApprovalStatus::Denied),
        "cancelled" => Ok(UniversalApprovalStatus::Cancelled),
        "expired" => Ok(UniversalApprovalStatus::Expired),
        "orphaned" => Ok(UniversalApprovalStatus::Orphaned),
        _ => Err(sqlx::Error::ColumnDecode {
            index: "universal_status".to_owned(),
            source: format!("unknown universal approval status {value:?}").into(),
        }),
    }
}

fn command_idempotency_status_to_db(status: &CommandIdempotencyStatus) -> &'static str {
    match status {
        CommandIdempotencyStatus::Pending => "pending",
        CommandIdempotencyStatus::Succeeded => "succeeded",
        CommandIdempotencyStatus::Failed => "failed",
        CommandIdempotencyStatus::Conflict => "conflict",
    }
}

fn command_idempotency_status_from_db(value: &str) -> Result<CommandIdempotencyStatus> {
    match value {
        "pending" => Ok(CommandIdempotencyStatus::Pending),
        "succeeded" => Ok(CommandIdempotencyStatus::Succeeded),
        "failed" => Ok(CommandIdempotencyStatus::Failed),
        "conflict" => Ok(CommandIdempotencyStatus::Conflict),
        _ => Err(sqlx::Error::ColumnDecode {
            index: "status".to_owned(),
            source: format!("unknown command idempotency status {value:?}").into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentProviderId, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind, ApprovalRequest,
        ApprovalStatus as UniversalApprovalStatus, NativeRef, SessionInfo, SessionSnapshot,
        SessionStatus, UniversalEventEnvelope, UniversalEventKind, UniversalEventSource,
        UniversalSeq, UserMessageEvent,
    };
    use sqlx::{Executor, PgPool, Row};

    use super::*;

    #[test]
    fn rejects_non_uuid_universal_event_ids_with_clear_error() {
        let error = parse_control_plane_event_id("native-event-1").expect_err("must reject");
        let message = error.to_string();

        assert!(message.contains("universal event_id must be a control-plane UUID"));
        assert!(message.contains("native stable IDs belong in NativeRef"));
    }

    #[test]
    fn maps_approval_decisions_to_universal_statuses() {
        assert_eq!(
            decision_universal_status(&ApprovalDecision::Accept),
            UniversalApprovalStatus::Approved
        );
        assert_eq!(
            decision_universal_status(&ApprovalDecision::Cancel),
            UniversalApprovalStatus::Cancelled
        );
        assert_eq!(
            decision_universal_status(&ApprovalDecision::Decline),
            UniversalApprovalStatus::Denied
        );
        assert_eq!(
            decision_universal_status(&ApprovalDecision::ProviderSpecific {
                payload: serde_json::json!({ "status": "approved" }),
            }),
            UniversalApprovalStatus::Approved
        );
    }

    async fn test_pool() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set to run ignored SQLx integration tests");

        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn finish_command_idempotency_missing_row_returns_row_not_found() {
        let pool = test_pool().await;
        let error = finish_command_idempotency(
            &pool,
            &format!("missing-command-{}", uuid::Uuid::new_v4()),
            CommandIdempotencyStatus::Failed,
            serde_json::json!({ "type": "close_session" }),
            serde_json::json!({ "status": 404 }),
        )
        .await
        .expect_err("missing command row must fail closed");

        assert!(matches!(error, sqlx::Error::RowNotFound));
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn creates_registry_rows_and_appends_event_cache() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let user = create_user(
            &pool,
            &format!("user-{suffix}@example.test"),
            Some("Test User"),
        )
        .await
        .expect("create user");
        let runner = register_runner(&pool, &format!("runner-{suffix}"), Some("test"))
            .await
            .expect("register runner");
        let workspace = upsert_workspace(
            &pool,
            runner.runner_id,
            &format!("/tmp/agenter-test-{suffix}"),
            Some("Test Workspace"),
        )
        .await
        .expect("upsert workspace");

        let updated_workspace = upsert_workspace(
            &pool,
            runner.runner_id,
            &workspace.path,
            Some("Renamed Workspace"),
        )
        .await
        .expect("upsert existing workspace");

        assert_eq!(workspace.workspace_id, updated_workspace.workspace_id);
        assert_eq!(
            updated_workspace.display_name.as_deref(),
            Some("Renamed Workspace")
        );

        let session = create_session(
            &pool,
            user.user_id,
            runner.runner_id,
            workspace.workspace_id,
            AgentProviderId::from(AgentProviderId::CODEX),
            Some("external-session-1"),
            Some("First Session"),
        )
        .await
        .expect("create session");

        assert_eq!(session.status, SessionStatus::Starting);

        let first_event = append_event_cache(
            &pool,
            session.session_id,
            &AppEvent::UserMessage(UserMessageEvent {
                session_id: session.session_id,
                message_id: Some("message-1".to_owned()),
                author_user_id: Some(user.user_id),
                content: "hello".to_owned(),
            }),
        )
        .await
        .expect("append event");
        let second_event = append_event_cache(
            &pool,
            session.session_id,
            &AppEvent::UserMessage(UserMessageEvent {
                session_id: session.session_id,
                message_id: Some("message-2".to_owned()),
                author_user_id: Some(user.user_id),
                content: "again".to_owned(),
            }),
        )
        .await
        .expect("append second event");

        assert_eq!(first_event.event_index, 1);
        assert_eq!(first_event.event_type, "user_message");
        assert_eq!(second_event.event_index, 2);
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn universal_event_log_sequences_replays_snapshots_approvals_and_legacy_cache() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let user = create_user(
            &pool,
            &format!("universal-user-{suffix}@example.test"),
            Some("Universal User"),
        )
        .await
        .expect("create user");
        let runner = register_runner(&pool, &format!("universal-runner-{suffix}"), Some("test"))
            .await
            .expect("register runner");
        let workspace = upsert_workspace(
            &pool,
            runner.runner_id,
            &format!("/tmp/agenter-universal-test-{suffix}"),
            Some("Universal Workspace"),
        )
        .await
        .expect("upsert workspace");
        let session = create_session(
            &pool,
            user.user_id,
            runner.runner_id,
            workspace.workspace_id,
            AgentProviderId::from(AgentProviderId::CODEX),
            None,
            Some("Universal Session"),
        )
        .await
        .expect("create session");

        let session_event = UniversalEventEnvelope {
            event_id: uuid::Uuid::new_v4().to_string(),
            seq: UniversalSeq::zero(),
            session_id: session.session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::ControlPlane,
            native: None,
            event: UniversalEventKind::SessionCreated {
                session: Box::new(SessionInfo {
                    session_id: session.session_id,
                    owner_user_id: user.user_id,
                    runner_id: runner.runner_id,
                    workspace_id: workspace.workspace_id,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    status: SessionStatus::Running,
                    external_session_id: None,
                    title: Some("Universal Session".to_owned()),
                    created_at: None,
                    updated_at: None,
                    usage: None,
                }),
            },
        };
        let first = append_universal_event_reducing_snapshot(
            &pool,
            workspace.workspace_id,
            session_event,
            None,
            None,
            |snapshot: &mut SessionSnapshot, event| {
                snapshot.session_id = event.session_id;
                snapshot.latest_seq = Some(event.seq);
                if let UniversalEventKind::SessionCreated { session } = &event.event {
                    snapshot.info = Some((**session).clone());
                }
            },
        )
        .await
        .expect("append universal session event");

        let approval_id = ApprovalId::new();
        let approval_event = UniversalEventEnvelope {
            event_id: uuid::Uuid::new_v4().to_string(),
            seq: UniversalSeq::zero(),
            session_id: session.session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::ApprovalRequested {
                approval: Box::new(ApprovalRequest {
                    approval_id,
                    session_id: session.session_id,
                    turn_id: None,
                    item_id: None,
                    kind: ApprovalKind::Command,
                    title: "Run command".to_owned(),
                    details: Some("cargo test".to_owned()),
                    options: Vec::new(),
                    status: UniversalApprovalStatus::Pending,
                    risk: Some("writes".to_owned()),
                    subject: Some("cargo test".to_owned()),
                    native_request_id: Some("canonical-native-approval-1".to_owned()),
                    native_blocking: true,
                    policy: None,
                    native: Some(NativeRef {
                        protocol: "codex.app_server".to_owned(),
                        method: Some("approval/requested".to_owned()),
                        kind: Some("command".to_owned()),
                        native_id: Some("native-approval-1".to_owned()),
                        summary: Some("Run command".to_owned()),
                        hash: None,
                        pointer: None,
                    }),
                    requested_at: Some(Utc::now()),
                    resolved_at: None,
                }),
            },
        };
        let legacy_event = AppEvent::UserMessage(UserMessageEvent {
            session_id: session.session_id,
            message_id: Some("legacy-user-1".to_owned()),
            author_user_id: Some(user.user_id),
            content: "legacy cache still exists".to_owned(),
        });
        let second = append_universal_event_reducing_snapshot(
            &pool,
            workspace.workspace_id,
            approval_event,
            None,
            Some(&legacy_event),
            |snapshot: &mut SessionSnapshot, event| {
                snapshot.session_id = event.session_id;
                snapshot.latest_seq = Some(event.seq);
                if let UniversalEventKind::ApprovalRequested { approval } = &event.event {
                    snapshot
                        .approvals
                        .insert(approval.approval_id, (**approval).clone());
                }
            },
        )
        .await
        .expect("append universal approval event");

        assert!(second.event.seq > first.event.seq);
        assert!(second.cached_event.is_some());

        let replay =
            list_universal_events_after(&pool, session.session_id, Some(first.event.seq), 10)
                .await
                .expect("list events after first seq");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].seq, second.event.seq);
        assert_eq!(replay[0].event_type, "approval.requested");

        let snapshot = load_session_snapshot(&pool, session.session_id)
            .await
            .expect("load snapshot")
            .expect("snapshot exists");
        assert_eq!(snapshot.latest_seq, Some(second.event.seq));
        assert!(snapshot.approvals.contains_key(&approval_id));

        let approval_row = sqlx::query(
            "select universal_status, native_request_id, native_json
             from pending_approvals
             where approval_id = $1",
        )
        .bind(approval_id.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("load materialized approval");
        let universal_status: String = approval_row
            .try_get("universal_status")
            .expect("approval status");
        let native_request_id: Option<String> = approval_row
            .try_get("native_request_id")
            .expect("native request id");
        let native_json: serde_json::Value = approval_row.try_get("native_json").expect("native");
        assert_eq!(universal_status, "pending");
        assert_eq!(
            native_request_id.as_deref(),
            Some("canonical-native-approval-1")
        );
        assert!(!native_json.to_string().contains("provider_payload"));

        let resolved = resolve_approval(
            &pool,
            approval_id,
            ApprovalDecision::Accept,
            Some(user.user_id),
        )
        .await
        .expect("resolve approval")
        .expect("approval resolved");
        assert_eq!(resolved.universal_status, UniversalApprovalStatus::Approved);

        let legacy_cache = list_event_cache(&pool, session.session_id)
            .await
            .expect("list legacy cache");
        assert_eq!(legacy_cache.len(), 1);
        assert_eq!(legacy_cache[0].event_type, "user_message");
        assert_eq!(legacy_cache[0].event_id, second.event.event_id);

        clear_session_event_projection(&pool, session.session_id)
            .await
            .expect("clear compatibility projection");
        assert!(
            list_universal_events_after(&pool, session.session_id, None, 10)
                .await
                .expect("list cleared universal events")
                .is_empty()
        );
        assert!(load_session_snapshot(&pool, session.session_id)
            .await
            .expect("load cleared snapshot")
            .is_none());
        assert!(list_event_cache(&pool, session.session_id)
            .await
            .expect("list cleared legacy cache")
            .is_empty());
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn upserts_codex_imports_and_replays_cached_events() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let user = create_user(
            &pool,
            &format!("codex-import-user-{suffix}@example.test"),
            Some("Codex Import User"),
        )
        .await
        .expect("create user");
        let runner_id = agenter_core::RunnerId::from_uuid(uuid::Uuid::new_v4());
        let workspace_id = agenter_core::WorkspaceId::from_uuid(uuid::Uuid::new_v4());
        let runner = upsert_runner_with_id(
            &pool,
            runner_id,
            &format!("codex-runner-{suffix}"),
            Some("test"),
        )
        .await
        .expect("upsert runner");
        let workspace = upsert_workspace_with_id(
            &pool,
            workspace_id,
            runner.runner_id,
            &format!("/tmp/agenter-codex-import-{suffix}"),
            Some("Codex Import Workspace"),
        )
        .await
        .expect("upsert workspace");

        let imported = upsert_session_by_external_id(
            &pool,
            UpsertSessionByExternalId {
                owner_user_id: user.user_id,
                runner_id: runner.runner_id,
                workspace_id: workspace.workspace_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                external_session_id: "codex-thread-imported",
                title: Some("Imported Codex Thread"),
                updated_at: None,
            },
        )
        .await
        .expect("upsert imported session");
        let duplicate = upsert_session_by_external_id(
            &pool,
            UpsertSessionByExternalId {
                owner_user_id: user.user_id,
                runner_id: runner.runner_id,
                workspace_id: workspace.workspace_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                external_session_id: "codex-thread-imported",
                title: Some("Renamed Codex Thread"),
                updated_at: None,
            },
        )
        .await
        .expect("upsert duplicate imported session");

        assert_eq!(imported.session_id, duplicate.session_id);
        assert_eq!(duplicate.title.as_deref(), Some("Renamed Codex Thread"));

        let source_updated_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("parse fixed timestamp")
            .with_timezone(&Utc);
        let with_timestamp = upsert_session_by_external_id(
            &pool,
            UpsertSessionByExternalId {
                owner_user_id: user.user_id,
                runner_id: runner.runner_id,
                workspace_id: workspace.workspace_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                external_session_id: "codex-thread-imported",
                title: Some("Timestamped Codex Thread"),
                updated_at: Some(source_updated_at),
            },
        )
        .await
        .expect("upsert imported session with timestamp");
        assert_eq!(with_timestamp.updated_at, source_updated_at);

        let preserved = upsert_session_by_external_id(
            &pool,
            UpsertSessionByExternalId {
                owner_user_id: user.user_id,
                runner_id: runner.runner_id,
                workspace_id: workspace.workspace_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                external_session_id: "codex-thread-imported",
                title: Some("Preserved Timestamp"),
                updated_at: None,
            },
        )
        .await
        .expect("upsert imported session without timestamp");
        assert_eq!(preserved.updated_at, source_updated_at);

        append_event_cache(
            &pool,
            imported.session_id,
            &AppEvent::UserMessage(UserMessageEvent {
                session_id: imported.session_id,
                message_id: Some("user-message-1".to_owned()),
                author_user_id: Some(user.user_id),
                content: "persist me".to_owned(),
            }),
        )
        .await
        .expect("append cached event");

        let sessions = list_sessions_for_user(&pool, user.user_id)
            .await
            .expect("list sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session.session_id, imported.session_id);
        assert_eq!(sessions[0].workspace.workspace_id, workspace.workspace_id);

        let events = list_event_cache(&pool, imported.session_id)
            .await
            .expect("list cached events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "user_message");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn persists_and_finds_password_credentials_by_email() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let email = format!("password-user-{suffix}@example.test");
        let user =
            create_user_with_password_credential(&pool, &email, Some("Password User"), "hash-1")
                .await
                .expect("create password user");

        let (found_user, password_hash) = find_password_credential_by_email(&pool, &email)
            .await
            .expect("find password credential")
            .expect("credential exists");
        assert_eq!(found_user.user_id, user.user_id);
        assert_eq!(password_hash, "hash-1");

        update_password_credential(&pool, user.user_id, &email, "hash-2")
            .await
            .expect("update password credential");

        let (_, updated_hash) = find_password_credential_by_email(&pool, &email)
            .await
            .expect("find updated password credential")
            .expect("credential still exists");
        assert_eq!(updated_hash, "hash-2");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn persists_browser_auth_sessions_until_expiry_or_revoke() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let user = create_user(
            &pool,
            &format!("browser-session-user-{suffix}@example.test"),
            Some("Browser Session User"),
        )
        .await
        .expect("create user");
        let token_hash = format!("browser-session-token-hash-{suffix}");
        let expires_at = Utc::now() + chrono::Duration::days(30);

        let session = create_browser_auth_session(&pool, &token_hash, user.user_id, expires_at)
            .await
            .expect("create browser auth session");
        assert_eq!(session.session_token_hash, token_hash);
        assert_eq!(session.user_id, user.user_id);
        assert!(session.revoked_at.is_none());

        let found = find_browser_auth_session_user(&pool, &token_hash, Utc::now())
            .await
            .expect("find active browser auth session")
            .expect("session should authenticate");
        assert_eq!(found.user_id, user.user_id);

        let expired = find_browser_auth_session_user(
            &pool,
            &token_hash,
            expires_at + chrono::Duration::seconds(1),
        )
        .await
        .expect("find expired browser auth session");
        assert!(expired.is_none());

        revoke_browser_auth_session(&pool, &token_hash)
            .await
            .expect("revoke browser auth session");
        let revoked = find_browser_auth_session_user(&pool, &token_hash, Utc::now())
            .await
            .expect("find revoked browser auth session");
        assert!(revoked.is_none());
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn creates_and_resolves_approval_once() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let user = create_user(
            &pool,
            &format!("approval-user-{suffix}@example.test"),
            Some("Approval User"),
        )
        .await
        .expect("create user");
        let runner = register_runner(&pool, &format!("approval-runner-{suffix}"), None)
            .await
            .expect("register runner");
        let workspace = upsert_workspace(
            &pool,
            runner.runner_id,
            &format!("/tmp/agenter-approval-test-{suffix}"),
            None,
        )
        .await
        .expect("upsert workspace");
        let session = create_session(
            &pool,
            user.user_id,
            runner.runner_id,
            workspace.workspace_id,
            AgentProviderId::from(AgentProviderId::QWEN),
            None,
            None,
        )
        .await
        .expect("create session");

        let approval = create_approval(
            &pool,
            session.session_id,
            ApprovalKind::Command,
            "Run cargo test",
            Some("cargo test -p agenter-db"),
            None,
            Some(serde_json::json!({ "native_id": "approval-1" })),
        )
        .await
        .expect("create approval");

        assert!(approval.resolved_at.is_none());

        let resolved = resolve_approval(
            &pool,
            approval.approval_id,
            ApprovalDecision::Accept,
            Some(user.user_id),
        )
        .await
        .expect("resolve approval")
        .expect("approval should resolve");

        assert_eq!(resolved.resolved_by_user_id, Some(user.user_id));
        assert_eq!(resolved.resolved_decision, Some(ApprovalDecision::Accept));
        assert!(resolved.resolved_at.is_some());

        let second_resolution = resolve_approval(
            &pool,
            approval.approval_id,
            ApprovalDecision::Decline,
            Some(user.user_id),
        )
        .await
        .expect("second resolution should not fail");

        assert!(second_resolution.is_none());
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn consumes_oidc_state_once_and_upserts_identity() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let provider_id = format!("authentik-{suffix}");
        let scopes = vec![
            "openid".to_owned(),
            "profile".to_owned(),
            "email".to_owned(),
        ];
        let provider = upsert_oidc_provider(
            &pool,
            UpsertOidcProvider {
                provider_id: &provider_id,
                display_name: "Authentik",
                issuer_url: "https://auth.example.test/application/o/agenter/",
                client_id: "agenter",
                client_secret_ciphertext: None,
                scopes: &scopes,
                enabled: true,
            },
        )
        .await
        .expect("upsert oidc provider");
        assert_eq!(provider.provider_id, provider_id);

        let state = format!("state-{suffix}");
        let expires_at = Utc::now() + chrono::Duration::minutes(5);
        create_oidc_login_state(
            &pool,
            &state,
            &provider.provider_id,
            "nonce-1",
            Some("pkce-1"),
            Some("/sessions"),
            expires_at,
        )
        .await
        .expect("create oidc state");

        let wrong_provider = consume_oidc_login_state(&pool, "wrong-provider", &state, Utc::now())
            .await
            .expect("wrong provider should not burn state");
        assert!(wrong_provider.is_none());

        let consumed = consume_oidc_login_state(&pool, &provider.provider_id, &state, Utc::now())
            .await
            .expect("consume oidc state")
            .expect("state should be consumable");
        assert_eq!(consumed.nonce, "nonce-1");
        assert!(consumed.consumed_at.is_some());

        let second = consume_oidc_login_state(&pool, &provider.provider_id, &state, Utc::now())
            .await
            .expect("consume state again");
        assert!(second.is_none());

        let email = format!("oidc-user-{suffix}@example.test");
        let user = upsert_oidc_identity(
            &pool,
            &provider.provider_id,
            "subject-1",
            &email,
            Some("Oidc User"),
        )
        .await
        .expect("upsert oidc identity");
        let same_user = upsert_oidc_identity(
            &pool,
            &provider.provider_id,
            "subject-1",
            &email,
            Some("Oidc User"),
        )
        .await
        .expect("upsert same oidc identity");
        assert_eq!(same_user.user_id, user.user_id);
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn consumes_connector_link_code_once() {
        let pool = test_pool().await;

        let suffix = uuid::Uuid::new_v4();
        let user = create_user(
            &pool,
            &format!("link-user-{suffix}@example.test"),
            Some("Link User"),
        )
        .await
        .expect("create link user");
        let code = format!("link-{suffix}");
        let expires_at = Utc::now() + chrono::Duration::minutes(5);
        create_connector_link_code(
            &pool,
            &code,
            "telegram",
            &format!("telegram-{suffix}"),
            Some("Telegram User"),
            expires_at,
        )
        .await
        .expect("create link code");

        let account = consume_connector_link_code(&pool, &code, user.user_id, Utc::now())
            .await
            .expect("consume link code")
            .expect("code should be consumable");
        assert_eq!(account.user_id, user.user_id);
        assert_eq!(account.connector_id, "telegram");

        let second = consume_connector_link_code(&pool, &code, user.user_id, Utc::now())
            .await
            .expect("consume link code again");
        assert_eq!(
            second
                .expect("same user retry returns linked account")
                .connector_account_id,
            account.connector_account_id
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn migration_creates_required_tables() {
        let pool = test_pool().await;

        for table_name in [
            "users",
            "auth_identities",
            "password_credentials",
            "oidc_providers",
            "runners",
            "runner_tokens",
            "workspaces",
            "event_cache_cursors",
            "agent_sessions",
            "connector_accounts",
            "connector_link_codes",
            "session_bindings",
            "oidc_login_states",
            "browser_auth_sessions",
            "pending_approvals",
            "event_cache",
            "agent_events",
            "agent_event_idempotency",
            "session_snapshots",
            "recent_turn_caches",
            "connector_deliveries",
        ] {
            let exists: bool = sqlx::query_scalar(
                "select exists (
                    select 1
                    from information_schema.tables
                    where table_schema = 'public' and table_name = $1
                )",
            )
            .bind(table_name)
            .fetch_one(&pool)
            .await
            .expect("check table existence");
            assert!(exists, "missing table {table_name}");
        }

        pool.execute("select 1")
            .await
            .expect("database remains usable");
    }
}

use agenter_core::{
    AgentProviderId, AgentTurnSettings, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
    RunnerId, SessionId, SessionStatus, UserId, WorkspaceId,
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgRow, PgPool, Result, Row};
use uuid::Uuid;

use crate::models::{
    AgentSession, AgentSessionWithWorkspace, CachedEvent, ConnectorAccount, ConnectorLinkCode,
    OidcLoginState, OidcProvider, PendingApproval, Runner, User, Workspace,
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
             external_session_id, status, title, turn_settings, created_at, updated_at",
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
            turn_settings
         )
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, turn_settings, created_at, updated_at",
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
            .turn_settings
            .map(|settings| serde_json::to_value(settings).expect("serialize turn settings")),
    )
    .fetch_one(pool)
    .await?;

    session_from_row(&row)
}

pub async fn upsert_session_by_external_id(
    pool: &PgPool,
    owner_user_id: UserId,
    runner_id: RunnerId,
    workspace_id: WorkspaceId,
    provider_id: AgentProviderId,
    external_session_id: &str,
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
         on conflict (runner_id, workspace_id, provider_id, external_session_id)
         where external_session_id is not null
         do update set title = coalesce(excluded.title, agent_sessions.title),
                       status = case
                           when agent_sessions.status = 'archived' then agent_sessions.status
                           else excluded.status
                       end,
                       updated_at = now()
         returning session_id, owner_user_id, runner_id, workspace_id, provider_id,
             external_session_id, status, title, turn_settings, created_at, updated_at",
    )
    .bind(owner_user_id.as_uuid())
    .bind(runner_id.as_uuid())
    .bind(workspace_id.as_uuid())
    .bind(provider_id.as_str())
    .bind(external_session_id)
    .bind(session_status_to_db(&SessionStatus::Running))
    .bind(title)
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
             external_session_id, status, title, turn_settings, created_at, updated_at",
    )
    .bind(owner_user_id.as_uuid())
    .bind(session_id.as_uuid())
    .bind(settings)
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
    let decision = serde_json::to_value(decision).map_err(sqlx::Error::decode)?;
    let resolved_by_user_id = resolved_by_user_id.map(UserId::as_uuid);
    let row = sqlx::query(
        "update pending_approvals
         set resolved_decision = $2,
             resolved_by_user_id = $3,
             resolved_at = now(),
             updated_at = now()
         where approval_id = $1 and resolved_at is null
         returning approval_id, session_id, kind, title, details, provider_payload,
             expires_at, resolved_decision, resolved_by_user_id, resolved_at, created_at, updated_at",
    )
    .bind(approval_id.as_uuid())
    .bind(decision)
    .bind(resolved_by_user_id)
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

fn session_with_workspace_from_row(row: &PgRow) -> Result<AgentSessionWithWorkspace> {
    let status: String = row.try_get("status")?;
    let provider_id: String = row.try_get("provider_id")?;
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
    let resolved_decision: Option<serde_json::Value> = row.try_get("resolved_decision")?;
    let resolved_by_user_id: Option<Uuid> = row.try_get("resolved_by_user_id")?;
    Ok(PendingApproval {
        approval_id: ApprovalId::from_uuid(row.try_get("approval_id")?),
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        kind: approval_kind_from_db(&kind)?,
        title: row.try_get("title")?,
        details: row.try_get("details")?,
        provider_payload: row.try_get("provider_payload")?,
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

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentProviderId, AppEvent, ApprovalDecision, ApprovalKind, SessionStatus, UserMessageEvent,
    };
    use sqlx::{Executor, PgPool};

    use super::*;

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
            user.user_id,
            runner.runner_id,
            workspace.workspace_id,
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-thread-imported",
            Some("Imported Codex Thread"),
        )
        .await
        .expect("upsert imported session");
        let duplicate = upsert_session_by_external_id(
            &pool,
            user.user_id,
            runner.runner_id,
            workspace.workspace_id,
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-thread-imported",
            Some("Renamed Codex Thread"),
        )
        .await
        .expect("upsert duplicate imported session");

        assert_eq!(imported.session_id, duplicate.session_id);
        assert_eq!(duplicate.title.as_deref(), Some("Renamed Codex Thread"));

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
            "pending_approvals",
            "event_cache",
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

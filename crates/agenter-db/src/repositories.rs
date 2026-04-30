use agenter_core::{
    AgentProviderId, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind, RunnerId, SessionId,
    SessionStatus, UserId, WorkspaceId,
};
use chrono::{DateTime, Utc};
use sqlx::{postgres::PgRow, PgPool, Result, Row};
use uuid::Uuid;

use crate::models::{AgentSession, CachedEvent, PendingApproval, Runner, User, Workspace};

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
             external_session_id, status, title, created_at, updated_at",
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
    Ok(AgentSession {
        session_id: SessionId::from_uuid(row.try_get("session_id")?),
        owner_user_id: UserId::from_uuid(row.try_get("owner_user_id")?),
        runner_id: RunnerId::from_uuid(row.try_get("runner_id")?),
        workspace_id: WorkspaceId::from_uuid(row.try_get("workspace_id")?),
        provider_id: AgentProviderId::from(provider_id),
        external_session_id: row.try_get("external_session_id")?,
        status: session_status_from_db(&status)?,
        title: row.try_get("title")?,
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
            "session_bindings",
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

create extension if not exists pgcrypto;

create table users (
    user_id uuid primary key default gen_random_uuid(),
    email text not null unique,
    display_name text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table auth_identities (
    auth_identity_id uuid primary key default gen_random_uuid(),
    user_id uuid not null references users(user_id) on delete cascade,
    provider_kind text not null check (provider_kind in ('password', 'oidc')),
    provider_id text not null,
    subject text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (provider_kind, provider_id, subject)
);

create table password_credentials (
    user_id uuid primary key references users(user_id) on delete cascade,
    password_hash text not null,
    password_updated_at timestamptz not null default now(),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table oidc_providers (
    oidc_provider_id text primary key,
    display_name text not null,
    issuer_url text not null,
    client_id text not null,
    client_secret_ciphertext text,
    scopes text[] not null default array['openid', 'profile', 'email'],
    enabled boolean not null default true,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table runners (
    runner_id uuid primary key default gen_random_uuid(),
    name text not null,
    version text,
    last_seen_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table runner_tokens (
    runner_token_id uuid primary key default gen_random_uuid(),
    runner_id uuid not null references runners(runner_id) on delete cascade,
    token_hash text not null unique,
    label text,
    expires_at timestamptz,
    revoked_at timestamptz,
    last_used_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table workspaces (
    workspace_id uuid primary key default gen_random_uuid(),
    runner_id uuid not null references runners(runner_id) on delete cascade,
    path text not null,
    display_name text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (runner_id, path)
);

create table agent_sessions (
    session_id uuid primary key default gen_random_uuid(),
    owner_user_id uuid not null references users(user_id) on delete restrict,
    runner_id uuid not null references runners(runner_id) on delete restrict,
    workspace_id uuid not null references workspaces(workspace_id) on delete restrict,
    provider_id text not null,
    external_session_id text,
    status text not null check (
        status in (
            'starting',
            'running',
            'waiting_for_input',
            'waiting_for_approval',
            'completed',
            'interrupted',
            'degraded',
            'failed',
            'archived'
        )
    ),
    title text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (provider_id, external_session_id)
);

create table connector_accounts (
    connector_account_id uuid primary key default gen_random_uuid(),
    user_id uuid not null references users(user_id) on delete cascade,
    connector_id text not null,
    external_account_id text not null,
    display_name text,
    linked_at timestamptz not null default now(),
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (connector_id, external_account_id)
);

create table session_bindings (
    connector_binding_id uuid primary key default gen_random_uuid(),
    session_id uuid not null references agent_sessions(session_id) on delete cascade,
    connector_account_id uuid references connector_accounts(connector_account_id) on delete cascade,
    connector_id text not null,
    external_chat_id text not null,
    external_thread_id text,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (connector_id, external_chat_id, external_thread_id)
);

create table pending_approvals (
    approval_id uuid primary key default gen_random_uuid(),
    session_id uuid not null references agent_sessions(session_id) on delete cascade,
    kind text not null check (kind in ('command', 'file_change', 'tool', 'provider_specific')),
    title text not null,
    details text,
    provider_payload jsonb,
    expires_at timestamptz,
    resolved_decision jsonb,
    resolved_by_user_id uuid references users(user_id) on delete set null,
    resolved_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table event_cache (
    event_id uuid primary key default gen_random_uuid(),
    session_id uuid not null references agent_sessions(session_id) on delete cascade,
    event_index bigint not null,
    event_type text not null,
    payload jsonb not null,
    created_at timestamptz not null default now(),
    unique (session_id, event_index)
);

create table connector_deliveries (
    connector_delivery_id uuid primary key default gen_random_uuid(),
    connector_binding_id uuid references session_bindings(connector_binding_id) on delete cascade,
    event_id uuid references event_cache(event_id) on delete cascade,
    connector_id text not null,
    idempotency_key text not null,
    external_message_id text,
    status text not null check (status in ('pending', 'delivered', 'failed', 'abandoned')),
    attempt_count integer not null default 0,
    next_attempt_at timestamptz,
    last_error text,
    delivered_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    unique (connector_id, idempotency_key)
);

create index idx_auth_identities_user_id on auth_identities(user_id);
create index idx_runner_tokens_runner_id on runner_tokens(runner_id);
create index idx_workspaces_runner_id on workspaces(runner_id);
create index idx_agent_sessions_owner_user_id on agent_sessions(owner_user_id);
create index idx_agent_sessions_runner_workspace on agent_sessions(runner_id, workspace_id);
create index idx_connector_accounts_user_id on connector_accounts(user_id);
create index idx_session_bindings_session_id on session_bindings(session_id);
create index idx_pending_approvals_session_id on pending_approvals(session_id);
create index idx_pending_approvals_unresolved on pending_approvals(session_id, created_at)
    where resolved_at is null;
create index idx_event_cache_session_index on event_cache(session_id, event_index);
create index idx_connector_deliveries_pending on connector_deliveries(status, next_attempt_at)
    where status in ('pending', 'failed');

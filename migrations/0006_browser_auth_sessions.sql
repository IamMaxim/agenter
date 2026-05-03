create table browser_auth_sessions (
    session_token_hash text primary key,
    user_id uuid not null references users(user_id) on delete cascade,
    expires_at timestamptz not null,
    revoked_at timestamptz,
    created_at timestamptz not null default now(),
    last_seen_at timestamptz not null default now()
);

create index idx_browser_auth_sessions_user_active
    on browser_auth_sessions(user_id, expires_at)
    where revoked_at is null;

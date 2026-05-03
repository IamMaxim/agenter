create table agent_events (
    seq bigserial primary key,
    event_id uuid not null unique,
    workspace_id uuid not null references workspaces(workspace_id) on delete restrict,
    session_id uuid not null references agent_sessions(session_id) on delete cascade,
    turn_id uuid,
    item_id uuid,
    event_type text not null,
    event_json jsonb not null,
    native_json jsonb,
    source text not null check (
        source in ('control_plane', 'runner', 'browser', 'connector', 'native')
    ),
    command_id uuid,
    created_at timestamptz not null default now()
);

create table agent_event_idempotency (
    idempotency_key text primary key,
    command_id uuid not null unique,
    session_id uuid references agent_sessions(session_id) on delete cascade,
    status text not null check (status in ('pending', 'succeeded', 'failed', 'conflict')),
    response_json jsonb,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table session_snapshots (
    session_id uuid primary key references agent_sessions(session_id) on delete cascade,
    latest_seq bigint not null check (latest_seq >= 0),
    snapshot_json jsonb not null,
    updated_at timestamptz not null default now()
);

create table recent_turn_caches (
    session_id uuid not null references agent_sessions(session_id) on delete cascade,
    turn_id uuid not null,
    cache_json jsonb not null,
    updated_at timestamptz not null default now(),
    primary key (session_id, turn_id)
);

alter table pending_approvals
    add column universal_status text not null default 'pending',
    add column native_request_id text,
    add column canonical_options jsonb not null default '[]'::jsonb,
    add column risk text,
    add column subject text,
    add column native_summary text,
    add column native_json jsonb;

update pending_approvals
set universal_status = case
    when resolved_at is null then 'pending'
    when resolved_decision ->> 'decision' in ('accept', 'accept_for_session') then 'approved'
    when resolved_decision ->> 'decision' = 'cancel' then 'cancelled'
    else 'denied'
end;

alter table pending_approvals
    add constraint pending_approvals_universal_status_check check (
        universal_status in (
            'pending',
            'presented',
            'resolving',
            'approved',
            'denied',
            'cancelled',
            'expired',
            'orphaned'
        )
    );

create index idx_agent_events_session_seq on agent_events(session_id, seq);
create index idx_agent_events_workspace_seq on agent_events(workspace_id, seq);
create index idx_agent_events_turn on agent_events(session_id, turn_id, seq)
    where turn_id is not null;
create index idx_agent_event_idempotency_session on agent_event_idempotency(session_id, created_at)
    where session_id is not null;
create index idx_recent_turn_caches_session_updated on recent_turn_caches(session_id, updated_at);
create index idx_pending_approvals_native_request_id on pending_approvals(session_id, native_request_id)
    where native_request_id is not null;

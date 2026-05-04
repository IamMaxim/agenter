create table approval_policy_rules (
    rule_id uuid primary key default gen_random_uuid(),
    owner_user_id uuid not null references users(user_id) on delete cascade,
    workspace_id uuid not null references workspaces(workspace_id) on delete cascade,
    provider_id text not null,
    kind text not null check (kind in ('command', 'file_change', 'tool', 'provider_specific')),
    label text not null,
    matcher jsonb not null,
    decision jsonb not null,
    source_approval_id uuid references pending_approvals(approval_id) on delete set null,
    created_by_user_id uuid references users(user_id) on delete set null,
    disabled_by_user_id uuid references users(user_id) on delete set null,
    disabled_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index idx_approval_policy_rules_scope
    on approval_policy_rules(owner_user_id, workspace_id, provider_id, kind, created_at)
    where disabled_at is null;

create unique index idx_approval_policy_rules_active_unique
    on approval_policy_rules(owner_user_id, workspace_id, provider_id, kind, matcher)
    where disabled_at is null;

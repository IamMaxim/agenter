alter table pending_approvals
    drop constraint if exists pending_approvals_universal_status_check;

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
            'orphaned',
            'detached'
        )
    );

create table agent_obligations (
    obligation_id text primary key,
    session_id uuid not null references agent_sessions(session_id) on delete cascade,
    turn_id uuid,
    runner_id uuid references runners(runner_id) on delete set null,
    native_request_id text,
    kind text not null check (kind in ('approval', 'question')),
    approval_id uuid,
    question_id uuid,
    status text not null check (
        status in (
            'pending',
            'presented',
            'resolving',
            'delivered_to_runner',
            'accepted_by_native',
            'resolved',
            'orphaned',
            'expired',
            'detached'
        )
    ),
    delivery_generation bigint not null default 0 check (delivery_generation >= 0),
    resolution_command_id uuid,
    payload_json jsonb,
    resolved_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now(),
    check (
        (kind = 'approval' and approval_id is not null and question_id is null)
        or
        (kind = 'question' and question_id is not null and approval_id is null)
    )
);

create unique index idx_agent_obligations_approval
    on agent_obligations(approval_id)
    where approval_id is not null;

create unique index idx_agent_obligations_question
    on agent_obligations(question_id)
    where question_id is not null;

create index idx_agent_obligations_session_status
    on agent_obligations(session_id, status, updated_at);

create index idx_agent_obligations_native_request
    on agent_obligations(session_id, native_request_id)
    where native_request_id is not null;

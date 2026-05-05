create table runner_event_receipts (
    runner_id uuid not null references runners(runner_id) on delete cascade,
    runner_event_seq bigint not null check (runner_event_seq > 0),
    event_seq bigint not null check (event_seq >= 0),
    event_id uuid not null,
    accepted_at timestamptz not null default now(),
    primary key (runner_id, runner_event_seq),
    unique (event_seq),
    unique (event_id)
);

create index idx_runner_event_receipts_runner_seq on runner_event_receipts(runner_id, runner_event_seq);

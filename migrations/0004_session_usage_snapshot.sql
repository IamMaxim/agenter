alter table agent_sessions
    add column if not exists usage_snapshot jsonb;


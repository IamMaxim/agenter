alter table agent_sessions
    drop constraint agent_sessions_status_check;

alter table agent_sessions
    add constraint agent_sessions_status_check check (
        status in (
            'starting',
            'running',
            'waiting_for_input',
            'waiting_for_approval',
            'idle',
            'stopped',
            'completed',
            'interrupted',
            'degraded',
            'failed',
            'archived'
        )
    );

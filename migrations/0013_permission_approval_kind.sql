alter table pending_approvals
    drop constraint pending_approvals_kind_check,
    add constraint pending_approvals_kind_check
        check (kind in ('command', 'file_change', 'permission', 'tool', 'provider_specific'));

alter table approval_policy_rules
    drop constraint approval_policy_rules_kind_check,
    add constraint approval_policy_rules_kind_check
        check (kind in ('command', 'file_change', 'permission', 'tool', 'provider_specific'));

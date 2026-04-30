create table oidc_login_states (
    state text primary key,
    provider_id text not null references oidc_providers(oidc_provider_id) on delete cascade,
    nonce text not null,
    pkce_verifier text,
    return_to text,
    expires_at timestamptz not null,
    consumed_at timestamptz,
    created_at timestamptz not null default now()
);

create index idx_oidc_login_states_unconsumed
    on oidc_login_states(provider_id, expires_at)
    where consumed_at is null;

create table connector_link_codes (
    code text primary key,
    user_id uuid references users(user_id) on delete cascade,
    connector_id text not null,
    external_account_id text not null,
    display_name text,
    expires_at timestamptz not null,
    consumed_at timestamptz,
    created_at timestamptz not null default now(),
    unique (connector_id, external_account_id, consumed_at)
);

create index idx_connector_link_codes_unconsumed
    on connector_link_codes(connector_id, external_account_id, expires_at)
    where consumed_at is null;

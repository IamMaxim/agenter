use crate::state::AppState;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};

pub struct AuthRepository<'a> {
    state: &'a AppState,
}

impl<'a> AuthRepository<'a> {
    pub const fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    fn db_pool(&self) -> Option<&sqlx::PgPool> {
        self.state.db_pool()
    }

    pub async fn find_oidc_provider(
        &self,
        provider_id: &str,
    ) -> Result<Option<agenter_db::models::OidcProvider>> {
        let Some(pool) = self.db_pool() else {
            return Ok(None);
        };
        agenter_db::find_oidc_provider(pool, provider_id)
            .await
            .map_err(|error| anyhow!(error))
    }

    pub async fn create_oidc_login_state(
        &self,
        state: &str,
        provider_id: &str,
        nonce: &str,
        pkce_verifier: Option<&str>,
        return_to: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<agenter_db::models::OidcLoginState> {
        let pool = self
            .db_pool()
            .ok_or_else(|| anyhow!("database not configured"))?;
        agenter_db::create_oidc_login_state(
            pool,
            state,
            provider_id,
            nonce,
            pkce_verifier,
            return_to,
            expires_at,
        )
        .await
        .map_err(anyhow::Error::from)
    }

    pub async fn consume_oidc_login_state(
        &self,
        provider_id: &str,
        state: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<agenter_db::models::OidcLoginState>> {
        let pool = self
            .db_pool()
            .ok_or_else(|| anyhow!("database not configured"))?;
        agenter_db::consume_oidc_login_state(pool, provider_id, state, now)
            .await
            .map_err(anyhow::Error::from)
    }

    pub async fn upsert_oidc_identity(
        &self,
        provider_id: &str,
        subject: &str,
        email: &str,
        display_name: Option<&str>,
    ) -> Result<agenter_db::models::User> {
        let pool = self
            .db_pool()
            .ok_or_else(|| anyhow!("database not configured"))?;
        agenter_db::upsert_oidc_identity(pool, provider_id, subject, email, display_name)
            .await
            .map_err(anyhow::Error::from)
    }

    pub async fn create_connector_link_code(
        &self,
        code: &str,
        connector_id: &str,
        external_account_id: &str,
        display_name: Option<&str>,
        expires_at: DateTime<Utc>,
    ) -> Result<agenter_db::models::ConnectorLinkCode> {
        let pool = self
            .db_pool()
            .ok_or_else(|| anyhow!("database not configured"))?;
        agenter_db::create_connector_link_code(
            pool,
            code,
            connector_id,
            external_account_id,
            display_name,
            expires_at,
        )
        .await
        .map_err(anyhow::Error::from)
    }

    pub async fn consume_connector_link_code(
        &self,
        code: &str,
        user_id: agenter_core::UserId,
        now: DateTime<Utc>,
    ) -> Result<Option<agenter_db::models::ConnectorAccount>> {
        let pool = self
            .db_pool()
            .ok_or_else(|| anyhow!("database not configured"))?;
        agenter_db::consume_connector_link_code(pool, code, user_id, now)
            .await
            .map_err(anyhow::Error::from)
    }
}

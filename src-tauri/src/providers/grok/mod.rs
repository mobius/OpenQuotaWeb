mod auth;
mod client;
mod local_usage;
mod mapper;

use std::sync::Arc;

use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use thiserror::Error;

use crate::{
    hashing::sha256_hex,
    models::{
        MetricDefinition, MetricSection, ProviderDefinition, ProviderErrorKind, ProviderLink,
        ProviderSnapshot, UsagePeriodSelection,
    },
    pricing::PricingStore,
    providers::log_usage::scan_or_cached_usage,
    storage::Storage,
};

use self::{
    auth::{GrokAuthState, GrokAuthStore},
    client::GrokClient,
    local_usage::GrokLogUsageScanner,
    mapper::{map_credits, plan_name},
};

use super::{ProviderError, UsageProvider};

pub(crate) fn definition() -> ProviderDefinition {
    definition_for("grok", "Grok", false)
}

fn definition_for(id: &str, display_name: &str, fallback_enabled: bool) -> ProviderDefinition {
    let mut definition = ProviderDefinition {
        id: id.into(),
        display_name: display_name.into(),
        short_name: "G".into(),
        fallback_enabled,
        local_usage_source_note: Some("From your Grok logs (estimated)".into()),
        links: vec![ProviderLink::new("Usage", "https://grok.com/?_s=usage")],
        metrics: vec![
            MetricDefinition::quota(
                "grok.weekly",
                "Weekly",
                "weekly",
                false,
                true,
                MetricSection::AlwaysVisible,
                false,
                "W",
            ),
            MetricDefinition::status(
                "grok.payAsYouGo",
                "Extra Usage",
                "payAsYouGo",
                true,
                MetricSection::OnDemand,
                false,
                "E",
            ),
            MetricDefinition::trend("grok.trend"),
            MetricDefinition::usage(
                "grok.today",
                "Today",
                UsagePeriodSelection::Today,
                MetricSection::OnDemand,
                "T",
            ),
            MetricDefinition::usage(
                "grok.yesterday",
                "Yesterday",
                UsagePeriodSelection::Yesterday,
                MetricSection::OnDemand,
                "Y",
            ),
            MetricDefinition::usage(
                "grok.last30",
                "Last 30 Days",
                UsagePeriodSelection::Last30Days,
                MetricSection::OnDemand,
                "M",
            ),
        ],
    };
    if id != "grok" {
        for metric in &mut definition.metrics {
            if let Some(suffix) = metric.id.strip_prefix("grok.") {
                metric.id = format!("{id}.{suffix}");
            }
            metric.default_pinned = false;
        }
    }
    definition
}

#[derive(Debug, Error)]
pub(crate) enum GrokError {
    #[error("Grok is not logged in. Run `grok login`.")]
    NotLoggedIn,
    #[error("Grok login data is invalid. Run `grok login` again.")]
    InvalidAuth,
    #[error("Grok login expired. Run `grok login` again.")]
    Expired,
    #[error("Refreshed Grok credentials could not be saved.")]
    AuthWrite,
    #[error("Could not reach Grok. Check your internet connection.")]
    ConnectionFailed,
    #[error("Grok returned an invalid billing response.")]
    InvalidResponse,
    #[error("Grok billing request failed (HTTP {0}).")]
    RequestFailed(u16),
    #[error("Local Grok usage logs could not be processed.")]
    LocalUsage,
    #[error("OpenQuota cache is unavailable.")]
    Storage,
}

impl From<crate::storage::StorageError> for GrokError {
    fn from(_: crate::storage::StorageError) -> Self {
        Self::Storage
    }
}

pub struct GrokProvider {
    definition: ProviderDefinition,
    account_identity: Option<String>,
    storage: Arc<Storage>,
    pricing: Arc<PricingStore>,
    auth: GrokAuthStore,
    client: GrokClient,
    log_usage: GrokLogUsageScanner,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl GrokProvider {
    pub(crate) fn new(
        storage: Arc<Storage>,
        pricing: Arc<PricingStore>,
    ) -> Result<Self, GrokError> {
        let account_identity = GrokAuthStore::new()
            .load_candidates()
            .ok()
            .and_then(|states| states.into_iter().next())
            .map(|state| state.account_identity());
        Self::new_scoped(
            storage,
            pricing,
            definition(),
            account_identity.map(|identity| account_identity_key(&identity)),
        )
    }

    fn new_scoped(
        storage: Arc<Storage>,
        pricing: Arc<PricingStore>,
        definition: ProviderDefinition,
        account_identity: Option<String>,
    ) -> Result<Self, GrokError> {
        if definition.id == "grok" {
            if let Some(identity) = account_identity.as_deref() {
                crate::providers::remember_default_account(&storage, "grok", identity)?;
            }
        }
        Ok(Self {
            definition,
            account_identity,
            storage,
            pricing,
            auth: GrokAuthStore::new(),
            client: GrokClient::new()?,
            log_usage: GrokLogUsageScanner::new(),
            now: Arc::new(Utc::now),
        })
    }

    pub(crate) fn runtimes(
        storage: Arc<Storage>,
        pricing: Arc<PricingStore>,
    ) -> Result<Vec<Arc<dyn crate::providers::UsageProvider>>, GrokError> {
        let primary = GrokAuthStore::new()
            .load_candidates()
            .ok()
            .and_then(|states| states.into_iter().next())
            .map(|state| account_identity_key(&state.account_identity()));
        let mut runtimes = vec![Arc::new(Self::new_scoped(
            storage.clone(),
            pricing.clone(),
            definition(),
            primary.clone(),
        )?) as Arc<dyn crate::providers::UsageProvider>];
        for identity in discover_additional_accounts(primary.as_deref())? {
            let provider_id = format!("grok@{}", &identity[..8]);
            runtimes.push(Arc::new(Self::new_scoped(
                storage.clone(),
                pricing.clone(),
                definition_for(
                    &provider_id,
                    &format!("Grok — {}", &provider_id["grok@".len()..]),
                    false,
                ),
                Some(identity),
            )?) as Arc<dyn crate::providers::UsageProvider>);
        }
        Ok(runtimes)
    }

    #[cfg(test)]
    fn with_dependencies(
        storage: Arc<Storage>,
        pricing: Arc<PricingStore>,
        definition: ProviderDefinition,
        account_identity: Option<String>,
        auth: GrokAuthStore,
        client: GrokClient,
        log_usage: GrokLogUsageScanner,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            definition,
            account_identity,
            storage,
            pricing,
            auth,
            client,
            log_usage,
            now: Arc::new(move || now),
        }
    }

    fn refresh_inner(&self) -> Result<ProviderSnapshot, GrokError> {
        let now = (self.now)();
        let candidates = self.auth.load_candidates()?;
        crate::app_debug!(
            "auth:grok",
            "credential candidates loaded ({})",
            candidates.len()
        );
        let mut last_auth_error = None;
        for mut state in candidates {
            let identity = account_identity_key(&state.account_identity());
            if !self.matches_identity(&identity) {
                continue;
            }
            match self.refresh_candidate(&mut state, now, &identity) {
                Ok(snapshot) => return Ok(snapshot),
                Err(error @ (GrokError::Expired | GrokError::InvalidAuth)) => {
                    last_auth_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_auth_error.unwrap_or(GrokError::InvalidAuth))
    }

    fn refresh_candidate(
        &self,
        state: &mut GrokAuthState,
        now: DateTime<Utc>,
        account_identity: &str,
    ) -> Result<ProviderSnapshot, GrokError> {
        let mut warnings = Vec::new();
        if self.auth.needs_refresh(state, now) {
            if let Err(error) = self.refresh_access_token(state, now, &mut warnings) {
                if self.auth.is_expired(state, now) {
                    return Err(GrokError::Expired);
                }
                crate::app_warn!(
                    "auth:grok",
                    "proactive token refresh failed; trying the current access token ({error})"
                );
            }
        }

        let mut credits = self.client.fetch_credits(&state.token)?;
        if matches!(
            credits.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            self.refresh_access_token(state, now, &mut warnings)?;
            credits = self.client.fetch_credits(&state.token)?;
        }
        let mapped = map_credits(&credits)?;
        let plan = self
            .client
            .fetch_settings(&state.token)
            .ok()
            .as_ref()
            .and_then(plan_name);
        let pricing = self.pricing.current();
        let usage = scan_or_cached_usage(
            &self.storage,
            &self.definition.id,
            crate::providers::CacheIdentity::Resolved(account_identity),
            "Grok",
            || self.log_usage.scan(&self.storage, now, &pricing),
            &mut warnings,
        );
        Ok(ProviderSnapshot {
            provider_id: self.definition.id.clone(),
            plan,
            quotas: mapped.quotas,
            value_metrics: Vec::new(),
            status_metrics: mapped.status_metrics,
            notices: Vec::new(),
            usage,
            warnings,
            refreshed_at: now,
        })
    }

    fn matches_identity(&self, identity: &str) -> bool {
        self.account_identity
            .as_deref()
            .map(|expected| expected == identity)
            .unwrap_or(true)
    }

    fn refresh_access_token(
        &self,
        state: &mut GrokAuthState,
        now: DateTime<Utc>,
        warnings: &mut Vec<String>,
    ) -> Result<(), GrokError> {
        let refresh_token = self
            .auth
            .refresh_token(state)
            .ok_or(GrokError::Expired)?
            .to_owned();
        let client_id = self.auth.client_id(state);
        let refreshed = self.client.refresh_token(&refresh_token, &client_id)?;
        self.auth.update_from_refresh(
            state,
            refreshed.access_token,
            refreshed.refresh_token,
            refreshed.id_token,
            refreshed.expires_in,
            now,
        );
        if self.auth.save(state).is_err() {
            crate::app_error!(
                "auth:grok",
                "failed to persist rotated credentials; using them for this session only"
            );
            warnings.push(
                "The refreshed Grok login is active for this session but could not be saved."
                    .into(),
            );
        }
        Ok(())
    }
}

impl UsageProvider for GrokProvider {
    fn definition(&self) -> ProviderDefinition {
        self.definition.clone()
    }

    fn has_local_credentials(&self) -> bool {
        self.auth.has_local_credentials()
    }

    fn cache_identity(&self) -> crate::providers::CacheIdentity<'_> {
        self.account_identity
            .as_deref()
            .map(crate::providers::CacheIdentity::Resolved)
            .unwrap_or(crate::providers::CacheIdentity::Unresolved)
    }

    fn supports_account_names(&self) -> bool {
        true
    }

    fn account_identity(&self) -> Option<&str> {
        self.account_identity.as_deref()
    }

    fn refresh(&self) -> Result<ProviderSnapshot, ProviderError> {
        self.refresh_inner().map_err(|error| {
            let kind = match error {
                GrokError::NotLoggedIn | GrokError::InvalidAuth | GrokError::Expired => {
                    ProviderErrorKind::Authentication
                }
                GrokError::AuthWrite => ProviderErrorKind::CredentialStorage,
                GrokError::RequestFailed(429) => ProviderErrorKind::RateLimited,
                GrokError::RequestFailed(_) | GrokError::ConnectionFailed => {
                    ProviderErrorKind::Network
                }
                GrokError::InvalidResponse => ProviderErrorKind::InvalidResponse,
                GrokError::LocalUsage => ProviderErrorKind::LocalData,
                GrokError::Storage => ProviderErrorKind::Storage,
            };
            ProviderError::from_display(kind, error)
        })
    }

    fn refresh_for_service(
        &self,
    ) -> Result<crate::providers::ProviderRefresh, crate::providers::ProviderError> {
        let snapshot = self.refresh()?;
        Ok(crate::providers::ProviderRefresh {
            snapshot,
            cache_identity: self.account_identity.clone(),
            account: self
                .account_identity
                .clone()
                .map(|identity| crate::providers::AccountRefresh {
                    family: "grok",
                    provider_id: self.definition.id.clone(),
                    identity,
                }),
        })
    }
}

fn account_identity_key(identity: &str) -> String {
    sha256_hex(identity.as_bytes())
}

fn discover_additional_accounts(primary_identity: Option<&str>) -> Result<Vec<String>, GrokError> {
    let identities = GrokAuthStore::new()
        .load_candidates()
        .unwrap_or_default()
        .into_iter()
        .map(|state| account_identity_key(&state.account_identity()))
        .collect::<Vec<_>>();
    let mut identities = identities;
    identities.sort();
    identities.dedup();
    if let Some(primary_identity) = primary_identity {
        identities.retain(|identity| identity != primary_identity);
        return Ok(identities);
    }
    if identities.len() <= 1 {
        return Ok(Vec::new());
    }
    Ok(identities.into_iter().skip(1).collect())
}

#[cfg(test)]
mod tests;

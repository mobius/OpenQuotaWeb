use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    Json,
};
use chrono::{DateTime, Utc};
use futures::{stream, Stream};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use crate::{
    logging,
    models::{
        ApiKeyMutationOutcome, ApiKeyStatus, AppSettings, ProviderApiKeyState, ProviderCatalog,
        SettingsViewState,
    },
    providers::{codex::reset_claim::ResetClaimOutcome, UsageProvider},
    service::UsageViewState,
};

use super::{events, state::AppState};

type ApiError = (StatusCode, String);
type ApiResult<T> = Result<Json<T>, ApiError>;

fn internal(message: impl Into<String>) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, message.into())
}

fn bad_request(message: impl Into<String>) -> ApiError {
    (StatusCode::BAD_REQUEST, message.into())
}

pub fn view_state(state: &AppState) -> SettingsViewState {
    state
        .settings
        .view_state("granted", None, false, Some("Web".to_owned()))
}

fn emit_account_if_changed(state: &AppState, observed: &AtomicU64) {
    let revision = state.settings.account_revision();
    if observed.swap(revision, Ordering::SeqCst) != revision {
        events::emit(&state.events, "settings-state", &view_state(state));
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    usage: UsageViewState,
    settings: SettingsViewState,
    catalog: ProviderCatalog,
}

pub async fn get_bootstrap(State(state): State<Arc<AppState>>) -> Json<BootstrapState> {
    Json(BootstrapState {
        usage: state.service.state(),
        settings: view_state(&state),
        catalog: state.settings.catalog().clone(),
    })
}

/// Refresh every enabled provider, streaming progress to connected browsers.
pub async fn refresh_providers(state: &Arc<AppState>, provider_ids: &[String]) {
    let observed = Arc::new(AtomicU64::new(state.settings.account_revision()));
    let progress_state = state.clone();
    let progress_observed = observed.clone();
    let result = state
        .service
        .refresh_enabled_with_progress(provider_ids, true, move |progress| {
            emit_account_if_changed(&progress_state, &progress_observed);
            events::emit(&progress_state.events, "usage-state", progress);
        })
        .await;
    emit_account_if_changed(state, &observed);
    events::emit(&state.events, "usage-state", &result);
}

pub async fn refresh_usage(State(state): State<Arc<AppState>>) -> Json<UsageViewState> {
    let ids = state.settings.enabled_provider_ids();
    let observed = Arc::new(AtomicU64::new(state.settings.account_revision()));
    let progress_state = state.clone();
    let progress_observed = observed.clone();
    let result = state
        .service
        .refresh_all_with_progress(&ids, true, move |progress| {
            emit_account_if_changed(&progress_state, &progress_observed);
            events::emit(&progress_state.events, "usage-state", progress);
        })
        .await;
    emit_account_if_changed(&state, &observed);
    events::emit(&state.events, "usage-state", &result);
    Json(result)
}

pub async fn refresh_provider(
    Path(provider_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<UsageViewState> {
    if !state.settings.enabled_provider_ids().contains(&provider_id) {
        return Err(bad_request("Provider is not enabled."));
    }
    let observed = AtomicU64::new(state.settings.account_revision());
    state.service.refresh(&provider_id, true).await;
    let result = state.service.state();
    emit_account_if_changed(&state, &observed);
    events::emit(&state.events, "usage-state", &result);
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetClaimBody {
    expires_at: DateTime<Utc>,
    redeem_request_id: String,
}

pub async fn claim_codex_reset(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResetClaimBody>,
) -> ApiResult<ResetClaimOutcome> {
    if !state
        .settings
        .enabled_provider_ids()
        .iter()
        .any(|id| id == "codex")
    {
        return Err(bad_request("Codex is not enabled."));
    }
    let claims = state.claims.clone();
    let outcome = crate::runtime::spawn_blocking(move || {
        claims.claim(body.expires_at, &body.redeem_request_id)
    })
    .await
    .map_err(|_| internal("The reset claim could not be completed."))?;

    if outcome != ResetClaimOutcome::Failed {
        let observed = AtomicU64::new(state.settings.account_revision());
        state.service.refresh("codex", true).await;
        let result = state.service.state();
        emit_account_if_changed(&state, &observed);
        events::emit(&state.events, "usage-state", &result);
    }
    Ok(Json(outcome))
}

fn resolve_runtime(
    state: &AppState,
    provider_id: &str,
) -> Result<Arc<dyn UsageProvider>, ApiError> {
    state
        .registry
        .runtime(provider_id)
        .ok_or_else(|| bad_request("Unknown provider."))
}

#[derive(Serialize)]
pub struct ProviderLinkResponse {
    pub url: String,
}

pub async fn get_provider_link(
    Path((provider_id, link_index)): Path<(String, usize)>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<ProviderLinkResponse> {
    let link = state
        .registry
        .definition(&provider_id)
        .and_then(|definition| definition.links.get(link_index))
        .ok_or_else(|| bad_request("That provider link is unavailable."))?;
    Ok(Json(ProviderLinkResponse {
        url: link.url.clone(),
    }))
}

/// Where a provider's credential file lives inside the container.
fn credential_path(provider_id: &str) -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/data/home"));
    match crate::providers::provider_family(provider_id) {
        "codex" => Some(
            std::env::var_os("CODEX_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("auth.json"),
        ),
        "claude" => Some(
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join(".credentials.json"),
        ),
        "opencode" => {
            let base = std::env::var_os("OPENCODE_DATA_DIR")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("XDG_DATA_HOME")
                        .map(|dir| std::path::PathBuf::from(dir).join("opencode"))
                })
                .unwrap_or_else(|| home.join(".local").join("share").join("opencode"));
            Some(base.join("auth.json"))
        }
        "antigravity" => Some(
            super::paths::app_data_dir()
                .join("antigravity")
                .join("auth.json"),
        ),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct CredentialsBody {
    content: String,
}

/// Import a CLI provider's credential file from the browser.
pub async fn save_provider_credentials(
    Path(provider_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CredentialsBody>,
) -> Result<StatusCode, ApiError> {
    let path = credential_path(&provider_id)
        .ok_or_else(|| bad_request("That provider does not accept imported credentials."))?;
    let content = body.content.trim();
    if content.is_empty() {
        return Err(bad_request("Paste the credential file contents first."));
    }
    if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
        serde_json::from_str::<serde_json::Value>(content)
            .map_err(|_| bad_request("That credential file is not valid JSON."))?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| internal("The credential directory could not be created."))?;
    }
    std::fs::write(&path, content).map_err(|_| internal("The credentials could not be saved."))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    crate::app_info!("auth", "credentials imported for {provider_id}");

    let command_guard = state.settings.lock_command_mutation().await;
    let _ = reconcile_credential(&state, &provider_id, true, true);
    drop(command_guard);
    state.service.refresh(&provider_id, true).await;
    let usage = state.service.state();
    events::emit(&state.events, "usage-state", &usage);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_api_key(
    Path(provider_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Option<ProviderApiKeyState>> {
    let runtime = resolve_runtime(&state, &provider_id)?;
    let id = provider_id.clone();
    let result =
        crate::runtime::spawn_blocking(move || -> Result<Option<ProviderApiKeyState>, String> {
            let Some(status) = runtime.api_key_status() else {
                return Ok(None);
            };
            let status = status.map_err(|error| error.to_string())?;
            Ok(Some(ProviderApiKeyState {
                provider_id: id,
                status,
            }))
        })
        .await
        .map_err(|_| internal("The API key status could not be read."))?
        .map_err(internal)?;
    Ok(Json(result))
}

enum ApiKeyMutation<'a> {
    Save(&'a str),
    Delete,
}

struct AppliedApiKeyMutation {
    state: ProviderApiKeyState,
    status_uncertain: bool,
}

fn mutate_api_key(
    runtime: &dyn UsageProvider,
    provider_id: String,
    mutation: ApiKeyMutation<'_>,
) -> Result<AppliedApiKeyMutation, String> {
    let initial_status = runtime
        .api_key_status()
        .ok_or_else(|| "That provider does not accept an API key.".to_owned())?
        .ok();
    let fallback_status = match &mutation {
        ApiKeyMutation::Save(_) => {
            if matches!(
                initial_status,
                Some(
                    ApiKeyStatus::FromEnvironment
                        | ApiKeyStatus::FromConfig
                        | ApiKeyStatus::OverrideActive
                )
            ) {
                ApiKeyStatus::OverrideActive
            } else {
                ApiKeyStatus::Saved
            }
        }
        ApiKeyMutation::Delete => ApiKeyStatus::NotSet,
    };

    match mutation {
        ApiKeyMutation::Save(value) => runtime.save_api_key(value),
        ApiKeyMutation::Delete => runtime.delete_api_key(),
    }
    .map_err(|error| error.to_string())?;

    let (status, status_uncertain) = match runtime.api_key_status() {
        Some(Ok(status)) => (status, false),
        Some(Err(_)) | None => (fallback_status, true),
    };
    Ok(AppliedApiKeyMutation {
        state: ProviderApiKeyState {
            provider_id,
            status,
        },
        status_uncertain,
    })
}

fn reconcile_credential(state: &AppState, provider_id: &str, detected: bool, enable: bool) -> bool {
    match state
        .settings
        .reconcile_provider_credential_state(provider_id, detected, enable)
    {
        Ok(_) => {
            events::emit(&state.events, "settings-state", &view_state(state));
            true
        }
        Err(error) => {
            crate::app_warn!(
                "auth",
                "provider state after API key change could not be reconciled for {provider_id}: {error}"
            );
            false
        }
    }
}

fn incomplete_mutation_warning(action: &str) -> String {
    format!(
        "The API key was {action}, but OpenQuota could not finish updating provider status. Reload the page or try again."
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyBody {
    api_key: String,
}

pub async fn save_api_key(
    Path(provider_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ApiKeyBody>,
) -> ApiResult<ApiKeyMutationOutcome> {
    let api_key = Zeroizing::new(body.api_key);
    let runtime = resolve_runtime(&state, &provider_id)?;
    let credential_guard = state.settings.lock_credential_mutation().await;
    state.settings.record_provider_credential_mutation();
    let id_for_save = provider_id.clone();
    let applied = crate::runtime::spawn_blocking(move || {
        mutate_api_key(
            runtime.as_ref(),
            id_for_save,
            ApiKeyMutation::Save(api_key.as_str()),
        )
    })
    .await
    .map_err(|_| internal("The API key could not be saved."))?
    .map_err(internal)?;

    let command_guard = state.settings.lock_command_mutation().await;
    let reconciled = reconcile_credential(&state, &provider_id, true, true);
    if applied.status_uncertain {
        crate::app_warn!(
            "auth",
            "API key status could not be confirmed after saving for {provider_id}"
        );
    }
    drop(command_guard);
    drop(credential_guard);

    state.service.refresh(&provider_id, true).await;
    let usage = state.service.state();
    events::emit(&state.events, "usage-state", &usage);
    crate::app_info!("auth", "API key saved for {provider_id}");
    Ok(Json(ApiKeyMutationOutcome {
        state: applied.state,
        warning: (applied.status_uncertain || !reconciled)
            .then(|| incomplete_mutation_warning("saved securely")),
    }))
}

pub async fn delete_api_key(
    Path(provider_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<ApiKeyMutationOutcome> {
    let runtime = resolve_runtime(&state, &provider_id)?;
    let credential_guard = state.settings.lock_credential_mutation().await;
    state.settings.record_provider_credential_mutation();
    let id_for_delete = provider_id.clone();
    let applied = crate::runtime::spawn_blocking(move || {
        mutate_api_key(runtime.as_ref(), id_for_delete, ApiKeyMutation::Delete)
    })
    .await
    .map_err(|_| internal("The API key could not be removed."))?
    .map_err(internal)?;

    let command_guard = state.settings.lock_command_mutation().await;
    let detected = applied.state.status != ApiKeyStatus::NotSet;
    let reconciled = reconcile_credential(&state, &provider_id, detected, false);
    if applied.status_uncertain {
        crate::app_warn!(
            "auth",
            "API key status could not be confirmed after removal for {provider_id}"
        );
    }
    let should_refresh = state
        .settings
        .get()
        .providers
        .iter()
        .any(|provider| provider.id == provider_id && provider.enabled);
    drop(command_guard);
    drop(credential_guard);

    if should_refresh {
        state.service.refresh(&provider_id, true).await;
        let usage = state.service.state();
        events::emit(&state.events, "usage-state", &usage);
    }
    crate::app_info!("auth", "saved API key removed for {provider_id}");
    Ok(Json(ApiKeyMutationOutcome {
        state: applied.state,
        warning: (applied.status_uncertain || !reconciled)
            .then(|| incomplete_mutation_warning("removed")),
    }))
}

pub async fn get_settings(State(state): State<Arc<AppState>>) -> Json<SettingsViewState> {
    Json(view_state(&state))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsBody {
    settings: AppSettings,
    expected_settings_revision: u64,
    expected_account_revision: u64,
}

pub async fn save_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveSettingsBody>,
) -> ApiResult<SettingsViewState> {
    let command_guard = state.settings.lock_command_mutation().await;
    let previous = state.settings.get();
    let updated = state
        .settings
        .update_from_view(
            body.settings,
            body.expected_settings_revision,
            body.expected_account_revision,
        )
        .map_err(bad_request)?;
    if previous.log_level != updated.log_level {
        logging::set_level(updated.log_level);
    }
    events::emit(&state.events, "settings-state", &view_state(&state));
    let newly_enabled = newly_enabled_provider_ids(&previous, &updated);
    drop(command_guard);

    if !newly_enabled.is_empty() {
        refresh_providers(&state, &newly_enabled).await;
    }
    Ok(Json(view_state(&state)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionBody {
    expected_settings_revision: u64,
    expected_account_revision: u64,
}

pub async fn reset_customization(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RevisionBody>,
) -> ApiResult<SettingsViewState> {
    let command_guard = state.settings.lock_command_mutation().await;
    let previous = state.settings.get();
    let mut next = previous.clone();
    let detected_before = next
        .providers
        .iter()
        .filter(|provider| provider.detected)
        .map(|provider| provider.id.clone())
        .collect::<HashSet<_>>();
    next.providers = state.settings.default_settings(&detected_before).providers;
    next.detection_notice_dismissed = false;
    let updated = state
        .settings
        .update_from_view(
            next,
            body.expected_settings_revision,
            body.expected_account_revision,
        )
        .map_err(bad_request)?;
    let newly_enabled = newly_enabled_provider_ids(&previous, &updated);
    let plan = state.settings.reset_detection_plan();
    events::emit(&state.events, "settings-state", &view_state(&state));
    drop(command_guard);
    spawn_provider_reseed(state.clone(), plan, newly_enabled);
    Ok(Json(view_state(&state)))
}

pub async fn reset_all_settings(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RevisionBody>,
) -> ApiResult<SettingsViewState> {
    let defaults = state.settings.reset_defaults();
    let command_guard = state.settings.lock_command_mutation().await;
    let previous = state.settings.get();
    let updated = state
        .settings
        .reset_all_from_view(
            defaults,
            body.expected_settings_revision,
            body.expected_account_revision,
        )
        .map_err(bad_request)?;
    if previous.log_level != updated.log_level {
        logging::set_level(updated.log_level);
    }
    let newly_enabled = newly_enabled_provider_ids(&previous, &updated);
    let plan = state.settings.reset_detection_plan();
    events::emit(&state.events, "settings-state", &view_state(&state));
    drop(command_guard);
    spawn_provider_reseed(state.clone(), plan, newly_enabled);
    Ok(Json(view_state(&state)))
}

pub async fn reset_provider(
    Path(provider_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RevisionBody>,
) -> ApiResult<SettingsViewState> {
    let command_guard = state.settings.lock_command_mutation().await;
    state
        .settings
        .reset_provider(
            &provider_id,
            body.expected_settings_revision,
            body.expected_account_revision,
        )
        .map_err(bad_request)?;
    events::emit(&state.events, "settings-state", &view_state(&state));
    drop(command_guard);
    Ok(Json(view_state(&state)))
}

fn newly_enabled_provider_ids(previous: &AppSettings, next: &AppSettings) -> Vec<String> {
    next.providers
        .iter()
        .filter(|provider| {
            provider.enabled
                && !previous
                    .providers
                    .iter()
                    .any(|old| old.id == provider.id && old.enabled)
        })
        .map(|provider| provider.id.clone())
        .collect()
}

fn spawn_provider_reseed(
    state: Arc<AppState>,
    plan: crate::settings::CredentialDetectionPlan,
    mut refresh_ids: Vec<String>,
) {
    crate::runtime::spawn(async move {
        let detected =
            crate::providers::detect_local_credentials(state.registry.clone(), plan.provider_ids())
                .await;
        let command_guard = state.settings.lock_command_mutation().await;
        if let Ok(outcome) = state.settings.apply_credential_detection(&plan, &detected) {
            events::emit(&state.events, "settings-state", &view_state(&state));
            for provider_id in outcome.newly_enabled_provider_ids {
                if !refresh_ids.contains(&provider_id) {
                    refresh_ids.push(provider_id);
                }
            }
        }
        drop(command_guard);
        let enabled = state
            .settings
            .enabled_provider_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        refresh_ids.retain(|provider_id| enabled.contains(provider_id));
        if refresh_ids.is_empty() {
            return;
        }
        refresh_providers(&state, &refresh_ids).await;
        events::emit(&state.events, "settings-state", &view_state(&state));
    });
}

pub async fn request_notification_permission(
    State(state): State<Arc<AppState>>,
) -> Json<SettingsViewState> {
    Json(view_state(&state))
}

pub async fn get_log_path() -> Json<String> {
    Json(logging::log_path().to_string_lossy().into_owned())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatusResponse {
    available: bool,
    current_version: &'static str,
    version: Option<String>,
    body: Option<String>,
    installable: bool,
    release_url: &'static str,
}

pub async fn get_updates() -> Json<UpdateStatusResponse> {
    Json(UpdateStatusResponse {
        available: false,
        current_version: env!("CARGO_PKG_VERSION"),
        version: None,
        body: None,
        installable: false,
        release_url: "https://github.com/deviffyy/OpenQuota/releases",
    })
}

pub async fn events_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = stream::unfold(receiver, |mut receiver| async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let sse = Event::default().event(event.name).data(event.data);
                    return Some((Ok::<_, Infallible>(sse), receiver));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Optional HTTP Basic authentication for the whole API/UI.
pub async fn require_auth(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let configured_user = std::env::var("OPENQUOTA_AUTH_USER").unwrap_or_default();
    let configured_password = std::env::var("OPENQUOTA_AUTH_PASSWORD").unwrap_or_default();
    if configured_user.is_empty()
        || configured_password.is_empty()
        || request.uri().path() == "/api/health"
    {
        return next.run(request).await;
    }
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|encoded| {
            use base64::{engine::general_purpose::STANDARD, Engine};
            STANDARD.decode(encoded.trim()).ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .is_some_and(|credentials| {
            credentials == format!("{configured_user}:{configured_password}")
        });

    if authorized {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [("WWW-Authenticate", "Basic realm=\"OpenQuota\"")],
            "Authentication required.",
        )
            .into_response()
    }
}

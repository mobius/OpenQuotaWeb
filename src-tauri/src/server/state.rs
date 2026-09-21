use std::sync::Arc;

use crate::{
    pricing::PricingStore,
    providers::{
        antigravity::AntigravityProvider, claude, codex,
        codex::reset_claim::CodexResetClaimService, copilot::CopilotProvider, cursor,
        deepseek::DeepSeekProvider, devin::DevinProvider, grok::GrokProvider, kimi::KimiProvider,
        minimax::MiniMaxProvider, opencode::OpenCodeProvider, openrouter::OpenRouterProvider,
        zai::ZaiProvider, ProviderRegistry, UsageProvider,
    },
    service::ProviderService,
    settings::{CredentialDetectionPlan, SettingsService},
    storage::Storage,
};

use super::{events::EventSender, paths};

/// Shared server state, mirroring the pieces the desktop app manages through
/// Tauri.
pub struct AppState {
    pub service: Arc<ProviderService>,
    pub settings: Arc<SettingsService>,
    pub registry: Arc<ProviderRegistry>,
    pub storage: Arc<Storage>,
    pub claims: Arc<CodexResetClaimService>,
    pub events: EventSender,
}

pub struct AppStateInit {
    pub state: Arc<AppState>,
    pub detection_plan: CredentialDetectionPlan,
}

/// Build the provider registry and services exactly like the desktop app does,
/// using the same persistent database and provider homes.
pub fn initialize() -> Result<AppStateInit, Box<dyn std::error::Error>> {
    let app_data_dir = paths::app_data_dir();
    std::fs::create_dir_all(&app_data_dir)?;

    let storage = Arc::new(Storage::open(&app_data_dir.join("openquota.db"))?);
    crate::provider_environment::initialize(storage.load_provider_environment()?);

    let pricing = Arc::new(PricingStore::new(app_data_dir.join("pricing"))?);
    let mut providers = claude::runtimes(storage.clone(), pricing.clone())?;
    providers.extend(codex::CodexProvider::runtimes(
        storage.clone(),
        pricing.clone(),
    )?);
    providers.extend(cursor::runtimes(pricing.clone())?);
    providers.extend(vec![
        Arc::new(DeepSeekProvider::new()?) as Arc<dyn UsageProvider>,
        Arc::new(AntigravityProvider::new(
            app_data_dir.join("antigravity").join("auth.json"),
        )?) as Arc<dyn UsageProvider>,
        Arc::new(CopilotProvider::new()?) as Arc<dyn UsageProvider>,
        Arc::new(DevinProvider::new()?) as Arc<dyn UsageProvider>,
        Arc::new(GrokProvider::new(storage.clone(), pricing.clone())?) as Arc<dyn UsageProvider>,
        Arc::new(OpenCodeProvider::new(pricing.clone())) as Arc<dyn UsageProvider>,
        Arc::new(OpenRouterProvider::new()?) as Arc<dyn UsageProvider>,
        Arc::new(ZaiProvider::new()?) as Arc<dyn UsageProvider>,
        Arc::new(KimiProvider::new()?) as Arc<dyn UsageProvider>,
        Arc::new(MiniMaxProvider::new()?) as Arc<dyn UsageProvider>,
    ]);

    let registry = Arc::new(ProviderRegistry::new(providers)?);
    let (settings_service, detection_plan) =
        SettingsService::new_deferred(storage.clone(), registry.clone())?;
    let settings = Arc::new(settings_service);
    let service = Arc::new(ProviderService::new_with_settings(
        registry.clone(),
        storage.clone(),
        settings.clone(),
    ));
    let claims = Arc::new(CodexResetClaimService::new()?);
    let events = super::events::channel();

    Ok(AppStateInit {
        state: Arc::new(AppState {
            service,
            settings,
            registry,
            storage,
            claims,
            events,
        }),
        detection_plan,
    })
}

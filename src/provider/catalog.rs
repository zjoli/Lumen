//! Provider catalog is data; authentication and model discovery have independent lifecycles.
use super::{credentials::Secret, management::connection_key, *};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
pub struct ProviderPreset {
    pub id: String,
    pub label: String,
    pub protocol: ProtocolKind,
    pub base_url: String,
    pub env: Option<String>,
}
impl ProviderPreset {
    pub fn connection(&self) -> ProviderConnection {
        protocol_connection(self.protocol, self.base_url.clone(), self.auth())
    }
    pub fn auth(&self) -> AuthConfig {
        self.env
            .clone()
            .map(|variable| AuthConfig::Env { variable })
            .unwrap_or(AuthConfig::None)
    }
}
pub fn provider_presets() -> Vec<ProviderPreset> {
    serde_json::from_str(include_str!("catalog.json")).expect("bundled provider catalog")
}
pub fn provider_preset(id: &str) -> Option<ProviderPreset> {
    provider_presets().into_iter().find(|p| p.id == id)
}
#[derive(Clone, Debug)]
pub struct ProviderEntry {
    pub id: String,
    pub label: String,
    pub connection: ProviderConnection,
    pub configured: bool,
    pub authenticated: bool,
}
#[derive(Clone, Debug)]
pub struct AvailableModel {
    pub provider: String,
    pub alias: Option<String>,
    pub model: DiscoveredModel,
    pub connection_key: String,
}
impl ProviderRegistry {
    pub fn provider_catalog(&self) -> Result<Vec<ProviderEntry>, ProviderError> {
        let mut entries = BTreeMap::new();
        for preset in provider_presets() {
            entries.insert(
                preset.id.clone(),
                ProviderEntry {
                    id: preset.id.clone(),
                    label: preset.label.clone(),
                    connection: preset.connection(),
                    configured: false,
                    authenticated: false,
                },
            );
        }
        if let Some(config) = self.config()? {
            for (id, connection) in config.providers {
                let label = entries
                    .get(&id)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| id.clone());
                entries.insert(
                    id.clone(),
                    ProviderEntry {
                        id,
                        label,
                        connection,
                        configured: true,
                        authenticated: false,
                    },
                );
            }
        }
        for entry in entries.values_mut() {
            entry.authenticated = Secret::resolve_at(&entry.connection.auth, self.root()).is_ok();
        }
        Ok(entries.into_values().collect())
    }
    /// Offline snapshot. Cached metadata is only accepted for the current connection and auth.
    pub fn available_models(&self) -> Result<Vec<AvailableModel>, ProviderError> {
        let mut models = vec![];
        let cache = self.cached_catalogs()?;
        if let Some(config) = self.config()? {
            for (provider, connection) in &config.providers {
                if Secret::resolve_at(&connection.auth, self.root()).is_err() {
                    continue;
                }
                let key = connection_key(connection);
                for (alias, model) in config
                    .models
                    .iter()
                    .filter(|(_, m)| &m.provider == provider)
                {
                    models.push(AvailableModel {
                        provider: provider.clone(),
                        alias: Some(alias.clone()),
                        model: DiscoveredModel {
                            id: model.model_id.clone(),
                            name: None,
                            context_tokens: Some(model.capabilities.context_tokens),
                            max_output_tokens: Some(model.capabilities.max_output_tokens),
                        },
                        connection_key: key.clone(),
                    });
                }
                if let Some(catalog) = cache.get(provider).filter(|c| c.connection_key == key) {
                    for model in &catalog.models {
                        if models
                            .iter()
                            .any(|m| &m.provider == provider && m.model.id == model.id)
                        {
                            continue;
                        }
                        models.push(AvailableModel {
                            provider: provider.clone(),
                            alias: None,
                            model: model.clone(),
                            connection_key: key.clone(),
                        });
                    }
                }
            }
        }
        Ok(models)
    }
}

//! Local provider editing and OpenAI-compatible model discovery.
use super::{credentials::Secret, *};
use crate::workspace::files;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const CONFIG: &str = ".lumen/providers.yaml";
const CREDENTIALS: &str = ".lumen/provider-credentials.json";
type Result<T> = std::result::Result<T, ProviderError>;
fn storage_error() -> ProviderError {
    ProviderError::configuration("无法保存供应商配置，请检查目录权限")
}
pub fn compatible_connection(base_url: String, auth: AuthConfig) -> ProviderConnection {
    protocol_connection(ProtocolKind::OpenaiChat, base_url, auth)
}
pub fn protocol_connection(
    protocol: ProtocolKind,
    base_url: String,
    auth: AuthConfig,
) -> ProviderConnection {
    ProviderConnection {
        protocol,
        base_url,
        state_scope: uuid::Uuid::now_v7().to_string(),
        auth,
        transport: TransportConfig::default(),
        protocol_options: ProtocolOptions {
            output_token_field: (protocol == ProtocolKind::OpenaiChat)
                .then_some(OutputTokenField::MaxTokens),
            ..Default::default()
        },
    }
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name == "mock"
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ProviderError::configuration(
            "供应商名称限 1–64 个字母、数字、下划线或短横线，不能为 mock",
        ));
    }
    Ok(())
}
fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 8192 && key.bytes().all(|b| (b'!'..=b'~').contains(&b))
}
pub(crate) fn read_credentials(root: &Path) -> Result<BTreeMap<String, String>> {
    let path = files::safe_path(root, CREDENTIALS).map_err(|_| storage_error())?;
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(_) => return Err(storage_error()),
    };
    if bytes.len() > 1024 * 1024 {
        return Err(ProviderError::configuration(
            "local credential store exceeds size limit",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|_| ProviderError::configuration("无法读取本机凭据文件"))
}
fn store_key(root: &Path, key: &str) -> Result<String> {
    let key = key.trim();
    if !valid_key(key) {
        return Err(ProviderError::configuration(
            "API Key 不能为空、包含空白或超过 8192 字节",
        ));
    }
    let mut values = read_credentials(root)?;
    let reference = uuid::Uuid::now_v7().to_string();
    values.insert(reference.clone(), key.into());
    write_credentials(root, &values)?;
    Ok(reference)
}
fn write_credentials(root: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    let bytes = serde_json::to_vec(values).map_err(|_| storage_error())?;
    if bytes.len() > 1024 * 1024 {
        return Err(ProviderError::configuration(
            "local credential store exceeds size limit",
        ));
    }
    let dest = files::safe_path(root, CREDENTIALS).map_err(|_| storage_error())?;
    let parent = dest.parent().ok_or_else(storage_error)?;
    fs::create_dir_all(parent).map_err(|_| storage_error())?;
    let temp = parent.join(format!(".lumen-{}.tmp", uuid::Uuid::now_v7()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp).map_err(|_| storage_error())?;
        file.write_all(&bytes).map_err(|_| storage_error())?;
        file.sync_all().map_err(|_| storage_error())?;
        drop(file);
        fs::rename(&temp, &dest).map_err(|_| storage_error())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
fn edit(root: &Path, f: impl FnOnce(&mut ProviderConfig) -> Result<()>) -> Result<()> {
    let lock_path =
        files::safe_path(root, ".lumen/provider-config.lock").map_err(|_| storage_error())?;
    fs::create_dir_all(lock_path.parent().ok_or_else(storage_error)?)
        .map_err(|_| storage_error())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|_| storage_error())?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| ProviderError::configuration("供应商配置正在修改，请重试"))?;
    let config_path = files::safe_path(root, CONFIG).map_err(|_| storage_error())?;
    let before = fs::read(&config_path).ok();
    let mut config = ProviderConfig::load(root)?.unwrap_or_else(|| ProviderConfig {
        schema_version: 1,
        default_model: String::new(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
    });
    let local_refs = |cfg: &ProviderConfig| {
        cfg.providers
            .values()
            .filter_map(|p| match &p.auth {
                AuthConfig::Local { credential } => Some(credential.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>()
    };
    let before_refs = local_refs(&config);
    f(&mut config)?;
    config.validate()?;
    let yaml = serde_yaml_ng::to_string(&config).map_err(|_| storage_error())?;
    if fs::read(&config_path).ok() != before {
        return Err(ProviderError::configuration(
            "供应商配置已被其他程序修改，请重试",
        ));
    }
    files::atomic_write(root, CONFIG, yaml.as_bytes()).map_err(|_| storage_error())?;
    let after_refs = local_refs(&config);
    let removed: Vec<_> = before_refs.difference(&after_refs).collect();
    if !removed.is_empty() {
        let mut values = read_credentials(root)?;
        for reference in removed {
            values.remove(reference);
        }
        write_credentials(root, &values)?;
    }
    Ok(())
}
pub(crate) fn connection_key(connection: &ProviderConnection) -> String {
    blake3::hash(&serde_json::to_vec(connection).unwrap_or_default())
        .to_hex()
        .to_string()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: Option<String>,
    pub context_tokens: Option<usize>,
    pub max_output_tokens: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderModelCatalog {
    pub provider: String,
    pub connection_key: String,
    pub models: Vec<DiscoveredModel>,
}
impl ProviderModelCatalog {
    pub fn render(&self) -> String {
        let mut text = format!("## {} 的可用模型\n", self.provider);
        for model in &self.models {
            text.push_str(&format!(
                "\n- {}{} · 上下文 {} · 最大输出 {}",
                model.id,
                model
                    .name
                    .as_ref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default(),
                model
                    .context_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "未提供".into()),
                model
                    .max_output_tokens
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "未提供".into())
            ));
        }
        if self.models.is_empty() {
            text.push_str("\n接口没有返回可用模型。");
        }
        text.push_str("\n\n通过 /provider model <供应商> <模型ID> <别名> <上下文> <输出上限> 添加；工具能力请按实际模型核对。");
        text
    }
}
#[derive(Clone, Debug)]
pub struct RegisterModel {
    pub provider: String,
    pub model_id: String,
    pub alias: String,
    pub context_tokens: usize,
    pub output_tokens: usize,
    pub tools: bool,
    pub make_default: bool,
    pub connection_key: Option<String>,
}
impl ProviderRegistry {
    pub(crate) fn cached_catalogs(&self) -> Result<BTreeMap<String, ProviderModelCatalog>> {
        let path = files::safe_path(self.root(), ".lumen/model-cache.json")
            .map_err(|_| storage_error())?;
        let bytes = match fs::read(path) {
            Ok(bytes) if bytes.len() <= 8 * 1024 * 1024 => bytes,
            _ => return Ok(BTreeMap::new()),
        };
        Ok(serde_json::from_slice(&bytes).unwrap_or_default())
    }
    fn cache_catalog(&self, catalog: &ProviderModelCatalog) -> Result<()> {
        let mut cache = self.cached_catalogs()?;
        cache.insert(catalog.provider.clone(), catalog.clone());
        let bytes = serde_json::to_vec(&cache).map_err(|_| storage_error())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Ok(());
        }
        files::atomic_write(self.root(), ".lumen/model-cache.json", &bytes)
            .map_err(|_| storage_error())
    }
    /// Removes persisted local credentials. Environment variables remain under the caller's control.
    pub fn logout(&self, name: &str) -> Result<()> {
        edit(self.root(), |cfg| {
            let provider = cfg
                .providers
                .get_mut(name)
                .ok_or_else(|| ProviderError::configuration("供应商不存在"))?;
            if matches!(provider.auth, AuthConfig::Local { .. }) {
                provider.auth = provider_preset(name)
                    .and_then(|p| p.env)
                    .map(|variable| AuthConfig::Env { variable })
                    .unwrap_or_else(|| AuthConfig::Env {
                        variable: format!(
                            "{}_API_KEY",
                            name.to_ascii_uppercase().replace('-', "_")
                        ),
                    });
            }
            Ok(())
        })
    }
    pub fn add_provider(
        &self,
        name: String,
        mut connection: ProviderConnection,
        api_key: Option<&str>,
    ) -> Result<()> {
        check_name(&name)?;
        edit(self.root(), |cfg| {
            if cfg.providers.contains_key(&name) {
                return Err(ProviderError::configuration(
                    "供应商已存在；修改 Key 请使用 /provider key <名称>",
                ));
            }
            cfg.providers.insert(name.clone(), connection.clone());
            cfg.validate()?;
            if let Some(key) = api_key {
                connection.auth = AuthConfig::Local {
                    credential: store_key(self.root(), key)?,
                };
                cfg.providers.insert(name, connection);
            }
            Ok(())
        })
    }
    pub fn set_api_key(&self, name: &str, key: &str) -> Result<()> {
        edit(self.root(), |cfg| {
            let provider = cfg
                .providers
                .get_mut(name)
                .ok_or_else(|| ProviderError::configuration("供应商不存在，请先添加"))?;
            provider.auth = AuthConfig::Local {
                credential: store_key(self.root(), key)?,
            };
            Ok(())
        })
    }
    pub fn set_provider_auth(&self, name: &str, auth: AuthConfig) -> Result<()> {
        if matches!(auth, AuthConfig::Local { .. }) {
            return Err(ProviderError::configuration(
                "请通过 API Key 输入框设置本机凭据",
            ));
        }
        edit(self.root(), |cfg| {
            cfg.providers
                .get_mut(name)
                .ok_or_else(|| ProviderError::configuration("供应商不存在"))?
                .auth = auth;
            Ok(())
        })
    }
    pub fn register_model(&self, request: RegisterModel) -> Result<()> {
        if request.alias.is_empty()
            || request.alias == "mock"
            || request.alias.len() > 200
            || request.alias.chars().any(char::is_whitespace)
            || request.model_id.is_empty()
            || request.model_id.len() > 512
            || request.model_id.chars().any(char::is_control)
        {
            return Err(ProviderError::configuration(
                "模型 ID 或别名无效，别名不能包含空白或为 mock",
            ));
        }
        edit(self.root(), |cfg| {
            let provider = cfg
                .providers
                .get(&request.provider)
                .ok_or_else(|| ProviderError::configuration("供应商不存在"))?;
            if request
                .connection_key
                .as_ref()
                .is_some_and(|key| key != &connection_key(provider))
            {
                return Err(ProviderError::configuration(
                    "供应商已改变，请重新查询模型列表",
                ));
            }
            if cfg.models.contains_key(&request.alias) {
                return Err(ProviderError::configuration(
                    "模型别名已存在，请选择其他别名或用 /model <别名> 切换",
                ));
            }
            let caps = ModelCapabilities {
                context_tokens: request.context_tokens,
                max_output_tokens: request.output_tokens,
                streaming: provider.transport.mode == TransportMode::Sse,
                tools: request.tools,
                tool_choice: if request.tools {
                    vec![
                        "auto".into(),
                        "none".into(),
                        "required".into(),
                        "named".into(),
                    ]
                } else {
                    vec![]
                },
                temperature: false,
                cache_modes: vec![CacheIntent::Disabled],
                ..ModelCapabilities::default()
            };
            if cfg.models.is_empty() || request.make_default {
                cfg.default_model = request.alias.clone();
            }
            cfg.models.insert(
                request.alias,
                ModelDefinition {
                    provider: request.provider,
                    model_id: request.model_id,
                    capabilities: caps,
                    defaults: GenerationOverrides {
                        max_output_tokens: Some(request.output_tokens),
                        cache: Some(CacheIntent::Disabled),
                        ..Default::default()
                    },
                },
            );
            Ok(())
        })
    }
    /// GET /models only: discovery never sends a chat request or infers model capacities.
    pub async fn discover_models(
        &self,
        name: &str,
        cancel: CancellationToken,
    ) -> Result<ProviderModelCatalog> {
        if cancel.is_cancelled() {
            return Err(ProviderError::cancelled());
        }
        let config = self
            .config()?
            .ok_or_else(|| ProviderError::configuration("请先添加供应商"))?;
        let provider = config
            .providers
            .get(name)
            .ok_or_else(|| ProviderError::configuration("供应商不存在"))?;
        if provider.protocol == ProtocolKind::Mock {
            return Err(ProviderError::unsupported("Mock 没有远端模型接口"));
        }
        let secret = Secret::resolve_at(&provider.auth, self.root())?;
        let http = self.discovery_transport(&provider.transport)?;
        let base = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let mut headers = vec![("accept".into(), "application/json".into())];
        if let Some(key) = secret {
            if provider.protocol == ProtocolKind::AnthropicMessages {
                headers.push(("x-api-key".into(), key.0));
            } else {
                headers.push(("authorization".into(), format!("Bearer {}", key.0)));
            }
        }
        if provider.protocol == ProtocolKind::AnthropicMessages {
            headers.push(("anthropic-version".into(), "2023-06-01".into()));
        }
        let work = async {
            let mut models = BTreeMap::new();
            let mut cursors = BTreeSet::new();
            let mut after: Option<String> = None;
            let mut total = 0usize;
            for _ in 0..20 {
                let mut url = reqwest::Url::parse(&base)
                    .map_err(|_| ProviderError::configuration("invalid model catalog endpoint"))?;
                if let Some(cursor) = &after {
                    url.query_pairs_mut().append_pair(
                        if provider.protocol == ProtocolKind::AnthropicMessages {
                            "after_id"
                        } else {
                            "after"
                        },
                        cursor,
                    );
                }
                let request = HttpRequest {
                    url: url.to_string(),
                    headers: headers.clone(),
                    body: Vec::<u8>::new().into(),
                };
                let response = http.get(request).await.map_err(|_| {
                    ProviderError::new(
                        ProviderErrorKind::Transport,
                        "模型列表查询失败，请检查服务地址与网络",
                    )
                })?;
                if !(200..300).contains(&response.status) {
                    let (kind, message) = match response.status {
                        401 | 403 => (
                            ProviderErrorKind::Authentication,
                            "模型列表认证失败，请检查 API Key",
                        ),
                        404 | 405 => (
                            ProviderErrorKind::UnsupportedFeature,
                            "该地址没有提供 GET /models；可通过 /provider model 手工添加",
                        ),
                        429 => (
                            ProviderErrorKind::RateLimited,
                            "模型列表查询限流，请稍后重试",
                        ),
                        _ => (ProviderErrorKind::Upstream, "模型列表接口返回错误"),
                    };
                    let mut e = ProviderError::new(kind, message);
                    e.status = Some(response.status);
                    return Err(e);
                }
                if !response.headers.get("content-type").is_some_and(|t| {
                    t.split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("application/json")
                }) {
                    return Err(ProviderError::protocol(
                        "模型列表接口需要返回 application/json",
                    ));
                }
                let mut body = response.body;
                let mut bytes = vec![];
                while let Some(chunk) = tokio::time::timeout(
                    Duration::from_millis(provider.transport.idle_timeout_ms),
                    body.next(),
                )
                .await
                .map_err(|_| ProviderError::new(ProviderErrorKind::Timeout, "模型列表读取超时"))?
                {
                    let chunk = chunk.map_err(|_| {
                        ProviderError::new(ProviderErrorKind::Transport, "模型列表传输中断")
                    })?;
                    total = total.saturating_add(chunk.len());
                    if total > 2 * 1024 * 1024 {
                        return Err(ProviderError::protocol("模型列表超过 2 MiB 上限"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let value: Value = serde_json::from_slice(&bytes)
                    .map_err(|_| ProviderError::protocol("模型列表 JSON 无效"))?;
                let data = value["data"]
                    .as_array()
                    .ok_or_else(|| ProviderError::protocol("模型列表缺少 data 数组"))?;
                for entry in data {
                    let id = entry["id"]
                        .as_str()
                        .filter(|s| {
                            !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
                        })
                        .ok_or_else(|| ProviderError::protocol("模型列表包含无效 ID"))?;
                    let count = |keys: &[&str]| {
                        keys.iter().find_map(|k| {
                            entry[*k]
                                .as_u64()
                                .and_then(|n| usize::try_from(n).ok())
                                .filter(|n| *n > 0)
                        })
                    };
                    models
                        .entry(id.to_string())
                        .or_insert_with(|| DiscoveredModel {
                            id: id.into(),
                            name: entry["name"]
                                .as_str()
                                .or_else(|| entry["display_name"].as_str())
                                .map(|s| s.chars().take(200).collect()),
                            context_tokens: count(&[
                                "context_window",
                                "context_length",
                                "max_context_tokens",
                            ]),
                            max_output_tokens: count(&["max_output_tokens"]).or_else(|| {
                                entry["top_provider"]["max_completion_tokens"]
                                    .as_u64()
                                    .and_then(|n| usize::try_from(n).ok())
                                    .filter(|n| *n > 0)
                            }),
                        });
                    if models.len() > 1000 {
                        return Err(ProviderError::protocol("模型列表超过 1000 项上限"));
                    }
                }
                if value["has_more"].as_bool() != Some(true) {
                    return Ok(ProviderModelCatalog {
                        provider: name.into(),
                        connection_key: connection_key(provider),
                        models: models.into_values().collect(),
                    });
                }
                let cursor = value["last_id"]
                    .as_str()
                    .or_else(|| data.last().and_then(|m| m["id"].as_str()))
                    .filter(|s| !s.is_empty() && s.len() <= 512)
                    .ok_or_else(|| ProviderError::protocol("模型列表分页缺少游标"))?;
                if !cursors.insert(cursor.to_string()) {
                    return Err(ProviderError::protocol("模型列表分页游标重复"));
                }
                after = Some(cursor.to_string());
            }
            Err(ProviderError::protocol("模型列表超过 20 页上限"))
        };
        let catalog = tokio::select! {biased;_=cancel.cancelled()=>Err(ProviderError::cancelled()),result=tokio::time::timeout(Duration::from_millis(provider.transport.request_timeout_ms.min(30_000)),work)=>result.unwrap_or_else(|_|Err(ProviderError::new(ProviderErrorKind::Timeout,"模型列表查询超时")))}?;
        // Discovery success does not depend on a writable cache.
        let _ = self.cache_catalog(&catalog);
        Ok(catalog)
    }
}

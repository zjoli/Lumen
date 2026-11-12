use futures_util::StreamExt;
use lumen::{agent::*, provider::*};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
fn c() -> CancellationToken {
    CancellationToken::new()
}
struct Reply {
    status: u16,
    kind: &'static str,
    body: Vec<u8>,
}
impl Reply {
    fn json(value: Value) -> Self {
        Self {
            status: 200,
            kind: "application/json",
            body: serde_json::to_vec(&value).unwrap(),
        }
    }
}
struct MemoryHttp {
    replies: Mutex<VecDeque<Reply>>,
    requests: Mutex<Vec<(&'static str, HttpRequest)>>,
}
impl MemoryHttp {
    fn new(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(vec![]),
        })
    }
    fn reply(
        &self,
        method: &'static str,
        request: HttpRequest,
    ) -> Result<HttpResponse, ProviderError> {
        self.requests.lock().unwrap().push((method, request));
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| ProviderError::protocol("unexpected request"))?;
        Ok(HttpResponse {
            status: reply.status,
            headers: BTreeMap::from([("content-type".into(), reply.kind.into())]),
            body: Box::pin(futures_util::stream::iter(vec![Ok(reply.body)])),
        })
    }
}
#[async_trait::async_trait]
impl HttpTransport for MemoryHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ProviderError> {
        self.reply("POST", request)
    }
    async fn get(&self, request: HttpRequest) -> Result<HttpResponse, ProviderError> {
        self.reply("GET", request)
    }
}
fn registration(provider: &str, model: &str, alias: &str) -> RegisterModel {
    RegisterModel {
        provider: provider.into(),
        model_id: model.into(),
        alias: alias.into(),
        context_tokens: 64000,
        output_tokens: 1024,
        tools: true,
        make_default: true,
        connection_key: None,
    }
}

#[tokio::test]
async fn deepseek_key_discovery_registration_and_chat_work_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let key = "test-only-provider-key";
    let first = ProviderRegistry::new(dir.path());
    let mut connection = provider_preset("deepseek").unwrap().connection();
    connection.transport.mode = TransportMode::Json;
    first
        .add_provider("deepseek".into(), connection, Some(key))
        .unwrap();
    assert!(matches!(
        first.default_selection().unwrap(),
        ModelSelection::Mock
    ));
    let config = std::fs::read_to_string(dir.path().join(".lumen/providers.yaml")).unwrap();
    assert!(!config.contains(key));
    assert!(!first.providers().unwrap().to_string().contains(key));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join(".lumen/provider-credentials.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let http = MemoryHttp::new(vec![
        Reply::json(
            json!({"data":[{"id":"deepseek-current","name":"Current DeepSeek","context_window":1048576,"max_output_tokens":32768}]}),
        ),
        Reply::json(
            json!({"id":"test-chat","choices":[{"index":0,"message":{"role":"assistant","content":"真实配置路径已运行"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}),
        ),
    ]);
    let registry = ProviderRegistry::with_transport(dir.path(), http.clone());
    let catalog = registry.discover_models("deepseek", c()).await.unwrap();
    assert_eq!(catalog.models[0].context_tokens, Some(1048576));
    assert_eq!(catalog.models[0].max_output_tokens, Some(32768));
    let mut request = registration("deepseek", "deepseek-current", "ds");
    request.connection_key = Some(catalog.connection_key);
    registry.register_model(request).unwrap();
    let resolved = registry
        .resolve(&registry.default_selection().unwrap(), c())
        .await
        .unwrap();
    assert_eq!(resolved.descriptor.model_id, "deepseek-current");
    let mut stream = resolved
        .provider
        .stream(
            ModelRequest {
                messages: vec![AgentMessage::User(vec![ContentPart::Text("hello".into())])],
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    let mut completed = false;
    while let Some(event) = stream.next().await {
        if matches!(event.unwrap(), ModelEvent::Completed(_)) {
            completed = true;
        }
    }
    assert!(completed);
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests[0].0, "GET");
    assert_eq!(requests[0].1.url, "https://api.deepseek.com/models");
    assert!(requests[0].1.body.is_empty());
    assert_eq!(requests[1].0, "POST");
    assert_eq!(
        requests[1].1.url,
        "https://api.deepseek.com/chat/completions"
    );
    assert!(requests.iter().all(|(_, r)| r
        .headers
        .iter()
        .any(|(k, v)| k == "authorization" && v == &format!("Bearer {key}"))));
}

#[tokio::test]
async fn custom_discovery_keeps_prefixes_paginates_and_does_not_guess_missing_capacities() {
    let dir = tempfile::tempdir().unwrap();
    let http = MemoryHttp::new(vec![
        Reply::json(json!({"data":[{"id":"a/model"}],"has_more":true,"last_id":"a/model"})),
        Reply::json(json!({"data":[{"id":"z","context_length":128000}],"has_more":false})),
    ]);
    let registry = ProviderRegistry::with_transport(dir.path(), http.clone());
    registry
        .add_provider(
            "local".into(),
            compatible_connection("https://gateway.test/prefix/v1/".into(), AuthConfig::None),
            None,
        )
        .unwrap();
    let catalog = registry.discover_models("local", c()).await.unwrap();
    assert_eq!(catalog.models.len(), 2);
    assert_eq!(catalog.models[0].id, "a/model");
    assert_eq!(catalog.models[0].context_tokens, None);
    assert_eq!(catalog.models[1].context_tokens, Some(128000));
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let url = reqwest::Url::parse(&requests[1].1.url).unwrap();
    assert_eq!(url.path(), "/prefix/v1/models");
    assert_eq!(url.query_pairs().collect::<Vec<_>>()[0].1, "a/model");
    assert!(requests.iter().all(
        |(method, r)| *method == "GET" && !r.headers.iter().any(|(k, _)| k == "authorization")
    ));
}

#[tokio::test]
async fn provider_edits_preserve_existing_aliases_and_reject_duplicates_invalid_values_and_stale_catalogs(
) {
    let dir = tempfile::tempdir().unwrap();
    let registry = ProviderRegistry::new(dir.path());
    registry
        .add_provider(
            "one".into(),
            compatible_connection("https://one.test/v1".into(), AuthConfig::None),
            None,
        )
        .unwrap();
    registry
        .register_model(registration("one", "model-one", "first"))
        .unwrap();
    let before = std::fs::read(dir.path().join(".lumen/providers.yaml")).unwrap();
    assert!(registry
        .add_provider(
            "one".into(),
            provider_preset("deepseek").unwrap().connection(),
            None
        )
        .is_err());
    let mut invalid = registration("one", "model-two", "second");
    invalid.output_tokens = invalid.context_tokens;
    assert!(registry.register_model(invalid).is_err());
    assert_eq!(
        std::fs::read(dir.path().join(".lumen/providers.yaml")).unwrap(),
        before
    );
    registry
        .add_provider(
            "two".into(),
            compatible_connection("https://two.test/v1".into(), AuthConfig::None),
            None,
        )
        .unwrap();
    assert!(registry
        .config()
        .unwrap()
        .unwrap()
        .models
        .contains_key("first"));
    assert!(registry
        .register_model(registration("two", "different", "first"))
        .is_err());
    let mut stale = registration("one", "model-new", "stale");
    stale.connection_key = Some("old-key".into());
    assert!(registry.register_model(stale).is_err());
    assert!(!registry
        .config()
        .unwrap()
        .unwrap()
        .models
        .contains_key("stale"));
    let mut connection = provider_preset("deepseek").unwrap().connection();
    connection.base_url = "https://user:secret@example.test/v1".into();
    assert!(registry
        .add_provider("bad".into(), connection, Some("fake-key"))
        .is_err());
    assert!(registry.set_api_key("missing", "fake-key").is_err());
    assert!(!dir.path().join(".lumen/provider-credentials.json").exists());
}

#[tokio::test]
async fn rotating_a_local_key_takes_effect_without_hiding_history_or_exposing_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let http = MemoryHttp::new(vec![Reply::json(json!({"data":[]}))]);
    let registry = ProviderRegistry::with_transport(dir.path(), http.clone());
    registry
        .add_provider(
            "custom".into(),
            compatible_connection("https://local.test/v1".into(), AuthConfig::None),
            Some("old-test-key"),
        )
        .unwrap();
    registry.set_api_key("custom", "new-test-key").unwrap();
    registry.discover_models("custom", c()).await.unwrap();
    assert!(http.requests.lock().unwrap()[0]
        .1
        .headers
        .iter()
        .any(|(k, v)| k == "authorization" && v == "Bearer new-test-key"));
    std::fs::remove_file(dir.path().join(".lumen/provider-credentials.json")).unwrap();
    assert!(registry.providers().is_ok());
    assert!(registry.models().is_ok());
    assert!(registry.discover_models("custom", c()).await.is_err());
    assert_eq!(http.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn catalog_errors_are_bounded_redacted_and_never_retried() {
    for reply in [
        Reply {
            status: 401,
            kind: "application/json",
            body: b"upstream-leaked-secret".to_vec(),
        },
        Reply {
            status: 404,
            kind: "application/json",
            body: vec![],
        },
        Reply {
            status: 200,
            kind: "text/html",
            body: b"upstream-leaked-secret".to_vec(),
        },
        Reply::json(json!({"data":[{"name":"no id"}]})),
        Reply {
            status: 200,
            kind: "application/json",
            body: vec![b' '; 2 * 1024 * 1024 + 1],
        },
    ] {
        let dir = tempfile::tempdir().unwrap();
        let http = MemoryHttp::new(vec![reply]);
        let registry = ProviderRegistry::with_transport(dir.path(), http.clone());
        registry
            .add_provider(
                "custom".into(),
                compatible_connection("https://local.test/v1".into(), AuthConfig::None),
                None,
            )
            .unwrap();
        let error = registry.discover_models("custom", c()).await.unwrap_err();
        assert!(!error.to_string().contains("upstream-leaked-secret"));
        assert_eq!(http.requests.lock().unwrap().len(), 1);
        assert!(registry
            .config()
            .unwrap()
            .unwrap()
            .providers
            .contains_key("custom"));
    }
}
struct NeverHttp;
#[async_trait::async_trait]
impl HttpTransport for NeverHttp {
    async fn send(&self, _: HttpRequest) -> Result<HttpResponse, ProviderError> {
        panic!("discovery must not POST")
    }
    async fn get(&self, _: HttpRequest) -> Result<HttpResponse, ProviderError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn discovery_can_be_cancelled_and_has_an_absolute_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let registry = ProviderRegistry::with_transport(dir.path(), Arc::new(NeverHttp));
    let mut connection = compatible_connection("https://local.test/v1".into(), AuthConfig::None);
    connection.transport.request_timeout_ms = 100;
    registry
        .add_provider("custom".into(), connection, None)
        .unwrap();
    let cancel = c();
    let sender = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        sender.cancel();
    });
    assert_eq!(
        registry
            .discover_models("custom", cancel)
            .await
            .unwrap_err()
            .kind,
        ProviderErrorKind::Cancelled
    );
    assert_eq!(
        registry
            .discover_models("custom", c())
            .await
            .unwrap_err()
            .kind,
        ProviderErrorKind::Timeout
    );
}

#[test]
fn provider_catalog_merges_data_presets_and_user_routes_without_replacing_connections() {
    let dir = tempfile::tempdir().unwrap();
    let registry = ProviderRegistry::new(dir.path());
    let presets = provider_presets();
    let mut ids = std::collections::BTreeSet::new();
    for preset in &presets {
        assert!(ids.insert(preset.id.clone()));
        ProviderConfig {
            schema_version: 1,
            default_model: String::new(),
            providers: BTreeMap::from([(preset.id.clone(), preset.connection())]),
            models: BTreeMap::new(),
        }
        .validate()
        .unwrap();
    }
    assert!(ids.contains("openai") && ids.contains("anthropic") && ids.contains("ollama"));
    registry
        .add_provider(
            "openai".into(),
            compatible_connection("https://company.test/private/v1".into(), AuthConfig::None),
            None,
        )
        .unwrap();
    registry
        .add_provider(
            "company".into(),
            protocol_connection(
                ProtocolKind::AnthropicMessages,
                "https://company.test/claude/v1".into(),
                AuthConfig::Env {
                    variable: "LUMEN_TEST_MISSING_CATALOG_KEY".into(),
                },
            ),
            None,
        )
        .unwrap();
    let catalog = registry.provider_catalog().unwrap();
    let openai = catalog.iter().find(|p| p.id == "openai").unwrap();
    assert!(openai.configured && openai.authenticated);
    assert_eq!(
        openai.connection.base_url,
        "https://company.test/private/v1"
    );
    assert_eq!(openai.connection.protocol, ProtocolKind::OpenaiChat);
    let company = catalog.iter().find(|p| p.id == "company").unwrap();
    assert!(company.configured && !company.authenticated);
    assert_eq!(company.connection.protocol, ProtocolKind::AnthropicMessages);
    assert_eq!(catalog.len(), presets.len() + 1);
}

#[tokio::test]
async fn discovered_models_survive_restart_and_stale_or_corrupt_caches_do_not_replace_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let http = MemoryHttp::new(vec![Reply::json(
        json!({"data":[{"id":"first"},{"id":"second"},{"id":"third"}]}),
    )]);
    let registry = ProviderRegistry::with_transport(dir.path(), http.clone());
    registry
        .add_provider(
            "company".into(),
            compatible_connection("https://company.test/v1".into(), AuthConfig::None),
            Some("key-one"),
        )
        .unwrap();
    registry.discover_models("company", c()).await.unwrap();
    let offline = ProviderRegistry::new(dir.path());
    let available = offline.available_models().unwrap();
    assert_eq!(available.len(), 3);
    assert!(available
        .iter()
        .all(|m| m.alias.is_none() && m.model.context_tokens.is_none()));
    offline
        .register_model(registration("company", "first", "research"))
        .unwrap();
    let available = offline.available_models().unwrap();
    assert_eq!(available.len(), 3);
    assert_eq!(available[0].alias.as_deref(), Some("research"));
    let mut another = registration("company", "second", "quick");
    another.make_default = false;
    offline.register_model(another).unwrap();
    assert_eq!(offline.config().unwrap().unwrap().default_model, "research");
    offline.set_api_key("company", "key-two").unwrap();
    assert_eq!(offline.available_models().unwrap().len(), 2); // configured aliases remain
    assert!(offline
        .available_models()
        .unwrap()
        .iter()
        .all(|m| m.alias.is_some()));
    std::fs::write(dir.path().join(".lumen/model-cache.json"), b"invalid cache").unwrap();
    assert_eq!(offline.available_models().unwrap().len(), 2);
    offline.logout("company").unwrap();
    assert!(offline.available_models().unwrap().is_empty());
    assert_eq!(offline.config().unwrap().unwrap().models.len(), 2);
    assert_eq!(http.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn anthropic_model_discovery_uses_its_protocol_auth_and_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let http = MemoryHttp::new(vec![
        Reply::json(
            json!({"data":[{"id":"claude-a","display_name":"Claude A"}],"has_more":true,"last_id":"claude-a"}),
        ),
        Reply::json(json!({"data":[{"id":"claude-b"}],"has_more":false})),
    ]);
    let registry = ProviderRegistry::with_transport(dir.path(), http.clone());
    registry
        .add_provider(
            "anthropic".into(),
            provider_preset("anthropic").unwrap().connection(),
            Some("test-anthropic-key"),
        )
        .unwrap();
    let catalog = registry.discover_models("anthropic", c()).await.unwrap();
    assert_eq!(catalog.models[0].name.as_deref(), Some("Claude A"));
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests[0].1.url, "https://api.anthropic.com/v1/models");
    let url = reqwest::Url::parse(&requests[1].1.url).unwrap();
    assert_eq!(
        url.query_pairs().collect::<Vec<_>>(),
        vec![("after_id".into(), "claude-a".into())]
    );
    for (_, request) in requests.iter() {
        assert!(request
            .headers
            .iter()
            .any(|(k, v)| k == "x-api-key" && v == "test-anthropic-key"));
        assert!(request
            .headers
            .iter()
            .any(|(k, v)| k == "anthropic-version" && v == "2023-06-01"));
        assert!(!request.headers.iter().any(|(k, _)| k == "authorization"));
    }
}

#[test]
fn rotation_logout_and_switching_auth_remove_unused_keys_and_preserve_models_and_routes() {
    let dir = tempfile::tempdir().unwrap();
    let registry = ProviderRegistry::new(dir.path());
    registry
        .add_provider(
            "openai".into(),
            compatible_connection("https://private.test/v1".into(), AuthConfig::None),
            Some("old-key"),
        )
        .unwrap();
    registry
        .register_model(registration("openai", "company-model", "company"))
        .unwrap();
    registry.set_api_key("openai", "new-key").unwrap();
    let secrets =
        || std::fs::read_to_string(dir.path().join(".lumen/provider-credentials.json")).unwrap();
    assert!(!secrets().contains("old-key"));
    registry.logout("openai").unwrap();
    assert!(!secrets().contains("new-key"));
    let config = registry.config().unwrap().unwrap();
    assert_eq!(
        config.providers["openai"].base_url,
        "https://private.test/v1"
    );
    assert_eq!(
        config.providers["openai"].protocol,
        ProtocolKind::OpenaiChat
    );
    assert!(
        matches!(&config.providers["openai"].auth, AuthConfig::Env { variable } if variable == "OPENAI_API_KEY")
    );
    assert_eq!(config.default_model, "company");
    registry.set_api_key("openai", "other-key").unwrap();
    registry
        .set_provider_auth(
            "openai",
            AuthConfig::Env {
                variable: "COMPANY_KEY".into(),
            },
        )
        .unwrap();
    assert!(!secrets().contains("other-key"));
    registry.logout("openai").unwrap();
    assert!(
        matches!(&registry.config().unwrap().unwrap().providers["openai"].auth, AuthConfig::Env { variable } if variable == "COMPANY_KEY")
    );
}

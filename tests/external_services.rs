use lumen::{agent::*, knowledge::*, provider::*, retrieval::*, services::*, workspace::*};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

fn c() -> CancellationToken {
    CancellationToken::new()
}
fn endpoint(path: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        endpoint: format!("https://services.example.test/{path}"),
        auth: AuthConfig::None,
        timeout_ms: 1000,
    }
}
struct MemoryHttp {
    replies: Mutex<VecDeque<Value>>,
    requests: Mutex<Vec<HttpRequest>>,
    dynamic_vectors: bool,
}
impl MemoryHttp {
    fn new(replies: Vec<Value>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(vec![]),
            dynamic_vectors: false,
        })
    }
    fn dynamic() -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(VecDeque::new()),
            requests: Mutex::new(vec![]),
            dynamic_vectors: true,
        })
    }
}
#[async_trait::async_trait]
impl HttpTransport for MemoryHttp {
    async fn send(&self, r: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let reply = if self.dynamic_vectors {
            let body: Value = serde_json::from_slice(&r.body).unwrap();
            if r.url.ends_with("embeddings") {
                let input = body["input"].as_array().unwrap();
                assert!(input.iter().all(|s| !s.as_str().unwrap().trim().is_empty()));
                json!({"data":input.iter().enumerate().rev().map(|(i,_)|json!({"index":i,"embedding":[1.0,0.5]})).collect::<Vec<_>>()})
            } else {
                let len = body["documents"].as_array().unwrap().len();
                json!({"results":(0..len).rev().map(|i|json!({"index":i,"relevance_score":(i+1) as f64/len as f64})).collect::<Vec<_>>()})
            }
        } else {
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| ProviderError::protocol("unexpected request"))?
        };
        self.requests.lock().unwrap().push(r);
        Ok(HttpResponse {
            status: 200,
            headers: BTreeMap::from([(
                "content-type".into(),
                "application/json; charset=utf-8".into(),
            )]),
            body: Box::pin(futures_util::stream::iter(vec![Ok(serde_json::to_vec(
                &reply,
            )
            .unwrap())])),
        })
    }
}
async fn workspace() -> (tempfile::TempDir, WorkspaceHandle) {
    let dir = tempfile::tempdir().unwrap();
    let w = WorkspaceService::init(
        InitRequest {
            root: dir.path().join("knowledge"),
            author_name: "test".into(),
            author_email: "test@example.test".into(),
        },
        c(),
    )
    .await
    .unwrap();
    (dir, w)
}

#[tokio::test]
async fn web_search_encodes_queries_filters_results_and_imports_attributed_snapshots() {
    let (_dir, w) = workspace().await;
    let http = MemoryHttp::new(vec![
        json!({"results":[{"title":"论文","url":"https://papers.example.test/a","content":"研究摘要"},{"title":"重复","url":"https://papers.example.test/a","content":"duplicate"},{"title":"无效链接","url":"file:///etc/passwd"}]}),
        json!({"data":{"title":"论文全文","content":"# 结论\n\n经实验验证的内容。"}}),
    ]);
    let config = ServicesConfig {
        schema_version: 1,
        search: Some(SearchConfig {
            connection: endpoint("search"),
            protocol: SearchProtocol::Searxng,
        }),
        reader: Some(endpoint("reader")),
        ..Default::default()
    };
    let service = ResearchService::new(config, ServiceHttp::with_transport(http.clone()));
    let query = "事务 & SQL + 中文";
    let results = service.search(query.into(), 8, true, c()).await.unwrap();
    assert_eq!(results.results.len(), 1);
    {
        let requests = http.requests.lock().unwrap();
        assert!(requests[0]
            .headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/x-www-form-urlencoded"));
        let form = reqwest::Url::parse(&format!(
            "https://form.test/?{}",
            std::str::from_utf8(&requests[0].body).unwrap()
        ))
        .unwrap();
        let pairs = form.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
        assert_eq!(pairs["q"], query);
        assert_eq!(pairs["categories"], "science");
    }
    let saved = service
        .import(w.clone(), results.results[0].url.clone(), None, None, c())
        .await
        .unwrap();
    assert_eq!(saved.metadata.kind, ObjectKind::Resource);
    assert_eq!(
        saved.metadata.origin.as_ref().unwrap().uri,
        "https://papers.example.test/a"
    );
    assert!(saved
        .metadata
        .origin
        .as_ref()
        .unwrap()
        .captured_at
        .contains('T'));
    assert!(saved.content.contains("经实验验证的内容"));
    assert!(saved.content.contains("https://papers.example.test/a"));
    assert!(w
        .retrieval()
        .search(
            SearchRequest {
                query: "实验".into(),
                ..Default::default()
            },
            c()
        )
        .await
        .unwrap()
        .hits
        .iter()
        .any(|h| h.id == saved.metadata.id.as_str()));
    assert!(
        w.validate(ValidationScope::WorkingTree, c())
            .await
            .unwrap()
            .valid
    );
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn jina_search_and_reader_limits_preserve_url_and_truncation_metadata() {
    let http = MemoryHttp::new(vec![
        json!({"data":[{"title":"Source","url":"https://example.test/paper","description":"summary"}]}),
        json!({"data":{"title":"long","content":"文".repeat(100_001)}}),
    ]);
    let service = ResearchService::new(
        ServicesConfig {
            schema_version: 1,
            search: Some(SearchConfig {
                connection: endpoint("search"),
                protocol: SearchProtocol::Jina,
            }),
            reader: Some(endpoint("reader")),
            ..Default::default()
        },
        ServiceHttp::with_transport(http.clone()),
    );
    let results = service.search("topic".into(), 3, false, c()).await.unwrap();
    assert_eq!(results.results[0].snippet, "summary");
    let page = service
        .fetch(results.results[0].url.clone(), c())
        .await
        .unwrap();
    assert!(page.truncated);
    assert_eq!(page.content.chars().count(), 100_000);
    let body: Value = serde_json::from_slice(&http.requests.lock().unwrap()[0].body).unwrap();
    assert_eq!(body["q"], "topic");
    assert_eq!(body["num"], 3);
    assert!(service
        .fetch("file:///etc/passwd".into(), c())
        .await
        .is_err());
}

#[tokio::test]
async fn embeddings_reorder_indices_validate_shape_and_separate_configuration_namespaces() {
    let config = EmbeddingConfig {
        connection: endpoint("embeddings"),
        model: "model".into(),
        dimensions: Some(2),
        state_scope: "account-a".into(),
    };
    let http = MemoryHttp::new(vec![
        json!({"data":[{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[1.0,0.0]}]}),
    ]);
    let embedder = HttpEmbedder::new(config.clone(), ServiceHttp::with_transport(http.clone()));
    let batch = embedder
        .embed(vec!["first".into(), "second".into()], c())
        .await
        .unwrap();
    assert_eq!(batch.vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    assert_eq!(batch.model_id, embedder.model_id());
    let body: Value = serde_json::from_slice(&http.requests.lock().unwrap()[0].body).unwrap();
    assert_eq!(body["encoding_format"], "float");
    assert_eq!(body["dimensions"], 2);
    let mut another = config.clone();
    another.state_scope = "account-b".into();
    assert_ne!(
        HttpEmbedder::new(another, ServiceHttp::with_transport(http)).model_id(),
        embedder.model_id()
    );
    for data in [
        json!([{"index":0,"embedding":[1.0,0.0]},{"index":0,"embedding":[1.0,0.0]}]),
        json!([{"index":0,"embedding":[1.0]}]),
        json!([{"index":0,"embedding":[0.0,0.0]}]),
    ] {
        let bad = HttpEmbedder::new(
            config.clone(),
            ServiceHttp::with_transport(MemoryHttp::new(vec![json!({"data":data})])),
        );
        assert!(bad.embed(vec!["first".into()], c()).await.is_err());
    }
}

#[tokio::test]
async fn configured_embedding_and_reranker_run_through_the_real_index_and_search_pipeline() {
    let (_dir, w) = workspace().await;
    // An empty body is legal knowledge and still needs a useful embedding input.
    let object = w
        .knowledge()
        .create(
            KnowledgeObject {
                metadata: ObjectMetadata::new(ObjectKind::Concept, "所有权"),
                body: String::new(),
            },
            c(),
        )
        .await
        .unwrap();
    let http = MemoryHttp::dynamic();
    let config = ServicesConfig {
        schema_version: 1,
        embedding: Some(EmbeddingConfig {
            connection: endpoint("embeddings"),
            model: "embedding-model".into(),
            dimensions: Some(2),
            state_scope: "test".into(),
        }),
        reranker: Some(RerankConfig {
            connection: endpoint("rerank"),
            model: "rerank-model".into(),
        }),
        ..Default::default()
    };
    config
        .attach(&w, ServiceHttp::with_transport(http.clone()))
        .unwrap();
    let status = w
        .index()
        .rebuild(IndexSelection::Vectors, c())
        .await
        .unwrap();
    assert!(status.semantic_ready);
    let results = w
        .retrieval()
        .search(
            SearchRequest {
                query: "借用规则".into(),
                mode: SearchMode::Semantic,
                rerank: true,
                ..Default::default()
            },
            c(),
        )
        .await
        .unwrap();
    assert!(results
        .hits
        .iter()
        .any(|h| h.id == object.metadata.id.as_str()));
    assert!(results.degraded.is_empty());
    assert!(http
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.url.ends_with("rerank")));
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn reranker_rejects_duplicate_missing_and_out_of_range_indices() {
    for items in [
        json!([{"index":0,"relevance_score":0.8}]),
        json!([{"index":0,"relevance_score":0.8},{"index":0,"relevance_score":0.6}]),
        json!([{"index":0,"relevance_score":0.8},{"index":3,"relevance_score":0.6}]),
    ] {
        let reranker = HttpReranker::new(
            RerankConfig {
                connection: endpoint("rerank"),
                model: "model".into(),
            },
            ServiceHttp::with_transport(MemoryHttp::new(vec![json!({"results":items})])),
        );
        assert!(reranker
            .rerank("query".into(), vec!["one".into(), "two".into()], c())
            .await
            .is_err());
    }
}

struct NeverHttp;
#[async_trait::async_trait]
impl HttpTransport for NeverHttp {
    async fn send(&self, _: HttpRequest) -> Result<HttpResponse, ProviderError> {
        std::future::pending().await
    }
}
struct RawHttp {
    status: u16,
    kind: &'static str,
    body: Vec<u8>,
}
#[async_trait::async_trait]
impl HttpTransport for RawHttp {
    async fn send(&self, _: HttpRequest) -> Result<HttpResponse, ProviderError> {
        Ok(HttpResponse {
            status: self.status,
            headers: BTreeMap::from([("content-type".into(), self.kind.into())]),
            body: Box::pin(futures_util::stream::iter(vec![Ok(self.body.clone())])),
        })
    }
}
#[tokio::test]
async fn services_enforce_cancellation_deadlines_response_limits_and_redacted_errors() {
    let http = ServiceHttp::with_transport(Arc::new(NeverHttp));
    let cancel = c();
    let sender = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        sender.cancel();
    });
    assert!(matches!(
        http.json(&endpoint("search"), json!({}), cancel).await,
        Err(WorkspaceError::Cancelled)
    ));
    let mut short = endpoint("search");
    short.timeout_ms = 100;
    assert!(matches!(
        http.json(&short, json!({}), c()).await,
        Err(WorkspaceError::Network)
    ));
    for raw in [
        RawHttp {
            status: 401,
            kind: "application/json",
            body: b"secret upstream error".to_vec(),
        },
        RawHttp {
            status: 200,
            kind: "text/html",
            body: b"secret upstream error".to_vec(),
        },
        RawHttp {
            status: 200,
            kind: "application/json",
            body: vec![b' '; 16 * 1024 * 1024 + 1],
        },
    ] {
        let error = ServiceHttp::with_transport(Arc::new(raw))
            .json(&endpoint("test"), json!({}), c())
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }
}

#[tokio::test]
async fn missing_credentials_do_not_prevent_config_loading_or_local_reads() {
    let (_dir, w) = workspace().await;
    let missing = format!("LUMEN_ABSENT_{}", uuid::Uuid::now_v7().simple());
    let value = json!({"schema_version":1,"embedding":{"connection":{"endpoint":"https://example.test/embeddings","auth":{"kind":"env","variable":missing}},"model":"model"}});
    std::fs::write(
        w.root().join(".lumen/services.yaml"),
        serde_yaml_ng::to_string(&value).unwrap(),
    )
    .unwrap();
    let config = ServicesConfig::load(w.root()).unwrap();
    assert!(!config.describe().to_string().contains(&missing));
    let http = MemoryHttp::new(vec![]);
    config
        .attach(&w, ServiceHttp::with_transport(http.clone()))
        .unwrap();
    assert!(w
        .knowledge()
        .list(ObjectFilter::default(), PageRequest::default(), c())
        .await
        .is_ok());
    let embedder = HttpEmbedder::new(
        config.embedding.unwrap(),
        ServiceHttp::with_transport(http.clone()),
    );
    assert!(matches!(
        embedder.embed(vec!["text".into()], c()).await,
        Err(WorkspaceError::AuthRequired)
    ));
    assert!(http.requests.lock().unwrap().is_empty());
    for yaml in [
        "schema_version: 2",
        "schema_version: 1\nunknown: true",
        "schema_version: 1\nschema_version: 1",
        "schema_version: 1\nreader:\n  endpoint: https://example.test/?secret=bad",
    ] {
        std::fs::write(w.root().join(".lumen/services.yaml"), yaml).unwrap();
        assert!(ServicesConfig::load(w.root()).is_err());
    }
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn summary_adapter_uses_configured_model_and_persistent_cache() {
    let (_dir, w) = workspace().await;
    let model_config = json!({"schema_version":1,"default_model":"summary","providers":{"test":{"protocol":"openai_chat","base_url":"https://model.example.test/v1","state_scope":"test","auth":{"kind":"none"},"transport":{"mode":"json"}}},"models":{"summary":{"provider":"test","model_id":"summary-model","capabilities":{"context_tokens":64000,"max_output_tokens":1024},"defaults":{"max_output_tokens":256}}}});
    std::fs::write(
        w.root().join(".lumen/providers.yaml"),
        serde_yaml_ng::to_string(&model_config).unwrap(),
    )
    .unwrap();
    let response = json!({"id":"summary-response","model":"summary-model","choices":[{"index":0,"message":{"role":"assistant","content":"保留来源与边界的摘要。"},"finish_reason":"stop"}],"usage":{"prompt_tokens":40,"completion_tokens":12}});
    let http = MemoryHttp::new(vec![response]);
    let registry = Arc::new(ProviderRegistry::with_transport(w.root(), http.clone()));
    let selection = registry.default_selection().unwrap();
    let descriptor = registry.describe(&selection).unwrap();
    let object = w
        .knowledge()
        .create(
            KnowledgeObject {
                metadata: ObjectMetadata::new(ObjectKind::Concept, "知识"),
                body: "完整材料".into(),
            },
            c(),
        )
        .await
        .unwrap();
    let generator = Arc::new(ModelSummary::new(
        registry,
        selection,
        descriptor.configuration_key,
    ));
    let text = generator
        .generate(
            SummaryInput {
                title: "知识".into(),
                content: "完整材料".into(),
                level: 0,
                max_chars: 256,
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(text, "保留来源与边界的摘要。");
    // Use a deterministic provider for the storage path without racing automatic maintenance.
    struct CachedSummary;
    #[async_trait::async_trait]
    impl SummaryGenerator for CachedSummary {
        fn id(&self) -> &str {
            "cached-test"
        }
        async fn generate(&self, _: SummaryInput, _: CancellationToken) -> WorkspaceResult<String> {
            Ok("缓存摘要".into())
        }
    }
    w.set_summary_generator(Some(Arc::new(CachedSummary)));
    let request = SummaryRequest {
        id: object.metadata.id,
        level: 0,
        max_chars: Some(256),
    };
    let saved = w.summaries().generate(request.clone(), c()).await.unwrap();
    assert!(saved.generated);
    assert_eq!(
        w.summaries().read(request, c()).await.unwrap().content,
        "缓存摘要"
    );
    {
        let requests = http.requests.lock().unwrap();
        let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(body["model"], "summary-model");
        assert_eq!(body["max_tokens"], 256);
    }
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn agent_can_search_and_save_fetched_sources_through_registered_tools() {
    let (_dir, w) = workspace().await;
    let http = MemoryHttp::new(vec![
        json!({"results":[{"title":"原始来源","url":"https://example.test/source","content":"摘要"}]}),
        json!({"data":{"title":"原始来源","content":"从原始来源抓取的正文"}}),
    ]);
    let service = ResearchService::new(
        ServicesConfig {
            schema_version: 1,
            search: Some(SearchConfig {
                connection: endpoint("search"),
                protocol: SearchProtocol::Searxng,
            }),
            reader: Some(endpoint("reader")),
            ..Default::default()
        },
        ServiceHttp::with_transport(http),
    );
    let call = |id: &str, name: &str, args: Value| AssistantMessage {
        tool_calls: vec![ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args,
        }],
        ..Default::default()
    };
    let provider = MockProvider::new(vec![
        call("search", "web_search", json!({"query":"research topic"})),
        call(
            "import",
            "web_import",
            json!({"url":"https://example.test/source"}),
        ),
        AssistantMessage {
            content: vec![ContentPart::Text("资料已保存".into())],
            ..Default::default()
        },
    ]);
    let mut agent = AgentLoop::new(Arc::new(provider));
    service.register_tools(&mut agent, Some(w.clone()));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    let result = agent
        .run(
            vec![ContentPart::Text("检索并保存来源".into())],
            &mut state,
            c(),
        )
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::Completed);
    assert_eq!(
        state
            .messages
            .iter()
            .filter(|m| matches!(m,AgentMessage::ToolResult(r) if !r.is_error))
            .count(),
        2
    );
    let resources = w
        .knowledge()
        .list(
            ObjectFilter {
                kinds: vec![ObjectKind::Resource],
                ..Default::default()
            },
            PageRequest::default(),
            c(),
        )
        .await
        .unwrap();
    assert_eq!(resources.items.len(), 1);
    assert_eq!(
        resources.items[0].origin.as_ref().unwrap().uri,
        "https://example.test/source"
    );
    w.close(c()).await.unwrap();
}

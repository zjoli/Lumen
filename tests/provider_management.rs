use futures_util::StreamExt;
use lumen::{agent::*, context::*, provider::*, session::*};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
fn c() -> CancellationToken {
    CancellationToken::new()
}
fn text(s: &str) -> Vec<ContentPart> {
    vec![ContentPart::Text(s.into())]
}
fn selection() -> ModelSelection {
    ModelSelection::Reference {
        model_ref: "test".into(),
        overrides: Default::default(),
    }
}
fn config(base: &str, protocol: ProtocolKind, mode: TransportMode) -> Value {
    json!({"schema_version":1,"default_model":"test","providers":{"local":{"protocol":protocol,"base_url":base,"state_scope":"test-account","auth":{"kind":"none"},"transport":{"mode":mode},"protocol_options":{}}},"models":{"test":{"provider":"local","model_id":"test-model","capabilities":{"context_tokens":64000,"max_output_tokens":4096,"tools":true,"tool_choice":["auto","none","required","named"],"temperature":true},"defaults":{"max_output_tokens":512}}}})
}
fn write_config(root: &Path, value: &Value) {
    std::fs::create_dir_all(root.join(".lumen")).unwrap();
    std::fs::write(
        root.join(".lumen/providers.yaml"),
        serde_yaml_ng::to_string(value).unwrap(),
    )
    .unwrap();
}
#[derive(Debug)]
struct Request {
    path: String,
    headers: String,
    body: Vec<u8>,
}
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    fragment: usize,
    delay: Duration,
    interval: Duration,
    headers: String,
}
impl Reply {
    fn sse(body: String) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream",
            body: body.into_bytes(),
            fragment: 1,
            delay: Duration::ZERO,
            interval: Duration::ZERO,
            headers: String::new(),
        }
    }
    fn json(body: Value) -> Self {
        Self {
            content_type: "application/json",
            body: serde_json::to_vec(&body).unwrap(),
            fragment: usize::MAX,
            ..Self::sse(String::new())
        }
    }
}
struct MemoryHttp {
    replies: std::sync::Mutex<std::collections::VecDeque<Reply>>,
    requests: mpsc::UnboundedSender<Request>,
}
#[async_trait::async_trait]
impl HttpTransport for MemoryHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ProviderError> {
        let url = reqwest::Url::parse(&request.url).unwrap();
        let headers = request
            .headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect::<String>();
        let _ = self.requests.send(Request {
            path: url.path().into(),
            headers,
            body: request.body.to_vec(),
        });
        let reply = self.replies.lock().unwrap().pop_front().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Transport,
                "unexpected extra HTTP request",
            )
        })?;
        let mut headers = std::collections::BTreeMap::from([
            ("content-type".into(), reply.content_type.into()),
            ("x-request-id".into(), "req-test".into()),
        ]);
        for line in reply.headers.lines() {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.to_ascii_lowercase(), v.trim().into());
            }
        }
        let status = reply.status;
        let body = Box::pin(async_stream::stream! {
            if !reply.delay.is_zero(){tokio::time::sleep(reply.delay).await;}
            for (index,chunk) in reply.body.chunks(reply.fragment.max(1)).enumerate(){if index>0&&!reply.interval.is_zero(){tokio::time::sleep(reply.interval).await;}yield Ok(chunk.to_vec());tokio::task::yield_now().await;}
        });
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}
struct Stub {
    base: String,
    requests: mpsc::UnboundedReceiver<Request>,
    transport: Arc<MemoryHttp>,
}
impl Stub {
    async fn new(replies: Vec<Reply>) -> Self {
        let (tx, requests) = mpsc::unbounded_channel();
        Self {
            base: format!(
                "http://stub-{}.invalid/prefix/v1",
                uuid::Uuid::now_v7().simple()
            ),
            requests,
            transport: Arc::new(MemoryHttp {
                replies: std::sync::Mutex::new(replies.into()),
                requests: tx,
            }),
        }
    }
    fn registry(&self, root: &Path) -> ProviderRegistry {
        ProviderRegistry::with_transport(root, self.transport.clone())
    }
    async fn request(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
fn frame(v: Value) -> String {
    format!("data: {v}\r\n\r\n")
}
fn named(v: Value) -> String {
    format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())
}
fn chat_text(s: &str) -> String {
    frame(
        json!({"id":"chat-1","choices":[{"index":0,"delta":{"role":"assistant","content":s},"finish_reason":null}]}),
    ) + &frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}))
        + &frame(
            json!({"choices":[],"usage":{"prompt_tokens":30,"completion_tokens":8,"prompt_tokens_details":{"cached_tokens":20}}}),
        )
        + "data: [DONE]\n\n"
}
fn chat_calls() -> String {
    frame(
        json!({"id":"chat-1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"echo","arguments":"{\"value\":"}},{"index":1,"id":"call-2","type":"function","function":{"name":"echo","arguments":"{\"value\":"}}]},"finish_reason":null}]}),
    ) + &frame(
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"function":{"arguments":"2}"}},{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":null}]}),
    ) + &frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}))
        + "data: [DONE]\n\n"
}
fn messages_body(blocks: Value, reason: &str) -> Value {
    json!({"id":"msg-1","type":"message","role":"assistant","content":blocks,"stop_reason":reason,"usage":{"input_tokens":10,"output_tokens":8,"cache_read_input_tokens":20,"cache_creation_input_tokens":3}})
}
fn messages_stream(blocks: Vec<Value>, reason: &str) -> String {
    let mut s = named(
        json!({"type":"message_start","message":{"id":"msg-1","role":"assistant","content":[],"usage":{"input_tokens":10,"output_tokens":0,"cache_read_input_tokens":20,"cache_creation_input_tokens":3}}}),
    );
    for (i, b) in blocks.iter().enumerate() {
        let mut start = b.clone();
        let delta = match b["type"].as_str().unwrap() {
            "text" => {
                start["text"] = json!("");
                json!({"type":"text_delta","text":b["text"]})
            }
            "tool_use" => {
                start["input"] = json!({});
                json!({"type":"input_json_delta","partial_json":b["input"].to_string()})
            }
            _ => panic!(),
        };
        s += &named(json!({"type":"content_block_start","index":i,"content_block":start}));
        s += &named(json!({"type":"content_block_delta","index":i,"delta":delta}));
        s += &named(json!({"type":"content_block_stop","index":i}));
    }
    s += &named(
        json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":8}}),
    );
    s += &named(json!({"type":"message_stop"}));
    s
}
fn responses_body(items: Value) -> Value {
    json!({"id":"resp-1","status":"completed","output":items,"usage":{"input_tokens":30,"output_tokens":8,"input_tokens_details":{"cached_tokens":20},"output_tokens_details":{"reasoning_tokens":3}}})
}
fn response_message(s: &str) -> Value {
    json!({"id":"item-1","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":s,"annotations":[]}]})
}
fn responses_stream(items: Vec<Value>) -> String {
    let mut s =
        named(json!({"type":"response.created","response":{"id":"resp-1","status":"in_progress"}}));
    for (i, item) in items.iter().enumerate() {
        s += &named(json!({"type":"response.output_item.added","output_index":i,"item":item}));
        if item["type"] == "message" {
            s += &named(
                json!({"type":"response.output_text.delta","output_index":i,"delta":item["content"][0]["text"]}),
            );
        }
        if item["type"] == "function_call" {
            s += &named(
                json!({"type":"response.function_call_arguments.delta","output_index":i,"delta":item["arguments"]}),
            );
        }
        s += &named(json!({"type":"response.output_item.done","output_index":i,"item":item}));
    }
    s += &named(json!({"type":"response.completed","response":responses_body(json!(items))}));
    s
}
async fn collect(
    provider: &Arc<dyn ModelProvider>,
    request: ModelRequest,
) -> Result<(Vec<String>, ModelResponse), ProviderError> {
    let mut stream = provider.stream(request, c()).await?;
    let mut deltas = vec![];
    let mut completed = None;
    let mut starts = 0;
    while let Some(event) = stream.next().await {
        match event? {
            ModelEvent::Started => {
                starts += 1;
                assert_eq!(starts, 1);
            }
            ModelEvent::TextDelta(s) => {
                assert!(completed.is_none());
                deltas.push(s);
            }
            ModelEvent::Completed(r) => {
                assert!(completed.replace(*r).is_none());
            }
            _ => assert!(completed.is_none()),
        }
    }
    Ok((
        deltas,
        completed.ok_or_else(|| ProviderError::protocol("missing complete"))?,
    ))
}
fn request() -> ModelRequest {
    ModelRequest {
        messages: vec![
            AgentMessage::System("system".into()),
            AgentMessage::User(text("hello")),
        ],
        ..Default::default()
    }
}
#[tokio::test]
async fn all_protocols_stream_text_with_final_usage_and_preserved_prefix_paths() {
    let replies = vec![
        (
            ProtocolKind::OpenaiChat,
            Reply::sse(chat_text("你好")),
            "chat/completions",
        ),
        (
            ProtocolKind::AnthropicMessages,
            Reply::sse(messages_stream(
                vec![json!({"type":"text","text":"你好"})],
                "end_turn",
            )),
            "messages",
        ),
        (
            ProtocolKind::OpenaiResponses,
            Reply::sse(responses_stream(vec![response_message("你好")])),
            "responses",
        ),
    ];
    for (protocol, reply, path) in replies {
        let mut stub = Stub::new(vec![reply]).await;
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            &config(&stub.base, protocol, TransportMode::Sse),
        );
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        let (deltas, r) = collect(&bound.provider, request()).await.unwrap();
        assert_eq!(deltas.concat(), "你好");
        assert_eq!(r.message.content, text("你好"));
        assert_eq!(r.finish_reason, ModelFinishReason::Stop);
        assert_eq!(r.usage.cached_tokens, Some(20));
        assert_eq!(
            r.usage.input_tokens,
            Some(if protocol == ProtocolKind::AnthropicMessages {
                33
            } else {
                30
            })
        );
        assert_eq!(r.request_id.as_deref(), Some("req-test"));
        let sent = stub.request().await;
        assert_eq!(sent.path, format!("/prefix/v1/{path}"));
        assert_eq!(
            serde_json::from_slice::<Value>(&sent.body).unwrap()["stream"],
            true
        );
    }
}
#[tokio::test]
async fn json_mode_has_same_completion_semantics_without_fake_deltas() {
    let samples = vec![
        (
            ProtocolKind::OpenaiChat,
            json!({"id":"chat","choices":[{"message":{"role":"assistant","content":"hello","tool_calls":null,"reasoning_content":null},"finish_reason":"stop"}],"usage":{"prompt_tokens":1}}),
        ),
        (
            ProtocolKind::AnthropicMessages,
            messages_body(json!([{"type":"text","text":"hello"}]), "end_turn"),
        ),
        (
            ProtocolKind::OpenaiResponses,
            responses_body(json!([response_message("hello")])),
        ),
    ];
    for (p, body) in samples {
        let stub = Stub::new(vec![Reply::json(body)]).await;
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), &config(&stub.base, p, TransportMode::Json));
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        let (deltas, r) = collect(&bound.provider, request()).await.unwrap();
        assert!(deltas.is_empty());
        assert_eq!(r.message.content, text("hello"));
    }
}
struct Echo(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    async fn execute(&self, args: Value, _: ToolContext) -> Result<ToolResult, AgentError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: text(&args.to_string()),
            is_error: false,
            terminate: false,
        })
    }
}
#[tokio::test]
async fn every_adapter_completes_parallel_tools_then_continues_the_same_loop() {
    let calls = vec![
        json!({"type":"tool_use","id":"call-1","name":"echo","input":{"value":1}}),
        json!({"type":"tool_use","id":"call-2","name":"echo","input":{"value":2}}),
    ];
    let samples = vec![
        (ProtocolKind::OpenaiChat, chat_calls(), chat_text("done")),
        (
            ProtocolKind::AnthropicMessages,
            messages_stream(calls, "tool_use"),
            messages_stream(vec![json!({"type":"text","text":"done"})], "end_turn"),
        ),
        (
            ProtocolKind::OpenaiResponses,
            responses_stream(vec![
                json!({"id":"f-1","type":"function_call","call_id":"call-1","name":"echo","arguments":"{\"value\":1}"}),
                json!({"id":"f-2","type":"function_call","call_id":"call-2","name":"echo","arguments":"{\"value\":2}"}),
            ]),
            responses_stream(vec![response_message("done")]),
        ),
    ];
    for (p, first, last) in samples {
        let mut stub = Stub::new(vec![Reply::sse(first), Reply::sse(last)]).await;
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), &config(&stub.base, p, TransportMode::Sse));
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentLoop::new(bound.provider);
        agent.register_tool(Arc::new(Echo(count.clone())));
        let mut state = AgentState {
            messages: vec![],
            turn_count: 0,
        };
        let result = agent.run(text("do it"), &mut state, c()).await.unwrap();
        assert_eq!(result.reason, StopReason::Completed, "{:?}", result.error);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert_eq!(
            result
                .events
                .iter()
                .filter(|e| matches!(e, AgentEvent::UsageReported(_)))
                .count(),
            2
        );
        stub.request().await;
        let next = stub.request().await;
        let body = String::from_utf8(next.body).unwrap();
        assert!(body.contains("call-1") && body.contains("call-2"));
    }
}
#[tokio::test]
async fn prepared_bytes_are_sent_unchanged_and_cannot_move_between_instances() {
    let mut stub = Stub::new(vec![Reply::sse(chat_text("ok"))]).await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse);
    cfg["providers"]["local"]["protocol_options"] =
        json!({"output_token_field":"max_completion_tokens","stream_usage":true});
    write_config(dir.path(), &cfg);
    let registry = stub.registry(dir.path());
    let a = registry.resolve(&selection(), c()).await.unwrap();
    let b = registry.resolve(&selection(), c()).await.unwrap();
    assert_eq!(
        a.provider.configuration_key(),
        b.provider.configuration_key()
    );
    let prepared = a.provider.prepare(request()).unwrap();
    let body = prepared.body().to_vec();
    assert!(!prepared.estimate().exact);
    assert!(prepared.estimated_input_tokens() >= body.len());
    let err = b
        .provider
        .stream_prepared(prepared, c())
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ProviderErrorKind::Protocol);
    let mut stream = a
        .provider
        .stream_prepared(a.provider.prepare(request()).unwrap(), c())
        .await
        .unwrap();
    while let Some(e) = stream.next().await {
        e.unwrap();
    }
    assert_eq!(stub.request().await.body, body);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["max_completion_tokens"], 512);
    assert!(v.get("max_tokens").is_none());
    assert_eq!(v["stream_options"]["include_usage"], true);
}
#[test]
fn config_rejects_invalid_versions_fields_capacities_routes_and_unknown_references() {
    let dir = tempfile::tempdir().unwrap();
    let base = config(
        "http://localhost/v1",
        ProtocolKind::OpenaiChat,
        TransportMode::Sse,
    );
    let mut cases = vec![];
    let mut x = base.clone();
    x["schema_version"] = json!(9);
    cases.push(x);
    let mut x = base.clone();
    x["unknown"] = json!(true);
    cases.push(x);
    let mut x = base.clone();
    x["models"]["test"]["capabilities"]["context_tokens"] = json!(0);
    cases.push(x);
    let mut x = base.clone();
    x["providers"]["local"]["base_url"] = json!("https://host/v1?api_key=secret");
    cases.push(x);
    let mut x = base.clone();
    x["providers"]["local"]["state_scope"] = json!("");
    cases.push(x);
    let mut x = base.clone();
    x["models"]["test"]["provider"] = json!("missing");
    cases.push(x);
    let mut x = base.clone();
    x["models"]["test"]["reasoning"] = json!(true);
    cases.push(x);
    for x in cases {
        write_config(dir.path(), &x);
        assert!(ProviderConfig::load(dir.path()).is_err());
    }
    write_config(dir.path(), &base);
    let reg = ProviderRegistry::new(dir.path());
    assert_eq!(reg.default_selection().unwrap(), selection());
    assert!(reg.selection("missing", None).is_err());
    assert_eq!(reg.selection("mock", None).unwrap(), ModelSelection::Mock);
}
#[test]
fn duplicate_config_keys_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".lumen")).unwrap();
    std::fs::write(
        dir.path().join(".lumen/providers.yaml"),
        "schema_version: 1\ndefault_model: x\nproviders: {}\nmodels:\n  x: {}\n  x: {}\n",
    )
    .unwrap();
    assert!(ProviderConfig::load(dir.path()).is_err());
}
#[tokio::test]
async fn missing_credentials_do_not_break_catalog_or_secret_free_describe() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        "http://localhost/v1",
        ProtocolKind::OpenaiChat,
        TransportMode::Sse,
    );
    cfg["providers"]["local"]["auth"] =
        json!({"kind":"env","variable":format!("LUMEN_MISSING_{}",uuid::Uuid::now_v7().simple())});
    write_config(dir.path(), &cfg);
    let reg = ProviderRegistry::new(dir.path());
    assert!(reg.models().is_ok() && reg.providers().is_ok());
    assert!(reg.describe(&selection()).is_ok());
    let error = reg.resolve(&selection(), c()).await.err().unwrap();
    assert_eq!(error.kind, ProviderErrorKind::Configuration);
    assert!(reg.resolve(&ModelSelection::Mock, c()).await.is_ok());
}
#[tokio::test]
async fn tool_and_parameter_capabilities_are_checked_before_network() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        "http://localhost/v1",
        ProtocolKind::OpenaiChat,
        TransportMode::Sse,
    );
    cfg["models"]["test"]["capabilities"]["tools"] = json!(false);
    cfg["models"]["test"]["capabilities"]["temperature"] = json!(false);
    write_config(dir.path(), &cfg);
    let reg = ProviderRegistry::new(dir.path());
    let bound = reg.resolve(&selection(), c()).await.unwrap();
    let mut r = request();
    r.tools.push(ToolDeclaration {
        name: "echo".into(),
        description: String::new(),
        input_schema: json!({"type":"object"}),
    });
    assert_eq!(
        bound.provider.prepare(r).err().unwrap().kind,
        ProviderErrorKind::UnsupportedFeature
    );
    let s = ModelSelection::Reference {
        model_ref: "test".into(),
        overrides: GenerationOverrides {
            temperature: Some(0.4),
            ..Default::default()
        },
    };
    assert_eq!(
        reg.resolve(&s, c()).await.err().unwrap().kind,
        ProviderErrorKind::UnsupportedFeature
    );
}
#[tokio::test]
async fn output_limit_and_refusal_are_distinct_and_never_execute_tools() {
    for (finish, stop) in [
        ("length", StopReason::OutputLimit),
        ("content_filter", StopReason::Refused),
    ] {
        let reply = Reply::json(
            json!({"choices":[{"message":{"role":"assistant","content":"partial","tool_calls":[{"id":"call","type":"function","function":{"name":"echo","arguments":"{invalid"}}]},"finish_reason":finish}]}),
        );
        let stub = Stub::new(vec![reply]).await;
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Json),
        );
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let mut agent = AgentLoop::new(bound.provider);
        agent.register_tool(Arc::new(Echo(count.clone())));
        let result = agent
            .run(
                text("go"),
                &mut AgentState {
                    messages: vec![],
                    turn_count: 0,
                },
                c(),
            )
            .await
            .unwrap();
        assert_eq!(result.reason, stop);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(result.messages.iter().any(|m|matches!(m,AgentMessage::Assistant(a) if a.content==text("partial")&&a.tool_calls.is_empty())));
    }
}
#[tokio::test]
async fn incomplete_stream_is_error_and_does_not_commit_or_execute_partial_assistant() {
    let stub = Stub::new(vec![Reply::sse(frame(
        json!({"choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}),
    ))])
    .await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse),
    );
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let result = AgentLoop::new(bound.provider)
        .run(
            text("go"),
            &mut AgentState {
                messages: vec![],
                turn_count: 0,
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::ModelError);
    assert!(result
        .events
        .iter()
        .any(|e| matches!(e, AgentEvent::MessageDelta { .. })));
    assert!(result
        .messages
        .iter()
        .all(|m| !matches!(m, AgentMessage::Assistant(_))));
}
#[tokio::test]
async fn bad_json_duplicate_calls_and_critical_unknown_blocks_fail_closed() {
    let samples = vec![
        (
            ProtocolKind::OpenaiChat,
            json!({"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"a","type":"function","function":{"name":"echo","arguments":"{invalid"}}]},"finish_reason":"tool_calls"}]}),
        ),
        (
            ProtocolKind::OpenaiChat,
            json!({"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"a","type":"function","function":{"name":"echo","arguments":"{}"}},{"id":"a","type":"function","function":{"name":"echo","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        ),
        (
            ProtocolKind::AnthropicMessages,
            messages_body(json!([{"type":"server_tool_use","id":"a"}]), "end_turn"),
        ),
        (
            ProtocolKind::OpenaiResponses,
            responses_body(json!([{"type":"computer_call","id":"a"}])),
        ),
    ];
    for (protocol, body) in samples {
        let stub = Stub::new(vec![Reply::json(body)]).await;
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            &config(&stub.base, protocol, TransportMode::Json),
        );
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        assert!(collect(&bound.provider, request()).await.is_err());
    }
}
#[tokio::test]
async fn http_errors_are_structured_redacted_and_never_retried() {
    let secret = "private-provider-credential";
    let variable = format!("LUMEN_PROVIDER_{}", uuid::Uuid::now_v7().simple());
    std::env::set_var(&variable, secret);
    let mut reply = Reply::json(json!({"error":{"code":"rate_limit","message":secret}}));
    reply.status = 429;
    reply.headers = "Retry-After: 12\r\n".into();
    let mut stub = Stub::new(vec![reply]).await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Json);
    cfg["providers"]["local"]["auth"] = json!({"kind":"env","variable":variable});
    write_config(dir.path(), &cfg);
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let prepared = bound.provider.prepare(request()).unwrap();
    assert!(!String::from_utf8_lossy(prepared.body()).contains(secret));
    assert!(!serde_json::to_string(&bound.descriptor)
        .unwrap()
        .contains(secret));
    let error = bound
        .provider
        .stream_prepared(prepared, c())
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind, ProviderErrorKind::RateLimited);
    assert_eq!(error.status, Some(429));
    assert_eq!(error.retry_after, Some(12));
    assert_eq!(error.delivery, DeliveryState::Rejected);
    assert!(!format!("{error:?}").contains(secret));
    let req = stub.request().await;
    assert!(req.headers.contains(secret));
    std::env::remove_var(variable);
}
#[tokio::test]
async fn first_byte_timeout_and_cancellation_interrupt_idle_http_without_commit() {
    let stub = Stub::new(vec![Reply {
        delay: Duration::from_secs(10),
        ..Reply::sse(chat_text("late"))
    }])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse);
    cfg["providers"]["local"]["transport"]["first_byte_timeout_ms"] = json!(30);
    write_config(dir.path(), &cfg);
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let mut stream = bound.provider.stream(request(), c()).await.unwrap();
    assert!(matches!(stream.next().await, Some(Ok(ModelEvent::Started))));
    let err = stream.next().await.unwrap().err().unwrap();
    assert_eq!(err.kind, ProviderErrorKind::Timeout);
    assert!(stream.next().await.is_none());
    let stub = Stub::new(vec![Reply {
        delay: Duration::from_secs(10),
        ..Reply::sse(chat_text("late"))
    }])
    .await;
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse),
    );
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let cancel = c();
    let mut stream = bound
        .provider
        .stream(request(), cancel.clone())
        .await
        .unwrap();
    stream.next().await.unwrap().unwrap();
    cancel.cancel();
    assert_eq!(
        stream.next().await.unwrap().err().unwrap().kind,
        ProviderErrorKind::Cancelled
    );
    assert!(stream.next().await.is_none());
    let cancelled = c();
    cancelled.cancel();
    assert_eq!(
        bound
            .provider
            .stream(request(), cancelled)
            .await
            .err()
            .unwrap()
            .kind,
        ProviderErrorKind::Cancelled
    );
}
#[tokio::test]
async fn responses_native_state_survives_storage_and_is_validated_before_replay() {
    let items = vec![
        json!({"type":"reasoning","id":"reason-1","summary":[],"encrypted_content":"opaque-state"}),
        json!({"type":"function_call","id":"function-1","call_id":"call-1","name":"echo","arguments":"{}"}),
    ];
    let stub = Stub::new(vec![Reply::sse(responses_stream(items))]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(
            &stub.base,
            ProtocolKind::OpenaiResponses,
            TransportMode::Sse,
        ),
    );
    let reg = stub.registry(dir.path());
    let bound = reg.resolve(&selection(), c()).await.unwrap();
    let (_, r) = collect(&bound.provider, request()).await.unwrap();
    assert!(r.message.provider_replay.is_some());
    assert!(!format!("{:?}", r.message.provider_replay).contains("opaque-state"));
    let stored = serde_json::to_vec(&AgentMessage::Assistant(r.message)).unwrap();
    let a: AgentMessage = serde_json::from_slice(&stored).unwrap();
    let mut next = request();
    next.messages.push(a.clone());
    next.messages
        .push(AgentMessage::ToolResult(ToolResultMessage {
            terminate: false,
            tool_call_id: "call-1".into(),
            content: text("ok"),
            is_error: false,
        }));
    let prepared = bound.provider.prepare(next.clone()).unwrap();
    let payload: Value = serde_json::from_slice(prepared.body()).unwrap();
    assert_eq!(payload["store"], false);
    assert_eq!(payload["input"][2]["encrypted_content"], "opaque-state");
    assert_eq!(payload["input"][4]["call_id"], "call-1");
    let mut cfg = config(
        "http://localhost/v1",
        ProtocolKind::OpenaiChat,
        TransportMode::Sse,
    );
    write_config(dir.path(), &cfg);
    let other = reg.resolve(&selection(), c()).await.unwrap();
    assert_eq!(
        other.provider.prepare(next.clone()).err().unwrap().kind,
        ProviderErrorKind::IncompatibleState
    );
    cfg = config(
        &stub.base,
        ProtocolKind::OpenaiResponses,
        TransportMode::Sse,
    );
    write_config(dir.path(), &cfg);
    if let AgentMessage::Assistant(m) = &mut next.messages[2] {
        m.tool_calls[0].name = "changed".into();
    }
    assert_eq!(
        bound.provider.prepare(next).err().unwrap().kind,
        ProviderErrorKind::Protocol
    );
}
#[test]
fn old_assistant_serialization_is_byte_identical_and_v1_model_still_decodes() {
    let old = r#"{"Assistant":{"content":[{"Text":"old"}],"tool_calls":[]}}"#;
    let message: AgentMessage = serde_json::from_str(old).unwrap();
    assert_eq!(serde_json::to_string(&message).unwrap(), old);
    let old = r#"{"profile_ref":"default","model":"old-model","format":"openai"}"#;
    let model: SessionModel = serde_json::from_str(old).unwrap();
    assert!(matches!(
        model.selection().unwrap(),
        ModelSelection::Legacy {
            format: LegacyFormat::Openai,
            ..
        }
    ));
}
#[tokio::test]
async fn frozen_provider_ignores_later_config_changes_and_new_runs_see_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        "http://localhost/v1",
        ProtocolKind::OpenaiChat,
        TransportMode::Json,
    );
    write_config(dir.path(), &cfg);
    let reg = ProviderRegistry::new(dir.path());
    let first = reg.resolve(&selection(), c()).await.unwrap();
    let first_key = first.provider.configuration_key();
    cfg["models"]["test"]["model_id"] = json!("new-model");
    write_config(dir.path(), &cfg);
    let next = reg.resolve(&selection(), c()).await.unwrap();
    assert_ne!(next.provider.configuration_key(), first_key);
    assert_eq!(first.provider.configuration_key(), first_key);
    let body: Value =
        serde_json::from_slice(first.provider.prepare(request()).unwrap().body()).unwrap();
    assert_eq!(body["model"], "test-model");
}
#[tokio::test]
async fn legacy_profiles_use_exact_endpoints_and_are_not_reinterpreted_as_aliases() {
    let mut stub = Stub::new(vec![Reply::json(
        json!({"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}]}),
    )])
    .await;
    let dir = tempfile::tempdir().unwrap();
    let variable = format!("LUMEN_LEGACY_{}", uuid::Uuid::now_v7().simple());
    std::env::set_var(&variable, "credential");
    std::fs::create_dir_all(dir.path().join(".lumen")).unwrap();
    let endpoint = format!("{}/exact", stub.base);
    std::fs::write(
        dir.path().join(".lumen/model-profiles.yaml"),
        serde_yaml_ng::to_string(&json!({"profile":{"endpoint":endpoint,"api_key_env":variable}}))
            .unwrap(),
    )
    .unwrap();
    write_config(
        dir.path(),
        &config(
            "http://localhost/v1",
            ProtocolKind::OpenaiResponses,
            TransportMode::Sse,
        ),
    );
    let reg = stub.registry(dir.path());
    let bound = reg
        .resolve(
            &ModelSelection::Legacy {
                profile_ref: "profile".into(),
                model: "old".into(),
                format: LegacyFormat::Openai,
            },
            c(),
        )
        .await
        .unwrap();
    collect(&bound.provider, request()).await.unwrap();
    assert_eq!(stub.request().await.path, "/prefix/v1/exact");
    std::env::remove_var(variable);
}
#[tokio::test]
async fn session_execution_plan_records_real_configuration_and_final_usage() {
    let stub = Stub::new(vec![Reply::sse(chat_text("stored"))]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse),
    );
    let mut manager = SessionManager::new(SessionRepository::standalone(dir.path()).unwrap());
    let view = manager
        .create(SessionMetadata::new(
            None,
            SessionModel::from_selection(selection()),
            SessionContext {
                context_tokens: 64000,
                output_tokens: 512,
            },
        ))
        .await
        .unwrap();
    let active = manager.active().unwrap();
    let (metadata, revision) = active.metadata();
    let resolved = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let mut agent = AgentLoop::new(resolved.provider.clone());
    let mut ctx = ContextConfig::for_capacity(64000);
    ctx.output_tokens = 512;
    agent.register_plugin(Arc::new(ContextWindowPlugin::new(
        active.runtime().unwrap(),
        resolved.provider.clone(),
        ctx,
    )));
    let plan = SessionExecutionPlan {
        metadata_revision: revision,
        selection: selection(),
        resolved,
        context: metadata.context,
    };
    let (tx, _) = mpsc::unbounded_channel();
    let result = SessionRunner::run_with_plan(
        active,
        agent,
        text("go"),
        "system",
        c(),
        tx,
        manager.activation(),
        plan,
    )
    .await
    .unwrap();
    assert!(result.primary_error.is_none(), "{:?}", result.primary_error);
    let loaded = manager
        .repository()
        .load(&view.info.identity.session_id)
        .await
        .unwrap();
    assert_eq!(loaded.metadata.schema_version, 2);
    let records = decode_records(&loaded.segments[0].bytes).unwrap();
    if let AgentMessage::Custom { content, .. } = &records[0] {
        let h: SegmentOpen = serde_json::from_value(content.clone()).unwrap();
        assert_eq!(h.schema_version, 2);
        assert_eq!(h.resolved_model.unwrap().model_id, "test-model");
    } else {
        panic!()
    };
    if let AgentMessage::Custom { content, .. } = records.last().unwrap() {
        let end: RunEnd = serde_json::from_value(content.clone()).unwrap();
        assert_eq!(end.schema_version, 2);
        assert_eq!(end.usage.len(), 1);
        assert_eq!(end.usage[0].cached_tokens, Some(20));
    } else {
        panic!()
    };
}

#[tokio::test]
async fn model_switch_preserves_committed_summary_and_does_not_resurrect_raw_native_state() {
    let mut stub = Stub::new(vec![Reply::sse(chat_text("new answer"))]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse),
    );
    let resolved = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let runtime = SessionRuntime::memory();
    runtime
        .append(AgentMessage::System("core".into()))
        .await
        .unwrap();
    runtime
        .append(AgentMessage::User(text("original request")))
        .await
        .unwrap();
    let old = AssistantMessage {
        content: text("old response"),
        provider_replay: Some(ProviderReplay {
            schema_version: 1,
            protocol: ProtocolKind::OpenaiResponses,
            compatibility_key: "old".into(),
            content_hash: "unused".into(),
            items: vec![json!({"type":"reasoning","encrypted_content":"old-opaque"})],
        }),
        ..Default::default()
    };
    runtime.append(AgentMessage::Assistant(old)).await.unwrap();
    let snap = runtime.snapshot().await;
    let mut next = snap.window.clone();
    next.epoch += 1;
    next.through = snap.messages.len();
    next.config = "old-provider".into();
    next.manifest = "old-committed-summary".into();
    next.projection = vec![
        AgentMessage::System("core".into()),
        AgentMessage::User(text("original request")),
        AgentMessage::User(text("Historical summary: prior work complete")),
    ];
    runtime.commit(&snap.window, next, true).await.unwrap();
    let mut agent = AgentLoop::new(resolved.provider.clone()).with_session(runtime.clone());
    agent.register_plugin(Arc::new(ContextWindowPlugin::new(
        runtime,
        resolved.provider,
        ContextConfig::for_capacity(64000),
    )));
    let result = agent
        .run(
            text("continue"),
            &mut AgentState {
                messages: vec![],
                turn_count: 0,
            },
            c(),
        )
        .await
        .unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    let req = stub.request().await;
    let body = String::from_utf8(req.body).unwrap();
    assert!(body.contains("Historical summary"));
    assert!(!body.contains("old-opaque") && !body.contains("old response"));
}
#[tokio::test]
async fn known_usage_is_retained_for_bad_tool_json_and_failed_summary() {
    let mut bad = json!({"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"a","type":"function","function":{"name":"echo","arguments":"{bad"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":42,"completion_tokens":5}});
    let stub = Stub::new(vec![Reply::json(bad.clone())]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Json),
    );
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let e = collect(&bound.provider, request()).await.err().unwrap();
    assert_eq!(e.observed_usage.unwrap().input_tokens, Some(42));
    bad["choices"][0]["message"] = json!({"role":"assistant","content":"not a structured summary"});
    bad["choices"][0]["finish_reason"] = json!("stop");
    let stub = Stub::new(vec![Reply::json(bad)]).await;
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Json),
    );
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let e = ModelCompactionService::new(bound.provider)
        .summarize(request(), 512, c())
        .await
        .err()
        .unwrap();
    let (usage, reports) = e.observations();
    assert_eq!(usage.len(), 1);
    assert_eq!(usage[0].input_tokens, Some(42));
    assert_eq!(reports.len(), 1);
}
#[tokio::test]
async fn messages_thinking_signature_is_replayed_in_original_block_order() {
    let blocks = json!([{"type":"thinking","thinking":"internal","signature":"signed-state"},{"type":"tool_use","id":"call","name":"echo","input":{}},{"type":"text","text":"working"}]);
    let stub = Stub::new(vec![Reply::json(messages_body(blocks, "tool_use"))]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(
            &stub.base,
            ProtocolKind::AnthropicMessages,
            TransportMode::Json,
        ),
    );
    let reg = stub.registry(dir.path());
    let bound = reg.resolve(&selection(), c()).await.unwrap();
    let (_, r) = collect(&bound.provider, request()).await.unwrap();
    assert_eq!(r.message.content, text("working"));
    let mut req = request();
    req.messages.push(AgentMessage::Assistant(r.message));
    req.messages
        .push(AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: "call".into(),
            content: text("ok"),
            is_error: false,
            terminate: false,
        }));
    let prepared = bound.provider.prepare(req.clone()).unwrap();
    let v: Value = serde_json::from_slice(prepared.body()).unwrap();
    assert_eq!(v["messages"][1]["content"][0]["signature"], "signed-state");
    assert_eq!(v["messages"][1]["content"][1]["type"], "tool_use");
    let mut cfg = config(
        &stub.base,
        ProtocolKind::AnthropicMessages,
        TransportMode::Json,
    );
    cfg["providers"]["local"]["state_scope"] = json!("another-account");
    write_config(dir.path(), &cfg);
    let other = reg.resolve(&selection(), c()).await.unwrap();
    assert_eq!(
        other.provider.prepare(req).err().unwrap().kind,
        ProviderErrorKind::IncompatibleState
    );
}
#[tokio::test]
async fn caching_is_encoded_before_budgeting_and_can_be_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        "http://localhost/v1",
        ProtocolKind::AnthropicMessages,
        TransportMode::Sse,
    );
    cfg["models"]["test"]["capabilities"]["cache_modes"] = json!(["disabled", "provider_default"]);
    write_config(dir.path(), &cfg);
    let bound = ProviderRegistry::new(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let p = bound.provider.prepare(request()).unwrap();
    let body: Value = serde_json::from_slice(p.body()).unwrap();
    assert_eq!(body["cache_control"]["type"], "ephemeral");
    assert!(p.estimated_input_tokens() >= p.body().len());
    let mut req = request();
    req.cache_intent = Some(CacheIntent::Disabled);
    let p = bound.provider.prepare(req).unwrap();
    let body: Value = serde_json::from_slice(p.body()).unwrap();
    assert!(body.get("cache_control").is_none());
}
#[tokio::test]
async fn v1_session_metadata_upgrades_on_write_with_old_segments_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let repo = SessionRepository::standalone(dir.path()).unwrap();
    let mut m = SessionManager::new(repo.clone());
    let view = m
        .create(SessionMetadata::new(
            None,
            SessionModel::default(),
            SessionContext::default(),
        ))
        .await
        .unwrap();
    let id = view.info.identity.session_id;
    m.close(None).unwrap();
    let path = dir.path().join("sessions").join(&id).join("session.yaml");
    let mut old: Value = serde_yaml_ng::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["schema_version"] = json!(1);
    old["model"] = json!({"profile_ref":"default","model":"mock","format":"openai"});
    let before = serde_yaml_ng::to_string(&old).unwrap();
    std::fs::write(&path, &before).unwrap();
    m.switch(&id, false, None).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    let handle = m.active().unwrap();
    let (meta, revision) = handle.metadata();
    let bound = ProviderRegistry::new(dir.path())
        .resolve(&meta.model.selection().unwrap(), c())
        .await
        .unwrap();
    let agent = AgentLoop::new(bound.provider.clone());
    let plan = SessionExecutionPlan {
        metadata_revision: revision,
        selection: meta.model.selection().unwrap(),
        resolved: bound,
        context: meta.context,
    };
    let (tx, _) = mpsc::unbounded_channel();
    let r = SessionRunner::run_with_plan(
        handle,
        agent,
        text("go"),
        "system",
        c(),
        tx,
        m.activation(),
        plan,
    )
    .await
    .unwrap();
    assert!(r.primary_error.is_none());
    let upgraded: Value = serde_yaml_ng::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(upgraded["schema_version"], 2);
    assert_eq!(upgraded["model"]["kind"], "mock");
}
#[tokio::test]
async fn oversized_and_wrong_content_type_responses_are_errors() {
    for reply in [
        Reply::sse(format!("data: {}\n\n", "x".repeat(1024 * 1024 + 1))),
        Reply {
            content_type: "application/json",
            ..Reply::sse(chat_text("text"))
        },
    ] {
        let stub = Stub::new(vec![Reply {
            fragment: usize::MAX,
            ..reply
        }])
        .await;
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse),
        );
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        assert_eq!(
            collect(&bound.provider, request())
                .await
                .err()
                .unwrap()
                .kind,
            ProviderErrorKind::Protocol
        );
    }
}
/// Run explicitly on a machine that permits local TCP listeners.
#[tokio::test]
#[ignore = "requires loopback socket permission; protocol and lifecycle coverage runs with the in-memory HTTP boundary"]
async fn reqwest_transport_loopback_smoke() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut raw = vec![];
        let split;
        loop {
            let mut buf = [0; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            raw.extend_from_slice(&buf[..n]);
            if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                split = i + 4;
                break;
            }
        }
        let header = String::from_utf8_lossy(&raw[..split]).into_owned();
        let len = header
            .lines()
            .find_map(|s| {
                s.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while raw.len() < split + len {
            let mut buf = [0; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            raw.extend_from_slice(&buf[..n]);
        }
        assert!(header.starts_with("POST /v1/chat/completions "));
        let body = chat_text("live local");
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&base, ProtocolKind::OpenaiChat, TransportMode::Sse),
    );
    let bound = ProviderRegistry::new(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    assert_eq!(
        collect(&bound.provider, request())
            .await
            .unwrap()
            .1
            .message
            .content,
        text("live local")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn unsupported_native_schema_is_readonly_but_history_remains_visible() {
    let dir = tempfile::tempdir().unwrap();
    let repo = SessionRepository::standalone(dir.path()).unwrap();
    let mut manager = SessionManager::new(repo.clone());
    let view = manager
        .create(SessionMetadata::new(
            None,
            SessionModel::default(),
            SessionContext::default(),
        ))
        .await
        .unwrap();
    let id = view.info.identity.session_id;
    manager.close(None).unwrap();
    let segment = repo.begin(&id).await.unwrap();
    repo.append(
        &id,
        &segment,
        &AgentMessage::Assistant(AssistantMessage {
            content: text("visible history"),
            provider_replay: Some(ProviderReplay {
                schema_version: 99,
                protocol: ProtocolKind::OpenaiResponses,
                compatibility_key: String::new(),
                content_hash: String::new(),
                items: vec![],
            }),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    repo.finish(&id, &segment).await.unwrap();
    assert!(manager.switch(&id, false, None).await.is_err());
    let view = manager.switch(&id, true, None).await.unwrap();
    assert_eq!(view.info.health, SessionHealth::Unsupported);
    assert_eq!(view.history.messages[0].text, "visible history");
    assert!(!view.writable);
}
#[test]
fn duplicate_valid_aliases_are_rejected_and_timeouts_have_finite_limits() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(
        "http://localhost/v1",
        ProtocolKind::OpenaiChat,
        TransportMode::Sse,
    );
    std::fs::create_dir_all(dir.path().join(".lumen")).unwrap();
    let model = &cfg["models"]["test"];
    let raw=format!("{{\"schema_version\":1,\"default_model\":\"test\",\"providers\":{},\"models\":{{\"test\":{},\"test\":{}}}}}",cfg["providers"],model,model);
    std::fs::write(dir.path().join(".lumen/providers.yaml"), raw).unwrap();
    assert!(ProviderConfig::load(dir.path()).is_err());
    cfg["providers"]["local"]["transport"]["request_timeout_ms"] = json!(u64::MAX);
    write_config(dir.path(), &cfg);
    assert!(ProviderConfig::load(dir.path()).is_err());
}

#[tokio::test]
async fn idle_timeout_and_absolute_deadline_are_separate_and_heartbeats_do_not_extend_runs() {
    for (idle, request_ms, gap) in [(20, 1000, 100), (1000, 30, 2)] {
        let reply = Reply {
            body: format!("{}{}", ": ping\n\n".repeat(100), chat_text("late")).into_bytes(),
            fragment: 1,
            interval: Duration::from_millis(gap),
            ..Reply::sse(String::new())
        };
        let stub = Stub::new(vec![reply]).await;
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse);
        cfg["providers"]["local"]["transport"]["idle_timeout_ms"] = json!(idle);
        cfg["providers"]["local"]["transport"]["request_timeout_ms"] = json!(request_ms);
        write_config(dir.path(), &cfg);
        let bound = stub
            .registry(dir.path())
            .resolve(&selection(), c())
            .await
            .unwrap();
        let e = collect(&bound.provider, request()).await.err().unwrap();
        assert_eq!(e.kind, ProviderErrorKind::Timeout);
    }
}
#[tokio::test]
async fn in_band_error_keeps_usage_once_and_does_not_commit_visible_deltas() {
    let body =
        frame(json!({"choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}))
            + &frame(json!({"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":2}}))
            + &frame(json!({"error":{"message":"private upstream detail"}}));
    let stub = Stub::new(vec![Reply::sse(body)]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Sse),
    );
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let r = AgentLoop::new(bound.provider)
        .run(
            text("go"),
            &mut AgentState {
                messages: vec![],
                turn_count: 0,
            },
            c(),
        )
        .await
        .unwrap();
    assert_eq!(r.reason, StopReason::ModelError);
    let usages = r
        .events
        .iter()
        .filter_map(|e| {
            if let AgentEvent::UsageReported(u) = e {
                Some(u)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(usages.len(), 1);
    assert_eq!(usages[0].input_tokens, Some(42));
    assert!(!r
        .messages
        .iter()
        .any(|m| matches!(m, AgentMessage::Assistant(_))));
    assert!(!format!("{:?}", r.error).contains("private upstream detail"));
}
#[tokio::test]
async fn cumulative_response_limit_is_applied_before_json_decode() {
    let reply = Reply {
        body: vec![b'x'; 32 * 1024 * 1024 + 1],
        ..Reply::json(json!({}))
    };
    let stub = Stub::new(vec![reply]).await;
    let dir = tempfile::tempdir().unwrap();
    write_config(
        dir.path(),
        &config(&stub.base, ProtocolKind::OpenaiChat, TransportMode::Json),
    );
    let bound = stub
        .registry(dir.path())
        .resolve(&selection(), c())
        .await
        .unwrap();
    let e = collect(&bound.provider, request()).await.err().unwrap();
    assert_eq!(e.kind, ProviderErrorKind::Protocol);
    assert!(e.message.contains("size limit"));
}

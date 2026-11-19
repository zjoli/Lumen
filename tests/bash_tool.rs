#![cfg(target_os = "linux")]
use lumen::{agent::*, context::*};
use serde_json::{json, Value};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

// Each sandbox applies per-user process limits. Keep integration tests deterministic.
static SANDBOX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn metadata(result: &ToolResult) -> Value {
    let ContentPart::Text(header) = &result.content[0];
    serde_json::from_str(header.trim().strip_prefix("Bash result: ").unwrap()).unwrap()
}
fn output(result: &ToolResult) -> &str {
    let ContentPart::Text(output) = &result.content[1];
    output
}
async fn execute(root: &Path, args: Value) -> ToolResult {
    BashTool::new(root.to_owned())
        .execute(
            args,
            ToolContext {
                cancel: CancellationToken::new(),
            },
        )
        .await
        .unwrap()
}
fn assert_success(result: &ToolResult) {
    assert!(!result.is_error, "{:?}", result.content);
    assert_eq!(metadata(result)["exit_code"], 0);
}

#[tokio::test]
async fn commands_pipelines_curl_selected_inputs_and_ephemeral_scratch() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/input.txt"), "hello 世界\n").unwrap();
    std::fs::write(root.path().join("data/second.txt"), "second\n").unwrap();
    let result = execute(root.path(), json!({
        "command":"pwd; cat /workspace/data/input.txt | tr 'a-z' 'A-Z'; curl -fsS file:///workspace/data/second.txt; printf scratch > /work/result; cat /work/result; printf warning >&2",
        "cwd":"/work", "inputs":["data/input.txt", "data"]
    })).await;
    assert_success(&result);
    assert!(output(&result).contains("HELLO 世界"));
    assert!(output(&result).contains("second"));
    assert!(output(&result).contains("scratch"));
    assert!(output(&result).contains("warning"));
    assert!(!root.path().join("result").exists());
    let next = execute(root.path(), json!({"command":"test ! -e /work/result; test ! -e /workspace/data/input.txt; printf isolated"})).await;
    assert_success(&next);
    assert!(output(&next).contains("isolated"));
}

#[tokio::test]
async fn filesystem_environment_network_and_privileges_are_confined() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("input"), "unchanged").unwrap();
    let host_net = std::fs::read_link("/proc/self/ns/net").unwrap();
    let result = execute(root.path(), json!({"inputs":["input"], "command":format!(
        "set -e\ncat /workspace/input\nif echo modified > /workspace/input; then exit 90; fi\nif touch /usr/lumen-bash-test; then exit 91; fi\ntest ! -e {}\ntest ! -e /home/ai-native/.aws\ntest -z \"${{LUMEN_MODEL_KEY+x}}\"\ntest -z \"${{BASH_ENV+x}}\"\nprintf 'network='; readlink /proc/self/ns/net\ngrep -E 'NoNewPrivs|CapEff' /proc/self/status\nulimit -v; ulimit -u; ulimit -n\nprintf done", root.path().display()
    )})).await;
    assert_success(&result);
    assert!(output(&result).contains(&format!("network={}", host_net.display())));
    assert_eq!(metadata(&result)["network_enabled"], true);
    assert!(output(&result).contains("NoNewPrivs:\t1"));
    assert!(output(&result).contains("CapEff:\t0000000000000000"));
    assert!(output(&result).contains("524288\n64\n256"));
    assert_eq!(
        std::fs::read_to_string(root.path().join("input")).unwrap(),
        "unchanged"
    );
}

#[tokio::test]
async fn socket_policy_preserves_host_ip_access_and_denies_unix_and_io_uring() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    // This host may itself prohibit networking (the Codex runner does). Match
    // its IP socket capability rather than claiming a successful network test.
    let expectations: Vec<_> = [libc::AF_INET, libc::AF_INET6]
        .into_iter()
        .map(|family| {
            let fd = unsafe { libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
            let error = if fd < 0 {
                std::io::Error::last_os_error().raw_os_error().unwrap()
            } else {
                unsafe {
                    libc::close(fd);
                }
                0
            };
            (family, error)
        })
        .collect();
    let script = r#"python3 - <<'PY'
import ctypes, errno, socket
for family, expected_error in EXPECTATIONS:
    try:
        socket.socket(family, socket.SOCK_STREAM).close()
    except OSError as error:
        assert expected_error == error.errno, (family, expected_error, error)
    else:
        assert expected_error == 0
for family in (socket.AF_UNIX, socket.AF_NETLINK):
    try:
        socket.socket(family, socket.SOCK_STREAM)
    except OSError as error:
        assert error.errno == errno.EPERM, error
    else:
        raise AssertionError('non-IP sockets must be blocked')
libc = ctypes.CDLL(None, use_errno=True)
assert libc.syscall(425, 0, 0) == -1
assert ctypes.get_errno() == errno.EPERM
print('socket policy enforced')
PY"#
    .replace("EXPECTATIONS", &format!("{expectations:?}"));
    let result = execute(root.path(), json!({"command": script})).await;
    assert_success(&result);
    assert!(output(&result).contains("socket policy enforced"));
}

#[tokio::test]
async fn invalid_arguments_private_paths_symlinks_and_oversize_inputs_fail_before_execution() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join(".env"), "secret").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.path().join("link")).unwrap();
    std::fs::File::create(root.path().join("large"))
        .unwrap()
        .set_len(17 * 1024 * 1024)
        .unwrap();
    for args in [
        json!({"command":"true","timeout_ms":120001}),
        json!({"command":"true","unknown":true}),
        json!({"command":"true","cwd":"/workspace/../../home"}),
        json!({"command":"true","inputs":["../outside"]}),
        json!({"command":"true","inputs":[".env"]}),
        json!({"command":"true","inputs":["link"]}),
        json!({"command":"true","inputs":["large"]}),
        json!({"command":"true","cwd":"/proc"}),
    ] {
        let result = BashTool::new(root.path().to_owned())
            .execute(
                args,
                ToolContext {
                    cancel: CancellationToken::new(),
                },
            )
            .await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn nonzero_status_and_both_output_streams_are_bounded_without_deadlock() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    let result = execute(root.path(), json!({"command":"(head -c 700000 /dev/zero | tr '\\0' x) & (head -c 700000 /dev/zero | tr '\\0' y >&2) & wait; exit 7"})).await;
    assert!(result.is_error);
    let meta = metadata(&result);
    assert_eq!(meta["exit_code"], 7);
    assert_eq!(meta["stdout_bytes"], 700000);
    assert_eq!(meta["stderr_bytes"], 700000);
    assert_eq!(meta["stdout_truncated"], true);
    assert_eq!(meta["stderr_truncated"], true);
    assert!(output(&result).len() < 1024 * 1024 + 100);
}

fn process_exists(token: &str) -> bool {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            std::fs::read(entry.path().join("cmdline"))
                .ok()
                .is_some_and(|bytes| bytes.split(|b| *b == 0).next() == Some(token.as_bytes()))
        })
}
async fn wait_for_process(token: &str, exists: bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while process_exists(token) != exists {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
fn sleeper(token: &str) -> String {
    format!("setsid /bin/bash --noprofile --norc -c 'exec -a {token} /bin/sleep 60' & wait")
}

#[tokio::test]
async fn timeout_cancel_and_dropped_future_remove_even_detached_children() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    for mode in ["timeout", "cancel", "drop"] {
        let token = format!("lumen-test-{}", uuid::Uuid::now_v7());
        let tool = BashTool::new(root.path().to_owned());
        let args = json!({"command":sleeper(&token),"timeout_ms":if mode == "timeout" {1500} else {30000}});
        let cancel = CancellationToken::new();
        let ctx = ToolContext {
            cancel: cancel.clone(),
        };
        let task = tokio::spawn(async move { tool.execute(args, ctx).await });
        wait_for_process(&token, true).await;
        if mode == "drop" {
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        } else {
            if mode == "cancel" {
                cancel.cancel();
            }
            let result = task.await.unwrap().unwrap();
            assert!(result.is_error);
            assert_eq!(
                metadata(&result)[if mode == "cancel" {
                    "cancelled"
                } else {
                    "timed_out"
                }],
                true
            );
        }
        wait_for_process(&token, false).await;
    }
}

#[tokio::test]
async fn normal_shell_exit_does_not_leave_background_children() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    let token = format!("lumen-test-{}", uuid::Uuid::now_v7());
    let result = execute(root.path(), json!({"command":format!("setsid /bin/bash -c 'exec -a {token} /bin/sleep 60' >/dev/null 2>&1 & sleep 0.1; printf done")})).await;
    assert_success(&result);
    wait_for_process(&token, false).await;
}

#[tokio::test]
async fn registry_cancellation_waits_for_bash_cleanup_and_keeps_status() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::default();
    registry.register(Arc::new(BashTool::new(root.path().to_owned())));
    let token = format!("lumen-test-{}", uuid::Uuid::now_v7());
    let call = ToolCall {
        id: "cancel-bash".into(),
        name: "bash".into(),
        arguments: json!({"command":sleeper(&token)}),
    };
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let task = tokio::spawn(async move { registry.execute(&call, child).await });
    wait_for_process(&token, true).await;
    cancel.cancel();
    let result = task.await.unwrap();
    assert!(result.is_error);
    assert_eq!(result.tool_call_id, "cancel-bash");
    let ContentPart::Text(header) = &result.content[0];
    assert!(header.contains("\"cancelled\":true"));
    wait_for_process(&token, false).await;
}

#[tokio::test]
async fn loop_executes_bash_then_continues_and_large_results_are_recoverable() {
    let _permit = SANDBOX.lock().await;
    let root = tempfile::tempdir().unwrap();
    let runtime = SessionRuntime::memory();
    let provider = Arc::new(MockProvider::new(vec![
        AssistantMessage {
            content: vec![],
            tool_calls: vec![ToolCall {
                id: "bash-1".into(),
                name: "bash".into(),
                arguments: json!({"command":"printf evidence; head -c 16000 /dev/zero | tr '\\0' z"}),
            }],
            ..Default::default()
        },
        AssistantMessage {
            content: vec![ContentPart::Text("done".into())],
            ..Default::default()
        },
    ]));
    let mut agent = AgentLoop::new(provider.clone()).with_session(runtime.clone());
    agent.register_tool(Arc::new(BashTool::new(root.path().to_owned())));
    agent.register_plugin(Arc::new(ContextWindowPlugin::new(
        runtime.clone(),
        provider,
        ContextConfig::default(),
    )));
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    let result = agent
        .run(
            vec![ContentPart::Text("run".into())],
            &mut state,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::Completed, "{:?}", result.error);
    let result = result
        .messages
        .iter()
        .find_map(|message| {
            if let AgentMessage::ToolResult(result) = message {
                Some(result)
            } else {
                None
            }
        })
        .unwrap();
    assert!(!result.is_error);
    let ContentPart::Text(text) = &result.content[0];
    assert!(text.contains("Output abbreviated"));
    assert!(text.contains("\"exit_code\":0"));
    let artifact = text
        .split("artifact=")
        .nth(1)
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    let original = runtime.read_artifact(artifact).await.unwrap();
    assert!(original.content.iter().any(|part| matches!(part, ContentPart::Text(text) if text.contains("evidence") && text.len() > 16000)));
}

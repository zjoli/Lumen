//! Non-interactive Bash, confined to an explicit input snapshot and ephemeral scratch.
use super::*;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

pub struct BashTool {
    input_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    command: String,
    #[serde(default = "default_cwd")]
    cwd: String,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    #[serde(default)]
    inputs: Vec<String>,
}
fn default_cwd() -> String {
    "/workspace".into()
}
fn default_timeout() -> u64 {
    30_000
}
impl BashTool {
    /// Only explicitly named paths below this trusted root can become sandbox inputs.
    pub fn new(input_root: PathBuf) -> Self {
        Self { input_root }
    }
}
fn bash_error(error: impl std::fmt::Display) -> AgentError {
    AgentError::Tool(format!("bash: {error}"))
}
#[async_trait]
impl AgentTool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Run non-interactive Bash in a Linux Bubblewrap sandbox with networking enabled. Use for curl, scripts and data processing. inputs explicitly selects relative files/directories from the workspace (or launch directory); hidden/private paths and symlinks are rejected. Selected files are a read-only snapshot at /workspace. cwd is a sandbox path, default /workspace. Only /work (64 MiB), /tmp (32 MiB), and /dev/shm (8 MiB) are writable scratch; files disappear after this call. Print results to stdout for retention. Commands do not share shell state. No host home, credentials or inherited environment. Use dedicated knowledge tools for persistent changes and existing web tools for research. Missing sandbox support is an error; never falls back to host execution. Output capture is bounded and truncation is explicit."
    }
    fn input_schema(&self) -> Value {
        json!({"type":"object","properties":{
            "command":{"type":"string","minLength":1,"maxLength":65536},
            "cwd":{"type":"string","minLength":1,"maxLength":4096,"description":"Sandbox path under /workspace, /work or /tmp; relative paths are based on /workspace."},
            "timeout_ms":{"type":"integer","minimum":1,"maximum":120000,"default":30000},
            "inputs":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":4096},"description":"Relative input files/directories below the workspace or launch directory; copied into a read-only snapshot. Default: no host files."}
        },"required":["command"],"additionalProperties":false})
    }
    fn execution_timeout(&self, args: &Value) -> Duration {
        // Internal deadline includes input staging. Leave room for process cleanup.
        Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(30_000).min(120_000) + 2_000)
    }
    fn cancellation_grace(&self) -> Duration {
        Duration::from_secs(2)
    }
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolResult, AgentError> {
        // Direct callers get the same contract as ToolRegistry callers.
        let validator = jsonschema::validator_for(&self.input_schema()).map_err(bash_error)?;
        validator.validate(&args).map_err(bash_error)?;
        let args: BashArgs = serde_json::from_value(args).map_err(bash_error)?;
        if args.command.contains('\0') || args.cwd.contains('\0') {
            return Err(bash_error("NUL is not allowed in commands or paths"));
        }
        #[cfg(target_os = "linux")]
        {
            linux::execute(self.input_root.clone(), args, ctx).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (&self.input_root, args, ctx);
            Err(bash_error(
                "sandbox backend unavailable on this platform; host execution is disabled",
            ))
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        fs::{self, File, OpenOptions},
        io::{self, Read, Seek, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt, process::ExitStatusExt},
        },
        path::{Component, Path},
        process::Stdio,
        sync::{Arc, Mutex},
        time::Instant,
    };
    use tokio::{
        io::{AsyncRead, AsyncReadExt},
        process::{Child, Command},
    };

    const INPUT_BYTES: usize = 16 * 1024 * 1024;
    const INPUT_FILES: usize = 1024;
    const CAPTURE_BYTES: usize = 512 * 1024;

    // Absolute system executables: an input-controlled PATH must never select the launcher.
    fn bwrap() -> Result<&'static str, AgentError> {
        ["/usr/bin/bwrap", "/bin/bwrap"]
            .into_iter()
            .find(|p| Path::new(p).is_file())
            .ok_or_else(|| bash_error("Bubblewrap is required; host execution is disabled"))
    }
    fn private_component(name: &std::ffi::OsStr) -> bool {
        let text = name.to_string_lossy().to_ascii_lowercase();
        text.starts_with('.')
            || matches!(
                text.as_str(),
                "credentials"
                    | "credentials.json"
                    | "credentials.yaml"
                    | "credentials.yml"
                    | "id_rsa"
                    | "id_ed25519"
                    | "id_ecdsa"
            )
            || text.ends_with(".pem")
            || text.ends_with(".key")
    }
    fn input_components(path: &str) -> Result<Vec<std::ffi::OsString>, AgentError> {
        let mut components = vec![];
        for c in Path::new(path).components() {
            match c {
                Component::Normal(name) if !private_component(name) => {
                    components.push(name.to_owned())
                }
                _ => {
                    return Err(bash_error(
                        "inputs must be relative non-private paths without traversal",
                    ))
                }
            }
        }
        if components.is_empty() {
            return Err(bash_error("empty input path"));
        }
        Ok(components)
    }
    /// Open each component relative to a directory FD. No symlink-check/open race.
    fn open_at(parent: &File, name: &std::ffi::OsStr) -> io::Result<File> {
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| io::Error::other("NUL in input path"))?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    struct CopyBudget {
        bytes: usize,
        files: usize,
    }
    fn copy_input(
        source: File,
        target: &Path,
        budget: &mut CopyBudget,
        depth: usize,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(), AgentError> {
        if cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        if depth > 32 || budget.files >= INPUT_FILES {
            return Err(bash_error("input tree exceeds depth/file limit"));
        }
        let metadata = source.metadata().map_err(bash_error)?;
        // A directory may exist because an earlier selection copied one child.
        if metadata.is_file() && target.is_file() {
            return Ok(());
        }
        budget.files += 1;
        if metadata.is_dir() {
            fs::create_dir_all(target).map_err(bash_error)?;
            for entry in
                fs::read_dir(format!("/proc/self/fd/{}", source.as_raw_fd())).map_err(bash_error)?
            {
                let entry = entry.map_err(bash_error)?;
                let name = entry.file_name();
                if private_component(&name) {
                    continue;
                }
                let child = open_at(&source, &name).map_err(bash_error)?;
                copy_input(child, &target.join(name), budget, depth + 1, cancel)?;
            }
        } else if metadata.is_file() {
            if metadata.len() > budget.bytes as u64 {
                return Err(bash_error("selected inputs exceed 16 MiB"));
            }
            fs::create_dir_all(target.parent().unwrap()).map_err(bash_error)?;
            let mut destination = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(target)
                .map_err(bash_error)?;
            let mut limited = source.take(budget.bytes as u64 + 1);
            let mut buffer = [0u8; 8192];
            loop {
                if cancel.is_cancelled() {
                    return Err(AgentError::Aborted);
                }
                let n = limited.read(&mut buffer).map_err(bash_error)?;
                if n == 0 {
                    break;
                }
                if n > budget.bytes {
                    return Err(bash_error("selected inputs exceed 16 MiB"));
                }
                budget.bytes -= n;
                destination.write_all(&buffer[..n]).map_err(bash_error)?;
            }
        } else {
            return Err(bash_error("inputs must be regular files or directories"));
        }
        Ok(())
    }
    fn snapshot(
        root: PathBuf,
        inputs: Vec<String>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<tempfile::TempDir, AgentError> {
        let snapshot = tempfile::Builder::new()
            .prefix("lumen-bash-input-")
            .tempdir()
            .map_err(bash_error)?;
        if inputs.is_empty() {
            return Ok(snapshot);
        }
        let canonical = root.canonicalize().map_err(bash_error)?;
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(canonical)
            .map_err(bash_error)?;
        let mut budget = CopyBudget {
            bytes: INPUT_BYTES,
            files: 0,
        };
        for input in inputs {
            let parts = input_components(&input)?;
            let mut source = root.try_clone().map_err(bash_error)?;
            for part in &parts {
                source = open_at(&source, part).map_err(bash_error)?;
            }
            let target: PathBuf = parts.iter().collect();
            copy_input(
                source,
                &snapshot.path().join(target),
                &mut budget,
                0,
                &cancel,
            )?;
        }
        Ok(snapshot)
    }
    fn cwd(path: &str) -> Result<PathBuf, AgentError> {
        let path = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            Path::new("/workspace").join(path)
        };
        if path.components().any(|c| {
            !matches!(
                c,
                Component::RootDir | Component::Normal(_) | Component::CurDir
            )
        }) {
            return Err(bash_error(
                "cwd cannot traverse outside sandbox directories",
            ));
        }
        if !["/workspace", "/work", "/tmp"]
            .iter()
            .any(|root| path.starts_with(root))
        {
            return Err(bash_error("cwd must be under /workspace, /work or /tmp"));
        }
        Ok(path)
    }
    /// Shared networking also shares Linux abstract Unix sockets. Confine payload
    /// socket creation to IP families so host session/management sockets stay hidden.
    fn network_filter() -> Result<File, AgentError> {
        if !cfg!(target_endian = "little") {
            return Err(bash_error(
                "sandbox syscall policy requires little-endian Linux",
            ));
        }
        let arch = if cfg!(target_arch = "x86_64") {
            0xc000003e
        } else if cfg!(target_arch = "aarch64") {
            0xc00000b7
        } else {
            return Err(bash_error(
                "sandbox syscall policy supports only Linux x86_64/aarch64",
            ));
        };
        let mut filter = tempfile::tempfile().map_err(bash_error)?;
        let instructions: [(u16, u8, u8, u32); 15] = [
            (0x20, 0, 0, 4),                               // BPF_LD W ABS: seccomp_data.arch
            (0x15, 1, 0, arch),                            // JEQ expected architecture
            (0x06, 0, 0, 0x80000000),                      // SECCOMP_RET_KILL_PROCESS
            (0x20, 0, 0, 0),                               // syscall nr
            (0x35, 0, 1, 0x40000000),                      // reject x32/unknown syscall ABI
            (0x06, 0, 0, 0x00050001),                      // SECCOMP_RET_ERRNO | EPERM
            (0x15, 0, 1, libc::SYS_io_uring_setup as u32), // no alternate socket creation via io_uring
            (0x06, 0, 0, 0x00050001),
            (0x15, 1, 0, libc::SYS_socket as u32),
            (0x15, 0, 4, libc::SYS_socketpair as u32),
            (0x20, 0, 0, 16), // args[0]: address family (low word on supported LE targets)
            (0x15, 2, 0, libc::AF_INET as u32),
            (0x15, 1, 0, libc::AF_INET6 as u32),
            (0x06, 0, 0, 0x00050001),
            (0x06, 0, 0, 0x7fff0000), // SECCOMP_RET_ALLOW
        ];
        for (code, jt, jf, k) in instructions {
            filter.write_all(&code.to_ne_bytes()).map_err(bash_error)?;
            filter.write_all(&[jt, jf]).map_err(bash_error)?;
            filter.write_all(&k.to_ne_bytes()).map_err(bash_error)?;
        }
        filter.rewind().map_err(bash_error)?;
        Ok(filter)
    }
    fn command(
        launcher: &str,
        snapshot: &Path,
        cwd: &Path,
        args: &BashArgs,
        filter_fd: std::os::fd::RawFd,
    ) -> Command {
        let mut cmd = Command::new(launcher);
        cmd.env_clear()
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd.args([
            "--unshare-all",
            "--unshare-user",
            "--share-net",
            "--uid",
            "65534",
            "--gid",
            "65534",
            "--disable-userns",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
        ]);
        for path in ["/usr", "/bin", "/sbin", "/lib", "/lib64"] {
            if Path::new(path).is_dir() {
                cmd.args(["--ro-bind", path, path]);
            }
        }
        // Only public resolver/TLS runtime files; never bind the host's /etc or home.
        cmd.args(["--dir", "/etc"]);
        for path in [
            "/etc/resolv.conf",
            "/etc/hosts",
            "/etc/nsswitch.conf",
            "/etc/ld.so.cache",
            "/etc/ssl/certs",
            "/etc/pki/tls/certs",
        ] {
            if Path::new(path).exists() {
                cmd.args(["--ro-bind", path, path]);
            }
        }
        cmd.arg("--ro-bind").arg(snapshot).arg("/workspace");
        cmd.args([
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--size",
            "33554432",
            "--tmpfs",
            "/tmp",
            "--size",
            "67108864",
            "--tmpfs",
            "/work",
            "--size",
            "8388608",
            "--tmpfs",
            "/dev/shm",
            "--remount-ro",
            "/dev",
            "--remount-ro",
            "/proc",
            "--remount-ro",
            "/",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "HOME",
            "/work",
            "--setenv",
            "TMPDIR",
            "/tmp",
            "--setenv",
            "LANG",
            "C.UTF-8",
        ]);
        cmd.arg("--chdir")
            .arg(cwd)
            .arg("--seccomp")
            .arg(filter_fd.to_string())
            .args(["--", "/bin/bash", "--noprofile", "--norc", "-c"])
            .arg(&args.command);
        let cpu = args.timeout_ms.div_ceil(1000) + 1;
        // Hard rlimits cannot be raised by the payload. No heap allocation in pre_exec.
        unsafe {
            cmd.pre_exec(move || {
                // Child-only descriptor inheritance; the parent keeps CLOEXEC set.
                if libc::fcntl(filter_fd, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                for (resource, value) in [
                    (libc::RLIMIT_AS, 512 * 1024 * 1024),
                    (libc::RLIMIT_CPU, cpu),
                    (libc::RLIMIT_NPROC, 64),
                    (libc::RLIMIT_NOFILE, 256),
                    (libc::RLIMIT_FSIZE, 16 * 1024 * 1024),
                    (libc::RLIMIT_CORE, 0),
                ] {
                    let limit = libc::rlimit {
                        rlim_cur: value,
                        rlim_max: value,
                    };
                    if libc::setrlimit(resource, &limit) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd
    }
    #[derive(Default)]
    struct Capture {
        bytes: Vec<u8>,
        total: usize,
    }
    async fn drain(
        mut stream: impl AsyncRead + Unpin,
        capture: Arc<Mutex<Capture>>,
    ) -> io::Result<()> {
        let mut chunk = [0u8; 8192];
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            let mut capture = capture.lock().unwrap();
            capture.total = capture.total.saturating_add(n);
            let keep = n.min(CAPTURE_BYTES.saturating_sub(capture.bytes.len()));
            capture.bytes.extend_from_slice(&chunk[..keep]);
        }
    }
    struct ProcessGuard(Option<Child>);
    impl Drop for ProcessGuard {
        fn drop(&mut self) {
            if let Some(mut child) = self.0.take() {
                if child.id().is_some() {
                    // Killing bwrap tears down its private PID namespace, including setsid children.
                    let _ = child.start_kill();
                    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                        runtime.spawn(async move {
                            let _ = child.wait().await;
                        });
                    }
                }
            }
        }
    }
    pub(super) async fn execute(
        root: PathBuf,
        args: BashArgs,
        ctx: ToolContext,
    ) -> Result<ToolResult, AgentError> {
        let started = Instant::now();
        let launcher = bwrap()?;
        let cwd = cwd(&args.cwd)?;
        let cancel = ctx.cancel.child_token();
        let _copy_guard = cancel.clone().drop_guard();
        let inputs = args.inputs.clone();
        // The copy owns its TempDir even if the awaiting request is cancelled.
        let copy = tokio::task::spawn_blocking(move || snapshot(root, inputs, cancel));
        let deadline = tokio::time::Instant::now() + Duration::from_millis(args.timeout_ms);
        let snapshot = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return Err(AgentError::Aborted),
            _ = tokio::time::sleep_until(deadline) => return Err(bash_error("timed out preparing input snapshot")),
            result = copy => result.map_err(bash_error)??,
        };
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        let filter = network_filter()?;
        let child = command(launcher, snapshot.path(), &cwd, &args, filter.as_raw_fd())
            .spawn()
            .map_err(|e| {
                bash_error(format!(
                    "sandbox could not start: {e}; host execution is disabled"
                ))
            })?;
        let mut guard = ProcessGuard(Some(child));
        let stdout = guard.0.as_mut().unwrap().stdout.take().unwrap();
        let stderr = guard.0.as_mut().unwrap().stderr.take().unwrap();
        let out = Arc::new(Mutex::new(Capture::default()));
        let err = Arc::new(Mutex::new(Capture::default()));
        let mut timed_out = false;
        let mut cancelled = false;
        let mut io_error = None;
        let status = {
            let execution = async {
                let (status, stdout, stderr) = tokio::join!(
                    guard.0.as_mut().unwrap().wait(),
                    drain(stdout, out.clone()),
                    drain(stderr, err.clone())
                );
                (status, stdout.and(stderr))
            };
            tokio::pin!(execution);
            tokio::select! {
                biased;
                _ = ctx.cancel.cancelled() => { cancelled = true; None },
                _ = tokio::time::sleep_until(deadline) => { timed_out = true; None },
                (status, output) = &mut execution => {
                    if let Err(e) = output { io_error = Some(e.to_string()); }
                    match status { Ok(status) => Some(status), Err(e) => { io_error = Some(e.to_string()); None } }
                }
            }
        };
        if status.is_none() {
            // Cleanup is bounded; ProcessGuard also handles an externally dropped future.
            let child = guard.0.as_mut().unwrap();
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
        }
        let out = out.lock().unwrap();
        let err = err.lock().unwrap();
        let metadata = json!({
            "sandbox":"bubblewrap", "network_enabled":true, "cwd":cwd,
            "exit_code":status.as_ref().and_then(|s| s.code()), "signal":status.as_ref().and_then(|s| s.signal()),
            "timed_out":timed_out, "cancelled":cancelled, "io_error":io_error,
            "stdout_bytes":out.total, "stderr_bytes":err.total,
            "stdout_truncated":out.total > out.bytes.len(), "stderr_truncated":err.total > err.bytes.len(),
            "duration_ms":started.elapsed().as_millis(), "scratch_persistent":false
        });
        Ok(ToolResult {
            is_error: timed_out
                || cancelled
                || io_error.is_some()
                || !status.is_some_and(|s| s.success()),
            terminate: false,
            // Status stays ahead of the large text when the context plugin abbreviates it.
            content: vec![
                ContentPart::Text(format!("Bash result: {metadata}\n")),
                ContentPart::Text(format!(
                    "stdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&out.bytes),
                    String::from_utf8_lossy(&err.bytes)
                )),
            ],
        })
    }
}

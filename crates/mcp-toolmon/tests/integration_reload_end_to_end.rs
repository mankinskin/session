//! Epic-level end-to-end proof (T7): drives the REAL `mcp-toolmon` binary as
//! a subprocess over stdin/stdout, exactly as an MCP client would, through a
//! full transparent binary swap detected by the real on-disk watcher (not a
//! direct `swap_child()` call — no in-process shortcut is available once the
//! proxy is an external process).

use std::{
    ffi::OsString,
    fs,
    io::{
        BufRead,
        BufReader,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Stdio,
    },
    sync::{
        Arc,
        Mutex,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
    thread,
    time::{
        Duration,
        Instant,
    },
};

use serde_json::{
    Value,
    json,
};
use session_api::{
    SessionStoreConfig,
    SessionWorktreeCheckInRequest,
};
use session_workspace_resolver::{
    ResolverConfig,
    SessionWorkspaceResolver,
};
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn active_session_fixture() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let main_checkout = temp.path().join("repository");
    let session_id = "11111111-1111-4111-8111-111111111111";
    let worktree = main_checkout
        .join(".worktrees")
        .join(session_id)
        .join("feature");
    assert!(
        Command::new("git")
            .current_dir(temp.path())
            .args(["init", "--quiet", &main_checkout.to_string_lossy()])
            .status()
            .unwrap()
            .success()
    );
    for args in [
        ["config", "user.email", "tests@example.invalid"],
        ["config", "user.name", "mcp-toolmon reload tests"],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&main_checkout)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(main_checkout.join("README.md"), "fixture\n").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&main_checkout)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&main_checkout)
            .args(["commit", "--quiet", "-m", "fixture"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&main_checkout)
            .args(["worktree", "add", "--quiet", "-b", "agent/test"])
            .arg(&worktree)
            .arg("HEAD")
            .status()
            .unwrap()
            .success()
    );
    SessionWorkspaceResolver::new(ResolverConfig {
        main_checkout: main_checkout.clone(),
        workspace_path: "default".to_string(),
    })
    .unwrap();
    // The anchor store beneath the main checkout is the worktree registry.
    SessionStoreConfig::new(main_checkout.join(".session"))
        .check_in_worktree(SessionWorktreeCheckInRequest {
            session_id: session_id.to_string(),
            owner_id: "agent".to_string(),
            ticket_id: "ticket".to_string(),
            worktree_path: worktree.clone(),
            branch: "agent/test".to_string(),
            predecessor_session_id: None,
        })
        .unwrap();
    (temp, main_checkout)
}

struct ActiveSessionEnvironment {
    _lock: std::sync::MutexGuard<'static, ()>,
    _temp: TempDir,
    main_checkout: PathBuf,
    previous_main_checkout: Option<OsString>,
}

impl Drop for ActiveSessionEnvironment {
    fn drop(&mut self) {
        unsafe {
            match &self.previous_main_checkout {
                Some(value) => std::env::set_var("MCP_MAIN_CHECKOUT", value),
                None => std::env::remove_var("MCP_MAIN_CHECKOUT"),
            }
        }
    }
}

fn active_session_environment() -> ActiveSessionEnvironment {
    let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let previous_main_checkout = std::env::var_os("MCP_MAIN_CHECKOUT");
    let (temp, main_checkout) = active_session_fixture();
    unsafe { std::env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    ActiveSessionEnvironment {
        _lock: lock,
        _temp: temp,
        main_checkout,
        previous_main_checkout,
    }
}

fn get_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcp-toolmon"))
}

fn fake_v1_bytes() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_fake-mcp-v1")).unwrap()
}

fn fake_v2_bytes() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_fake-mcp-v2")).unwrap()
}

fn canonical_exe_name() -> &'static str {
    if cfg!(windows) {
        "canonical.exe"
    } else {
        "canonical"
    }
}

fn write_exe(
    path: &Path,
    bytes: &[u8],
) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

/// Collects the child's stdout lines in the background so the test can do
/// bounded (deadline) waits instead of blocking reads. `eof` flips true the
/// moment the pipe closes — the client-connection-never-dropped assertion.
struct Transcript {
    lines: Arc<Mutex<Vec<String>>>,
    eof: Arc<AtomicBool>,
}

fn spawn_collector(stdout: std::process::ChildStdout) -> Transcript {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let eof = Arc::new(AtomicBool::new(false));
    let (lines2, eof2) = (Arc::clone(&lines), Arc::clone(&eof));
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    eof2.store(true, Ordering::SeqCst);
                    break;
                },
                Ok(_) =>
                    lines2.lock().unwrap().push(line.trim_end().to_string()),
            }
        }
    });
    Transcript { lines, eof }
}

/// Bounded wait (deadline, not a bare sleep as the sync primitive).
fn wait_until<F: Fn(&[String]) -> bool>(
    t: &Transcript,
    timeout: Duration,
    msg: &str,
    pred: F,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if pred(&t.lines.lock().unwrap()) {
            return;
        }
        if Instant::now() >= deadline {
            panic!("{msg}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn parsed(lines: &[String]) -> Vec<Value> {
    lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn find_response(
    lines: &[String],
    id: i64,
) -> Option<Value> {
    parsed(lines)
        .into_iter()
        .find(|v| v.get("id").and_then(Value::as_i64) == Some(id))
}

/// Windows job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: any process
/// assigned to it (and any descendant it spawns, since job membership is
/// inherited by default) is terminated the moment the job handle closes —
/// used so a panic or interrupted run can never leave the spawned
/// `mcp-toolmon.exe` or its shadow-copied `canonical.exe` grandchild running
/// and holding a file lock (see ticket 276446e8).
#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl WindowsJob {
    fn new() -> Self {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOBOBJECT_BASIC_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JobObjectExtendedLimitInformation,
            CreateJobObjectW,
            SetInformationJobObject,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            assert!(!job.is_null(), "CreateJobObjectW failed");
            let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    ..std::mem::zeroed()
                },
                ..std::mem::zeroed()
            };
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()
                    as u32,
            );
            assert!(ok != 0, "SetInformationJobObject failed");
            Self(job)
        }
    }

    /// Best-effort: losing the race against the child exiting immediately
    /// after spawn only forfeits the cleanup guarantee, never the test.
    fn assign(&self, child: &std::process::Child) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        unsafe {
            let handle = child.as_raw_handle()
                as windows_sys::Win32::Foundation::HANDLE;
            let _ = AssignProcessToJobObject(self.0, handle);
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// RAII guard around the spawned `mcp-toolmon` subprocess: kills (and waits
/// on) it on drop — including the unwind path from a test panic — so a
/// failing assertion can never leak an orphaned process holding a file lock
/// on the target binary (ticket 276446e8). On Windows this also tears down
/// the shadow-copied `canonical.exe` grandchild via a job object, since
/// killing the direct child alone does not kill its own children.
struct ChildGuard {
    child: std::process::Child,
    // Held only for its Drop side effect (job-close kills the process tree).
    #[cfg_attr(windows, allow(dead_code))]
    #[cfg(windows)]
    job: WindowsJob,
}

impl ChildGuard {
    fn spawn(mut command: Command) -> std::io::Result<Self> {
        #[cfg(windows)]
        let job = WindowsJob::new();
        let child = command.spawn()?;
        #[cfg(windows)]
        job.assign(&child);
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }
}

impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn transparent_reload_end_to_end_subprocess() {
    let session_environment = active_session_environment();
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let mut command = Command::new(get_binary_path());
    command
        .arg("--")
        .arg(&canonical)
        .env("TOOLMON_POLL_MS", "25")
        .env("TOOLMON_DRAIN_MS", "200")
        .env("MCP_MAIN_CHECKOUT", &session_environment.main_checkout)
        .env_remove("COST_GATE_TABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = ChildGuard::spawn(command)
        .expect("failed to spawn real mcp-toolmon binary");

    let mut stdin = child.stdin.take().unwrap();
    let transcript = spawn_collector(child.stdout.take().unwrap());

    let mut send = |v: &Value| {
        writeln!(stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
        stdin.flush().unwrap();
    };

    // 1) Real handshake.
    send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0.0"}}}),
    );
    wait_until(
        &transcript,
        Duration::from_secs(5),
        "no initialize response observed",
        |lines| find_response(lines, 1).is_some(),
    );
    send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    // 2) tools/call served by v1.
    send(
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"generation","arguments":{"session_id":"11111111-1111-4111-8111-111111111111"}}}),
    );
    wait_until(
        &transcript,
        Duration::from_secs(5),
        "no response to pre-swap generation call",
        |lines| find_response(lines, 2).is_some(),
    );
    let v1_resp = find_response(&transcript.lines.lock().unwrap(), 2).unwrap();
    assert_eq!(v1_resp["result"]["content"][0]["text"], "v1");

    // 3) Overwrite the canonical path while the proxy runs — the
    // lock-freedom property; this must succeed because mcp-toolmon only
    // ever executes a shadow copy of P, never P itself.
    let overwrite =
        std::panic::catch_unwind(|| write_exe(&canonical, &fake_v2_bytes()));
    assert!(
        overwrite.is_ok(),
        "overwriting canonical path P while mcp-toolmon runs must succeed"
    );

    // Race a request right at the swap boundary: this must be answered
    // (real v1/v2 result or a synthesized JSON-RPC error), never hang.
    send(
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"generation","arguments":{"session_id":"11111111-1111-4111-8111-111111111111"}}}),
    );

    // 4) Bounded wait for the real on-disk watcher to detect the change and
    // notify the client — no direct swap_child() call is possible against
    // an external process, so this exercises the real poller end to end.
    wait_until(
        &transcript,
        Duration::from_secs(10),
        "notifications/tools/list_changed was never observed after the real file swap",
        |lines| {
            parsed(lines).iter().any(|v| {
                v.get("method").and_then(Value::as_str)
                    == Some("notifications/tools/list_changed")
            })
        },
    );

    // Connection must never have dropped up to this point.
    assert!(
        !transcript.eof.load(Ordering::SeqCst),
        "client stdout hit EOF before the session ended"
    );

    // The racing id=3 request must have been answered by now (never a hang).
    wait_until(
        &transcript,
        Duration::from_secs(5),
        "request id=3 (in flight at swap boundary) was never answered",
        |lines| find_response(lines, 3).is_some(),
    );

    // 5) Post-swap call served by v2 (retry loop: the swap may still be
    // finishing draining/respawning for a brief moment after list_changed
    // was queued). Each attempt gets its own bounded (non-panicking) wait
    // so a slow-but-eventual respawn doesn't abort the whole assertion.
    fn wait_or_none(
        t: &Transcript,
        timeout: Duration,
        id: i64,
    ) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = find_response(&t.lines.lock().unwrap(), id) {
                return Some(v);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    let overall_deadline = Instant::now() + Duration::from_secs(15);
    let mut next_id = 4i64;
    let mut got_v2 = false;
    while Instant::now() < overall_deadline {
        send(
            &json!({"jsonrpc":"2.0","id":next_id,"method":"tools/call","params":{"name":"generation","arguments":{"session_id":"11111111-1111-4111-8111-111111111111"}}}),
        );
        if let Some(resp) =
            wait_or_none(&transcript, Duration::from_secs(2), next_id)
            && resp["result"]["content"][0]["text"] == "v2"
        {
            got_v2 = true;
            break;
        }
        next_id += 1;
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        got_v2,
        "a post-swap generation call must eventually be served by v2 within the overall bound"
    );

    // 6) Never asked to re-initialize: exactly one line carrying an
    // initialize-shaped result (protocolVersion) across the whole session.
    let init_like = parsed(&transcript.lines.lock().unwrap())
        .into_iter()
        .filter(|v| {
            v.get("result")
                .and_then(|r| r.get("protocolVersion"))
                .is_some()
        })
        .count();
    assert_eq!(
        init_like, 1,
        "client must receive exactly one initialize response for the whole session"
    );

    // 7) No request id went unanswered across the entire session.
    for id in [1i64, 2, 3] {
        assert!(
            find_response(&transcript.lines.lock().unwrap(), id).is_some(),
            "id={id} was never answered"
        );
    }

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(
        status.success() || cfg!(windows),
        "proxy should exit cleanly on client stdin EOF, got {status:?}"
    );
}

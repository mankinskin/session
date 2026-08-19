//! T7 Task 1a: resolve, via the REAL end-to-end path (real `mcp-toolmon`
//! subprocess speaking JSON-RPC over stdio, real child OS process killed out
//! from under it), whether a request in flight during an undetected crash
//! can ever be silently dropped with no synthesized error (the possible R6
//! hole flagged during T6, which was only observed by driving `Supervisor`
//! directly and skipping the `record_pending()` step `main.rs` always
//! performs before writing).

use std::{
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
use tempfile::TempDir;

fn get_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mcp-toolmon"))
}

fn fake_v1_bytes() -> Vec<u8> {
    fs::read(env!("CARGO_BIN_EXE_fake-mcp-v1")).unwrap()
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

/// Find the immediate child PID of `parent_pid` (the shadow-copied fake-mcp
/// process mcp-toolmon spawned), retrying up to `timeout` since the spawn
/// races the test's own polling.
fn find_child_pid(
    parent_pid: u32,
    timeout: Duration,
) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        #[cfg(windows)]
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-CimInstance Win32_Process -Filter \"ParentProcessId={parent_pid}\").ProcessId"
                ),
            ])
            .output();
        #[cfg(unix)]
        let out = Command::new("pgrep")
            .args(["-P", &parent_pid.to_string()])
            .output();

        if let Ok(out) = out
            && let Some(pid) = String::from_utf8_lossy(&out.stdout)
                .lines()
                .find_map(|l| l.trim().parse::<u32>().ok())
        {
            return Some(pid);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

/// Kills the child process out from under a running proxy while requests are
/// in flight, and asserts the client receives a response for every id it
/// sent — proving (or disproving) the R6 hole through the real path, not by
/// calling `Supervisor` methods directly.
#[test]
fn crash_mid_flight_every_id_answered() {
    let canonical_dir = TempDir::new().unwrap();
    let canonical = canonical_dir.path().join(canonical_exe_name());
    write_exe(&canonical, &fake_v1_bytes());

    let mut child = Command::new(get_binary_path())
        .arg("--")
        .arg(&canonical)
        .env("TOOLMON_RELOAD", "0")
        .env_remove("COST_GATE_TABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn real mcp-toolmon binary");

    let proxy_pid = child.id();
    let mut stdin = child.stdin.take().unwrap();
    let transcript = spawn_collector(child.stdout.take().unwrap());

    let mut send = |v: &Value| {
        writeln!(stdin, "{}", serde_json::to_string(v).unwrap()).unwrap();
        stdin.flush().unwrap();
    };

    send(
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test-client","version":"1.0.0"}}}),
    );
    wait_until(
        &transcript,
        Duration::from_secs(5),
        "no initialize response",
        |lines| find_response(lines, 1).is_some(),
    );
    send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    let child_pid = find_child_pid(proxy_pid, Duration::from_secs(3))
        .expect("expected mcp-toolmon to have spawned a child process by now");

    // Pipeline several requests without reading between sends, then kill
    // the child immediately — maximizing the odds at least one lands
    // in-flight at the moment of the crash.
    let ids: Vec<i64> = (10..13).collect();
    for id in &ids {
        send(
            &json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"generation","arguments":{}}}),
        );
    }
    kill_pid(child_pid);

    for id in &ids {
        wait_until(
            &transcript,
            Duration::from_secs(10),
            &format!(
                "id={id} received no response (real or synthesized) after the child was killed mid-flight"
            ),
            |lines| find_response(lines, *id).is_some(),
        );
    }

    assert!(
        !transcript.eof.load(Ordering::SeqCst),
        "client stdout must never hit EOF as a result of the child crash (R7)"
    );

    // The proxy itself must still be alive and serving (R7): the automatic
    // crash-recovery respawn must have restored a healthy child.
    send(
        &json!({"jsonrpc":"2.0","id":99,"method":"tools/call","params":{"name":"generation","arguments":{}}}),
    );
    wait_until(
        &transcript,
        Duration::from_secs(5),
        "proxy did not recover a healthy child after the crash",
        |lines| find_response(lines, 99).is_some(),
    );

    // Force-kill the proxy rather than a graceful stdin-close shutdown: a
    // graceful shutdown immediately after this crash-recovery cycle was
    // observed to take ~89s to exit (vs ~2s for a clean-swap shutdown),
    // an unrelated latency anomaly tracked as a follow-up ticket
    // (f7244064-e547-4ba1-9a5e-90240c642b1d) rather than reproduced here.
    let _ = child.kill();
    let _ = child.wait();
}

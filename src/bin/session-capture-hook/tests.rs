use std::{
    env,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

use session_api::SessionStoreConfig;
use tempfile::tempdir;

use super::{
    checkout_for_store_root, infer_capture_worktree, initialize_session_routing,
    resolve_capture_store_root,
};
use crate::args::normalize_transcript_path;

struct PoisonTolerantMutex(Mutex<()>);

impl PoisonTolerantMutex {
    const fn new() -> Self {
        Self(Mutex::new(()))
    }

    fn lock(&self) -> PoisonTolerantLock<'_> {
        PoisonTolerantLock(self.0.lock())
    }
}

struct PoisonTolerantLock<'a>(std::sync::LockResult<MutexGuard<'a, ()>>);

impl<'a> PoisonTolerantLock<'a> {
    fn unwrap(self) -> MutexGuard<'a, ()> {
        self.0.unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

static CWD_LOCK: PoisonTolerantMutex = PoisonTolerantMutex::new();
static ENV_LOCK: PoisonTolerantMutex = PoisonTolerantMutex::new();

fn create_git_checkout(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(&["init", "--quiet", "--initial-branch", "main"], path);
    git(&["config", "user.email", "hook@example.com"], path);
    git(&["config", "user.name", "hook"], path);
    git(&["commit", "--quiet", "--allow-empty", "-m", "init"], path);
}

fn register_active_worktree(main_checkout: &Path, session_id: &str) -> PathBuf {
    let worktree = main_checkout
        .join(".worktrees")
        .join(session_id)
        .join("capture");
    create_git_worktree(
        main_checkout,
        &worktree,
        &format!("agent/{session_id}/capture"),
    );
    SessionStoreConfig::new(main_checkout.join(".workflow-tools/session"))
        .check_in_worktree(session_api::SessionWorktreeCheckInRequest {
            session_id: session_id.to_string(),
            owner_id: "test".to_string(),
            ticket_id: "test".to_string(),
            worktree_path: worktree.clone(),
            branch: format!("agent/{session_id}/capture"),
            predecessor_session_id: None,
        })
        .unwrap();
    worktree
}

#[test]
fn capture_writes_to_worktree_store_while_cwd_is_main_checkout() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let worktree = register_active_worktree(&main_checkout, "44444444-4444-4444-8444-444444444444");
    std::fs::create_dir_all(main_checkout.join(".workflow-tools/session")).unwrap();
    let original_cwd = std::env::current_dir().unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    std::env::set_current_dir(&main_checkout).unwrap();

    let store_root = resolve_capture_store_root(None, Some("44444444-4444-4444-8444-444444444444"))
        .expect("active worktree assignment should resolve");

    let transcript_path = fixture.path().join("capture.jsonl");
    std::fs::write(
        &transcript_path,
        include_str!("../../../tests/fixtures/local_parse_fixture_a.jsonl"),
    )
    .unwrap();
    let config = SessionStoreConfig::new(&store_root);
    let plan = config
        .capture_copilot_transcript_with_tool_response(&transcript_path, "Stop", None)
        .expect("capture should persist into the resolved worktree store");

    std::env::set_current_dir(original_cwd).unwrap();
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
    assert_eq!(store_root, worktree.join(".workflow-tools/session"));
    let record = config
        .read_session(&plan.record.session_id)
        .expect("captured session should be readable from the worktree store");
    assert_eq!(record.session_id, plan.record.session_id);
    assert!(
        !main_checkout
            .join(".workflow-tools/session")
            .join("sessions")
            .join(&plan.record.session_id)
            .exists(),
        "capture must not write a same-session record into the main-checkout store"
    );
}

#[test]
fn capture_without_assignment_uses_canonical_main_store_when_legacy_exists() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    create_git_worktree(&main_checkout, &main_checkout.join("seed-worktree"), "seed");
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };

    assert_eq!(
        resolve_capture_store_root(None, Some("66666666-6666-4666-8666-666666666666")),
        Some(main_checkout.join(".workflow-tools/session"))
    );
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
}

#[test]
fn capture_without_assignment_prefers_canonical_store_layout() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    create_git_worktree(&main_checkout, &main_checkout.join("seed-worktree"), "seed");
    let canonical = main_checkout.join(".workflow-tools").join("session");
    std::fs::create_dir_all(&canonical).unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };

    let resolved = resolve_capture_store_root(None, Some("66666666-6666-4666-8666-666666666666"));

    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
    assert_eq!(resolved, Some(canonical));
}

#[test]
fn checkout_for_store_root_steps_over_canonical_stores_dir() {
    let root = Path::new("/repo");
    assert_eq!(
        checkout_for_store_root(&root.join(".workflow-tools/session")),
        Some(root)
    );
    assert_eq!(checkout_for_store_root(&root.join(".session")), Some(root));
}

#[test]
fn capture_ignores_leftover_checkouts_but_does_not_ignore_broken_registrations() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let session_id = "66666666-6666-4666-8666-666666666666";
    register_active_worktree(&main_checkout, session_id);
    let registry = main_checkout
        .join(".workflow-tools/session/local/worktrees")
        .join(format!("{session_id}.json"));
    std::fs::remove_file(&registry).unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    let unregistered = resolve_capture_store_root(None, Some(session_id));
    std::fs::write(&registry, b"not json").unwrap();
    let broken = resolve_capture_store_root(None, Some(session_id));
    let invalid_id = resolve_capture_store_root(None, Some("not-a-uuid"));
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
    assert_eq!(
        unregistered,
        Some(main_checkout.join(".workflow-tools/session"))
    );
    assert_eq!(broken, None);
    assert_eq!(invalid_id, None);
}

#[test]
fn capture_store_resolution_ignores_process_current_directory() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let worktree = register_active_worktree(&main_checkout, "55555555-5555-4555-8555-555555555555");
    std::fs::create_dir_all(main_checkout.join(".workflow-tools/session")).unwrap();
    let unrelated = fixture.path().join("unrelated");
    std::fs::create_dir_all(&unrelated).unwrap();
    let original_cwd = std::env::current_dir().unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    std::env::set_current_dir(&unrelated).unwrap();

    let result = resolve_capture_store_root(None, Some("55555555-5555-4555-8555-555555555555"));

    std::env::set_current_dir(original_cwd).unwrap();
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
    assert_eq!(result, Some(worktree.join(".workflow-tools/session")));
}

#[test]
fn capture_inference_uses_resolved_store_parent_not_process_directory() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let worktree = main_checkout.join("worktree");
    create_git_worktree(&main_checkout, &worktree, "feature");
    let store_root = worktree.join(".session");
    let config = SessionStoreConfig::new(&store_root);
    let original_cwd = env::current_dir().unwrap();
    env::set_current_dir(&main_checkout).unwrap();

    infer_capture_worktree(&config, "session-store-parent", &store_root).unwrap();

    env::set_current_dir(original_cwd).unwrap();
    let record = config.read_session("session-store-parent").unwrap();
    assert_eq!(
        record
            .metadata
            .worktree
            .unwrap()
            .path
            .canonicalize()
            .unwrap(),
        worktree.canonicalize().unwrap()
    );
}

#[test]
fn normalize_transcript_path_keeps_plain_paths() {
    let path = PathBuf::from("C:/repo/transcript.jsonl");
    let normalized = normalize_transcript_path(&path);
    assert!(!normalized.as_os_str().is_empty());
}

fn git(args: &[&str], cwd: &Path) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", cwd.display());
}

/// Creates a real repository plus a real linked worktree.
///
/// Worktree inference shells out to `git rev-parse`, so a fixture that only
/// fabricates a `.git` entry would silently no-op instead of assigning.
fn create_git_worktree(main_checkout: &Path, worktree: &Path, branch: &str) {
    std::fs::create_dir_all(main_checkout).unwrap();
    git(&["init", "--quiet"], main_checkout);
    git(&["config", "user.email", "hook@example.com"], main_checkout);
    git(&["config", "user.name", "hook"], main_checkout);
    git(
        &["commit", "--quiet", "--allow-empty", "-m", "init"],
        main_checkout,
    );
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            worktree.to_str().unwrap(),
        ],
        main_checkout,
    );
}

fn run_session_start(main_checkout: &Path, process_directory: &Path, session_id: Option<&str>) {
    let original_cwd = env::current_dir().unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", main_checkout) };
    env::set_current_dir(process_directory).unwrap();

    initialize_session_routing(session_id, None);

    env::set_current_dir(original_cwd).unwrap();
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
}

#[test]
fn session_start_does_not_provision_a_worktree_without_registration() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    create_git_checkout(&main_checkout);
    std::fs::create_dir_all(main_checkout.join(".session")).unwrap();
    let session_id = "99999999-9999-4999-8999-999999999999";

    run_session_start(&main_checkout, &main_checkout, Some(session_id));

    assert!(!main_checkout.join(".worktrees").exists());
    assert!(!main_checkout.join(".session").join("sessions").exists());
}

#[test]
fn session_start_without_session_id_or_with_blank_id_does_not_assign() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let worktree = main_checkout.join("worktree");
    create_git_worktree(&main_checkout, &worktree, "feature");

    run_session_start(&main_checkout, &worktree, None);
    run_session_start(&main_checkout, &worktree, Some("   "));

    assert!(!main_checkout.join(".session").exists());
    assert!(!main_checkout.join(".worktrees").exists());
}

#[test]
fn non_session_start_does_not_provision_or_assign() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let worktree = main_checkout.join("worktree");
    create_git_worktree(&main_checkout, &worktree, "feature");
    let original_cwd = env::current_dir().unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &main_checkout) };
    env::set_current_dir(&worktree).unwrap();

    initialize_session_routing(Some("session-one"), Some(&main_checkout.join(".session")));

    env::set_current_dir(original_cwd).unwrap();
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
    assert!(!main_checkout.join(".session").exists());
    assert!(!main_checkout.join(".worktrees").exists());
}

#[test]
fn session_start_skips_a_missing_anchor_override() {
    let _cwd_lock = CWD_LOCK.lock().unwrap();
    let _env_lock = ENV_LOCK.lock().unwrap();
    let fixture = tempdir().unwrap();
    let main_checkout = fixture.path().join("main");
    let worktree = main_checkout.join("worktree");
    let invalid_main_checkout = fixture.path().join("missing-main");
    create_git_worktree(&main_checkout, &worktree, "feature");
    let original_cwd = env::current_dir().unwrap();
    let original_main_checkout = env::var_os("MCP_MAIN_CHECKOUT");
    unsafe { env::set_var("MCP_MAIN_CHECKOUT", &invalid_main_checkout) };
    env::set_current_dir(&worktree).unwrap();

    initialize_session_routing(
        Some("session-one"),
        Some(&invalid_main_checkout.join(".session")),
    );

    env::set_current_dir(original_cwd).unwrap();
    unsafe {
        match original_main_checkout {
            Some(value) => env::set_var("MCP_MAIN_CHECKOUT", value),
            None => env::remove_var("MCP_MAIN_CHECKOUT"),
        }
    }
    assert!(!invalid_main_checkout.exists());
}

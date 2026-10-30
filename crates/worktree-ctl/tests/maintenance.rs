use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use tempfile::TempDir;

const SESSION_UUID: &str = "12345678-1234-1234-1234-123456789abc";

struct Fixture {
    _temp: TempDir,
    main: PathBuf,
    tool: PathBuf,
}

impl Fixture {
    fn worktree(&self, slug: &str) -> PathBuf {
        self.main.join(".worktrees").join(SESSION_UUID).join(slug)
    }

    fn legacy_worktree(&self, name: &str) -> PathBuf {
        self.main.join(".worktrees").join(name)
    }

    fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(&self.tool)
            .args(args)
            .current_dir(&self.main)
            .env("GIT_ALLOW_PROTOCOL", "file")
            .output()
            .expect("tool starts")
    }
}

fn fixture_repo() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("submodule-source");
    let main = temp.path().join("main");
    init_repo(&source);
    fs::write(source.join("file.txt"), "initial\n").expect("write source");
    git(&source, &["add", "file.txt"]);
    git(&source, &["commit", "-m", "initial"]);

    init_repo(&main);
    fs::write(main.join("README"), "fixture\n").expect("write readme");
    git(&main, &["add", "README"]);
    git(&main, &["commit", "-m", "initial"]);
    git(
        &main,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.to_str().expect("utf-8 path"),
            "modules/example",
        ],
    );
    git(&main, &["commit", "-m", "add submodule"]);

    Fixture {
        _temp: temp,
        main,
        tool: PathBuf::from(env!("CARGO_BIN_EXE_worktree-ctl")),
    }
}

fn init_repo(path: &Path) {
    git_in(
        path.parent().expect("repo parent"),
        &[
            "init",
            "--initial-branch=main",
            path.to_str().expect("utf-8 path"),
        ],
    );
    git(path, &["config", "user.email", "test@example.invalid"]);
    git(path, &["config", "user.name", "test"]);
}

fn git(repository: &Path, arguments: &[&str]) {
    git_in(repository, arguments);
}

fn git_in(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("git starts");
    assert!(output.status.success(), "git failed: {}", all(&output));
}

fn git_revision(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .output()
        .expect("git starts");
    assert!(output.status.success(), "git failed: {}", all(&output));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn all(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn visible_width(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut index = 0;
    let mut width = 0;
    while index < bytes.len() {
        if bytes[index] == b'\x1b' && bytes.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < bytes.len() && !(b'@'..=b'~').contains(&bytes[index]) {
                index += 1;
            }
            index += usize::from(index < bytes.len());
        } else {
            width += 1;
            index += 1;
        }
    }
    width
}
fn create(fixture: &Fixture, slug: &str) {
    let output = fixture.run(["new", SESSION_UUID, slug]);
    assert!(output.status.success(), "new failed: {}", all(&output));
}

fn write_owned_session_record(worktree: &Path, captured_at: Option<&str>) {
    write_session_record(worktree, worktree, captured_at);
}

fn write_session_record(root: &Path, owner_worktree: &Path, captured_at: Option<&str>) {
    let session_dir = root
        .join(".workflow-tools/session")
        .join("sessions")
        .join(SESSION_UUID);
    fs::create_dir_all(&session_dir).expect("create local session record");
    let captured_at = captured_at
        .map(|value| format!(",\"captured_at\":\"{value}\""))
        .unwrap_or_default();
    fs::write(
        session_dir.join("session.json"),
        format!(
            "{{\"session_id\":\"{SESSION_UUID}\"{captured_at},\"metadata\":{{\"worktree\":{{\"path\":\"{}\"}}}}}}",
            owner_worktree.display().to_string().replace('\\', "\\\\")
        ),
    )
    .expect("write local session record");
}

fn create_legacy_worktree(fixture: &Fixture, name: &str) -> PathBuf {
    let worktree = fixture.legacy_worktree(name);
    git(
        &fixture.main,
        &[
            "worktree",
            "add",
            "-b",
            &format!("agent/{name}"),
            worktree.to_str().expect("utf-8 worktree path"),
            "main",
        ],
    );
    worktree
}

#[test]
fn new_creates_nested_worktree_and_branch() {
    let fixture = fixture_repo();

    create(&fixture, "created");

    assert!(fixture.worktree("created").is_dir());
    assert!(
        git_revision(
            &fixture.main,
            &[
                "branch",
                "--list",
                "agent/12345678-1234-1234-1234-123456789abc/created",
            ],
        )
        .contains("agent/12345678-1234-1234-1234-123456789abc/created")
    );
}

#[test]
fn rename_reslugs_nested_worktree_and_branch() {
    let fixture = fixture_repo();
    create(&fixture, "old");

    let output = fixture.run([
        "rename",
        "12345678-1234-1234-1234-123456789abc/old",
        "12345678-1234-1234-1234-123456789abc/new",
    ]);

    assert!(output.status.success(), "rename failed: {}", all(&output));
    assert!(!fixture.worktree("old").exists());
    assert!(fixture.worktree("new").is_dir());
    assert!(
        git_revision(
            &fixture.main,
            &[
                "branch",
                "--list",
                "agent/12345678-1234-1234-1234-123456789abc/new",
            ],
        )
        .contains("agent/12345678-1234-1234-1234-123456789abc/new")
    );
}

#[test]
fn remove_keeps_nonempty_session_parent_then_cleans_it() {
    let fixture = fixture_repo();
    create(&fixture, "first");
    let second = fixture.worktree("second");
    git(
        &fixture.main,
        &[
            "worktree",
            "add",
            "-b",
            "agent/12345678-1234-1234-1234-123456789abc/second",
            second.to_str().expect("utf-8 worktree path"),
            "main",
        ],
    );

    let first = fixture.run([
        "remove",
        "12345678-1234-1234-1234-123456789abc/first",
        "--force",
    ]);
    assert!(first.status.success(), "remove failed: {}", all(&first));
    assert!(fixture.main.join(".worktrees").join(SESSION_UUID).is_dir());

    let second = fixture.run([
        "remove",
        "12345678-1234-1234-1234-123456789abc/second",
        "--force",
    ]);
    assert!(second.status.success(), "remove failed: {}", all(&second));
    assert!(!fixture.main.join(".worktrees").join(SESSION_UUID).exists());
}

#[test]
fn nested_new_dry_run_preserves_status() {
    let fixture = fixture_repo();
    let before = git_revision(&fixture.main, &["status", "--short"]);

    let output = fixture.run(["new", SESSION_UUID, "dry-run", "--dry-run"]);

    assert!(output.status.success(), "dry-run failed: {}", all(&output));
    assert!(!fixture.worktree("dry-run").exists());
    assert_eq!(before, git_revision(&fixture.main, &["status", "--short"]));
}

#[test]
fn list_reports_lifecycle_state_and_rejection_reason() {
    let fixture = fixture_repo();
    create(&fixture, "state");
    let legacy = fixture.legacy_worktree("legacy-state");
    git(
        &fixture.main,
        &[
            "worktree",
            "add",
            "-b",
            "agent/legacy-state",
            legacy.to_str().expect("utf-8 worktree path"),
            "main",
        ],
    );

    let output = fixture.run(["list", "--verbose", "--dry-run"]);

    assert!(output.status.success(), "list failed: {}", all(&output));
    let report = all(&output);
    assert!(report.contains("worktree: "), "{report}");
    assert!(
        report.contains(
            "superproject: branch=agent/12345678-1234-1234-1234-123456789abc/state changes=clean ahead=0 behind=0"
        ),
        "{report}"
    );
    assert!(
        report.contains("superproject: branch=agent/legacy-state changes=clean ahead=0 behind=0"),
        "{report}"
    );
    assert!(
        report.contains("modules/example: branch=HEAD changes=clean ahead=0 behind=0"),
        "{report}"
    );
    assert!(report.contains("modules/example: unavailable"), "{report}");
    assert!(
        report.contains("lifecycle: preserved reason=not-idle"),
        "{report}"
    );
}

#[test]
fn list_reports_dirty_superproject_and_ahead_submodule() {
    let fixture = fixture_repo();
    create(&fixture, "live-state");
    let worktree = fixture.worktree("live-state");
    fs::write(worktree.join("pending.txt"), "pending\n")
        .expect("write pending superproject change");
    let submodule = worktree.join("modules/example");
    fs::write(submodule.join("file.txt"), "initial\nahead\n").expect("write submodule change");
    git(&submodule, &["commit", "-am", "ahead"]);

    let output = fixture.run(["list"]);

    assert!(output.status.success(), "list failed: {}", all(&output));
    let report = all(&output);
    assert!(report.contains(".worktrees"), "{report}");
    assert!(report.contains("[held]"), "{report}");
    assert!(report.contains("dirty:super"), "{report}");
    assert!(report.contains("ahead:modules/example+1"), "{report}");
    assert!(report.contains("super="), "{report}");
    assert!(report.contains("modules/example="), "{report}");
    assert!(!report.contains("branch="), "{report}");
    assert!(!report.contains("ahead=0"), "{report}");
    let lines = report.lines().collect::<Vec<_>>();
    assert!(lines.len() >= 3, "{report}");
    assert!(
        lines.iter().all(|line| visible_width(line) <= 100),
        "compact output exceeds 100 visible characters: {report}"
    );
}
#[test]
fn doctor_repairs_stale_core_worktree() {
    let fixture = fixture_repo();
    let config = fixture
        .main
        .join(".git")
        .join("modules")
        .join("modules")
        .join("example")
        .join("config");
    let missing = fixture
        .main
        .join(".worktrees")
        .join("manually-deleted")
        .join("modules/example");
    git(
        &fixture.main,
        &[
            "config",
            "--file",
            config.to_str().expect("utf-8 config path"),
            "core.worktree",
            missing.to_str().expect("utf-8 missing path"),
        ],
    );

    let output = fixture.run(["doctor"]);

    assert!(output.status.success(), "doctor failed: {}", all(&output));
    assert!(
        all(&output).contains("stale-core-worktree"),
        "{}",
        all(&output)
    );
    let config_output = Command::new("git")
        .args([
            "config",
            "--file",
            config.to_str().expect("utf-8 config path"),
            "--get",
            "core.worktree",
        ])
        .current_dir(&fixture.main)
        .output()
        .expect("config read starts");
    assert!(
        !config_output.status.success(),
        "core.worktree remains configured"
    );
    let status = Command::new("git")
        .args(["status", "--short"])
        .current_dir(&fixture.main)
        .output()
        .expect("status starts");
    assert!(status.status.success(), "status failed: {}", all(&status));
}

#[test]
fn clean_refuses_checkpoint_when_an_unrelated_path_is_dirty() {
    let fixture = fixture_repo();
    create(&fixture, "session-plus-code");
    let worktree = fixture.worktree("session-plus-code");
    write_owned_session_record(&worktree, None);
    fs::write(worktree.join("unrelated.txt"), "keep me\n").expect("write unrelated change");

    let output = fixture.run([
        "clean",
        "12345678-1234-1234-1234-123456789abc/session-plus-code",
    ]);

    assert!(output.status.success(), "clean failed: {}", all(&output));
    assert!(
        worktree.is_dir(),
        "unrelated changes must preserve worktree"
    );
    let status = git_revision(&worktree, &["status", "--porcelain"]);
    assert!(status.contains("unrelated.txt"), "{status}");
    assert!(
        status.contains(".workflow-tools/"),
        "session artifacts must remain uncommitted when unrelated changes exist: {status}"
    );
}

#[test]
fn commit_stages_only_requested_pathspecs() {
    let fixture = fixture_repo();
    create(&fixture, "commit-filter");
    let worktree = fixture.worktree("commit-filter");
    fs::write(worktree.join("included.txt"), "included\n").expect("write included change");
    fs::write(worktree.join("excluded.txt"), "excluded\n").expect("write excluded change");

    let output = fixture.run([
        "commit",
        "12345678-1234-1234-1234-123456789abc/commit-filter",
        "--",
        "included.txt",
    ]);

    assert!(output.status.success(), "commit failed: {}", all(&output));
    assert!(
        git_revision(&worktree, &["show", "--format=", "--name-only", "HEAD"])
            .contains("included.txt")
    );
    assert!(
        git_revision(&worktree, &["status", "--porcelain"]).contains("excluded.txt"),
        "unselected path must remain uncommitted"
    );
}

#[path = "maintenance/rebasing.rs"]
mod rebasing;
#[allow(unused_imports)]
pub use rebasing::*;

#[path = "maintenance/merging.rs"]
mod merging;
#[allow(unused_imports)]
pub use merging::*;

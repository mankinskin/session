use std::{
    ffi::OsStr,
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Output,
    },
};
use tempfile::TempDir;

const SESSION_UUID: &str = "12345678-1234-1234-1234-123456789abc";

struct Fixture {
    _temp: TempDir,
    main: PathBuf,
    tool: PathBuf,
}
impl Fixture {
    fn worktree(
        &self,
        slug: &str,
    ) -> PathBuf {
        self.main.join(".worktrees").join(SESSION_UUID).join(slug)
    }
    fn name(
        &self,
        slug: &str,
    ) -> String {
        format!("{SESSION_UUID}/{slug}")
    }
    fn run<I, S>(
        &self,
        args: I,
    ) -> Output
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
fn fixture_repo(tool: &Path) -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("submodule-source");
    let main = temp.path().join("main");
    init_repo(&source);
    fs::write(source.join("file.txt"), "initial\n").unwrap();
    git(&source, &["add", "file.txt"]);
    git(&source, &["commit", "-m", "initial"]);
    init_repo(&main);
    fs::write(main.join("README"), "fixture\n").unwrap();
    git(&main, &["add", "README"]);
    git(&main, &["commit", "-m", "initial"]);
    git(
        &main,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.to_str().unwrap(),
            "modules/example",
        ],
    );
    git(&main, &["commit", "-m", "add submodule"]);
    Fixture {
        _temp: temp,
        main,
        tool: tool.to_path_buf(),
    }
}
fn init_repo(path: &Path) {
    git_in(
        path.parent().unwrap(),
        &["init", "--initial-branch=main", path.to_str().unwrap()],
    );
    git(path, &["config", "user.email", "test@example.invalid"]);
    git(path, &["config", "user.name", "test"]);
}
fn git(
    repo: &Path,
    args: &[&str],
) {
    git_in(repo, args)
}
fn git_in(
    directory: &Path,
    args: &[&str],
) {
    let output = Command::new("git")
        .args(args)
        .current_dir(directory)
        .output()
        .unwrap();
    ok(&output, "git command");
}
fn git_out(
    repo: &Path,
    args: &[&str],
) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    ok(&output, "git command");
    out(&output).trim().to_owned()
}
fn recorded_sha(
    repo: &Path,
    path: &str,
) -> String {
    git_out(repo, &["rev-parse", &format!("HEAD:{path}")])
}
fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn all(output: &Output) -> String {
    format!("{}{}", out(output), String::from_utf8_lossy(&output.stderr))
}
fn ok(
    output: &Output,
    label: &str,
) {
    assert!(output.status.success(), "{label} failed: {}", all(output));
}
fn fails_with(
    output: &Output,
    expected: &str,
) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded: {}",
        all(output)
    );
    assert!(
        all(output).contains(expected),
        "expected {expected:?}: {}",
        all(output)
    );
}
fn tool() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_worktree-ctl"))
}
fn create(
    fixture: &Fixture,
    slug: &str,
) {
    ok(&fixture.run(["new", SESSION_UUID, slug]), "new worktree")
}
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(4)
        .expect("workspace root")
}

#[test]
fn guidance_documents_nested_legacy_and_worktree_local_active_session_marker() {
    for (path, markers) in [
        (
            ".agents/instructions/session/worktree-provisioning.instructions.md",
            [
                ".worktrees/<full-session-uuid>/<slug>",
                "Existing flat `.worktrees/<short-id>-<slug>` worktrees remain supported",
                "worktree-local `.session/sessions/<uuid>/session.json`",
            ],
        ),
        (
            ".agents/instructions/session/session-identity-and-handoff.instructions.md",
            [
                ".worktrees/<session-uuid>/<slug>",
                "Existing flat `.worktrees/<session-short-id>-<slug>` worktrees remain supported",
                ".session/local/active_workspace_session.json",
            ],
        ),
        (
            ".agents/instructions/commit/branch-worktree.instructions.md",
            [
                "<full-session-uuid>/session",
                "Existing flat `.worktrees/<short-id>-<slug>` worktrees remain supported",
                ".session/local/active_workspace_session.json",
            ],
        ),
    ] {
        let content = fs::read_to_string(workspace_root().join(path))
            .unwrap_or_else(|error| panic!("read {path}: {error}"));
        for marker in markers {
            assert!(content.contains(marker), "{path} must document {marker}");
        }
    }
}

#[test]
fn all_lifecycle_ops_support_dry_run() {
    let f = fixture_repo(&tool());
    let before = git_out(&f.main, &["rev-parse", "main"]);
    ok(
        &f.run(["new", SESSION_UUID, "run", "--dry-run"]),
        "dry-run new",
    );
    assert!(!f.worktree("run").exists());
    create(&f, "run");
    ok(
        &f.run(["rebase", &f.name("run"), "--dry-run"]),
        "dry-run rebase",
    );
    ok(
        &f.run(["rename", &f.name("run"), &f.name("renamed"), "--dry-run"]),
        "dry-run rename",
    );
    assert!(f.worktree("run").is_dir());
    assert!(!f.worktree("renamed").exists());
    ok(
        &f.run(["remove", &f.name("run"), "--dry-run"]),
        "dry-run remove",
    );
    assert!(f.worktree("run").is_dir());
    ok(
        &f.run(["finish", &f.name("run"), "--dry-run"]),
        "dry-run finish",
    );
    assert!(f.worktree("run").is_dir());
    assert_eq!(before, git_out(&f.main, &["rev-parse", "main"]));
}
#[test]
fn bootstrap_populates_submodule_offline() {
    let f = fixture_repo(&tool());
    let sha = recorded_sha(&f.main, "modules/example");
    create(&f, "bootstrap");
    let sub = f.worktree("bootstrap").join("modules/example");
    assert!(sub.join("file.txt").is_file());
    git(&sub, &["cat-file", "-e", &sha]);
}
#[test]
fn bootstrap_resolves_main_only_commit() {
    let f = fixture_repo(&tool());
    let main_sub = f.main.join("modules/example");
    fs::write(main_sub.join("file.txt"), "initial\nmain-only\n").unwrap();
    git(&main_sub, &["commit", "-am", "main-only commit"]);
    git(&f.main, &["add", "modules/example"]);
    git(
        &f.main,
        &["commit", "-m", "record main-only submodule commit"],
    );
    let sha = recorded_sha(&f.main, "modules/example");
    create(&f, "object");
    let sub = f.worktree("object").join("modules/example");
    git(&sub, &["cat-file", "-e", &sha]);
    assert_eq!(sha, git_out(&sub, &["rev-parse", "HEAD"]));
}
#[test]
fn create_preserves_dirty_main_checkout() {
    let f = fixture_repo(&tool());
    fs::write(f.main.join("README"), "fixture\ndirty main change\n").unwrap();
    fails_with(&f.run(["new", SESSION_UUID, "dirty"]), "README");
    assert!(!f.worktree("dirty").exists());
    let result =
        f.run(["new", SESSION_UUID, "dirty", "--preserve-main-changes"]);
    ok(&result, "preserved new");
    assert!(all(&result).contains("README"));
    assert!(
        git_out(&f.main, &["stash", "list"]).contains("preserve-main-changes")
    );
}
#[test]
fn create_requires_acknowledgement_when_dirty() {
    let f = fixture_repo(&tool());
    fs::write(f.main.join("README"), "fixture\nunacknowledged change\n")
        .unwrap();
    fails_with(
        &f.run(["new", SESSION_UUID, "dirty"]),
        "uncommitted changes",
    );
    assert!(
        fs::read_to_string(f.main.join("README"))
            .unwrap()
            .contains("unacknowledged change")
    );
    assert!(!f.worktree("dirty").exists());
}
#[test]
fn dry_run_plan_has_no_origin() {
    let f = fixture_repo(&tool());
    let result = f.run(["new", SESSION_UUID, "plan", "--dry-run"]);
    ok(&result, "dry-run plan");
    let plan = all(&result);
    assert!(!plan.contains("fetch origin"));
    assert!(!plan.contains("origin/main"));
}
#[test]
fn finish_rebases_marks_ready_and_removes() {
    let f = fixture_repo(&tool());
    create(&f, "ready");
    let worktree = f.worktree("ready");
    fs::write(worktree.join("completed.txt"), "completed\n").unwrap();
    git(&worktree, &["add", "completed.txt"]);
    git(&worktree, &["commit", "-m", "completed"]);
    let before = git_out(&f.main, &["rev-parse", "main"]);
    let result = f.run(["finish", &f.name("ready")]);
    ok(&result, "finish");
    assert_eq!(before, git_out(&f.main, &["rev-parse", "main"]));
    assert!(!worktree.exists());
    assert!(
        git_out(
            &f.main,
            &[
                "branch",
                "--list",
                "agent/12345678-1234-1234-1234-123456789abc/ready"
            ]
        )
        .contains("agent/12345678-1234-1234-1234-123456789abc/ready")
    );
    assert!(all(&result).contains("ready-to-merge"));
}
#[test]
fn no_origin_references() {
    let f = fixture_repo(&tool());
    let result = f.run(["new", SESSION_UUID, "behavior"]);
    ok(&result, "new without origin");
    let output = all(&result);
    assert!(!output.contains("fetch origin"));
    assert!(!output.contains("origin/main"));
}
#[test]
fn no_submodule_deinit() {
    let f = fixture_repo(&tool());
    create(&f, "guard");
    let worktree = f.worktree("guard");
    fs::write(worktree.join("dirty.txt"), "preserve main module\n").unwrap();
    ok(
        &f.run(["remove", &f.name("guard"), "--force"]),
        "forced remove",
    );
    assert!(f.main.join("modules/example/file.txt").is_file());
}
#[test]
fn no_worktree_move() {
    let f = fixture_repo(&tool());
    create(&f, "source");
    ok(
        &f.run(["rename", &f.name("source"), &f.name("target")]),
        "rename with submodule",
    );
    assert!(!f.worktree("source").exists());
    assert!(
        f.worktree("target")
            .join("modules/example/file.txt")
            .is_file()
    );
}
#[test]
fn remove_refuses_dirty_worktree() {
    let f = fixture_repo(&tool());
    create(&f, "dirty");
    let worktree = f.worktree("dirty");
    fs::write(worktree.join("dirty.txt"), "do not lose me\n").unwrap();
    fails_with(&f.run(["remove", &f.name("dirty")]), "dirty.txt");
    assert!(worktree.join("dirty.txt").is_file());
    ok(
        &f.run(["remove", &f.name("dirty"), "--force"]),
        "forced remove",
    );
    assert!(!worktree.exists());
}
#[test]
fn rename_is_remove_and_recreate() {
    let f = fixture_repo(&tool());
    create(&f, "source");
    ok(
        &f.run(["rename", &f.name("source"), &f.name("target")]),
        "rename",
    );
    let worktree = f.worktree("target");
    let sha = recorded_sha(&worktree, "modules/example");
    assert!(!f.worktree("source").exists());
    git(&worktree.join("modules/example"), &["cat-file", "-e", &sha]);
}
#[test]
fn rename_preserves_commit_ahead_of_gitlink() {
    let f = fixture_repo(&tool());
    create(&f, "source");
    let sub = f.worktree("source").join("modules/example");
    fs::write(sub.join("file.txt"), "initial\nahead\n").unwrap();
    git(&sub, &["commit", "-am", "ahead"]);
    let sha = git_out(&sub, &["rev-parse", "HEAD"]);
    ok(
        &f.run(["rename", &f.name("source"), &f.name("target")]),
        "rename",
    );
    git(
        &f.worktree("target").join("modules/example"),
        &["cat-file", "-e", &sha],
    );
}
#[test]
fn second_worktree_is_rejected_for_one_session() {
    let f = fixture_repo(&tool());
    create(&f, "first");
    fails_with(
        &f.run(["new", SESSION_UUID, "second"]),
        "ambiguous session worktree",
    );
    assert!(f.worktree("first").is_dir());
    assert!(!f.worktree("second").exists());
}
#[test]
fn session_reuses_existing_worktree() {
    let f = fixture_repo(&tool());
    let first = f.run(["new", SESSION_UUID, "session"]);
    ok(&first, "first new");
    let second = f.run(["new", SESSION_UUID, "session"]);
    ok(&second, "second new");
    let first_output = out(&first);
    let second_output = out(&second);
    let one = first_output
        .lines()
        .find_map(|line| line.strip_prefix("WORKTREE_PATH="))
        .expect("first worktree path");
    let two = second_output
        .lines()
        .find_map(|line| line.strip_prefix("WORKTREE_PATH="))
        .expect("second worktree path");
    assert_eq!(one, two);
    let count = fs::read_dir(f.main.join(".worktrees").join(SESSION_UUID))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count();
    assert_eq!(count, 1);
}

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

    fn legacy_worktree(
        &self,
        name: &str,
    ) -> PathBuf {
        self.main.join(".worktrees").join(name)
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

fn git(
    repository: &Path,
    arguments: &[&str],
) {
    git_in(repository, arguments);
}

fn git_in(
    directory: &Path,
    arguments: &[&str],
) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("git starts");
    assert!(output.status.success(), "git failed: {}", all(&output));
}

fn git_revision(
    repository: &Path,
    arguments: &[&str],
) -> String {
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

fn create(
    fixture: &Fixture,
    slug: &str,
) {
    let output = fixture.run(["new", SESSION_UUID, slug]);
    assert!(output.status.success(), "new failed: {}", all(&output));
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

    let output = fixture.run(["list", "--dry-run"]);

    assert!(output.status.success(), "list failed: {}", all(&output));
    let report = all(&output);
    assert!(
        report.contains(
            "branch=agent/12345678-1234-1234-1234-123456789abc/state"
        ),
        "{report}"
    );
    assert!(report.contains("branch=agent/legacy-state"), "{report}");
    assert!(report.contains("submodules=initialized"), "{report}");
    assert!(
        report.contains("lifecycle=preserved reason=not-idle"),
        "{report}"
    );
}

#[test]
fn merge_refuses_non_fast_forward() {
    let fixture = fixture_repo();
    create(&fixture, "non-ff");
    let worktree = fixture.worktree("non-ff");
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "main advanced"]);

    let output =
        fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/non-ff"]);

    assert!(
        !output.status.success(),
        "merge unexpectedly succeeded: {}",
        all(&output)
    );
    assert!(
        all(&output).contains("merge --ff-only failed"),
        "{}",
        all(&output)
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
fn merge_accepts_bottom_up_gitlink_integration() {
    let fixture = fixture_repo();
    create(&fixture, "bottom-up");
    let worktree = fixture.worktree("bottom-up");
    let nested = worktree.join("modules/example");
    git(
        &nested,
        &[
            "checkout",
            "-b",
            "agent/12345678-1234-1234-1234-123456789abc/bottom-up",
        ],
    );
    fs::write(nested.join("file.txt"), "initial\nbottom-up\n")
        .expect("write nested change");
    git(&nested, &["commit", "-am", "nested feature"]);
    git(
        &fixture.main.join("modules/example"),
        &[
            "merge",
            "--ff-only",
            "agent/12345678-1234-1234-1234-123456789abc/bottom-up",
        ],
    );
    git(&worktree, &["add", "modules/example"]);
    git(&worktree, &["commit", "-m", "bump nested gitlink"]);

    let output = fixture
        .run(["merge", "12345678-1234-1234-1234-123456789abc/bottom-up"]);

    assert!(output.status.success(), "merge failed: {}", all(&output));
    assert_eq!(
        git_revision(&fixture.main, &["rev-parse", "HEAD:modules/example"]),
        git_revision(
            &fixture.main.join("modules/example"),
            &["rev-parse", "main"]
        )
    );
}

#[test]
fn merge_rejects_orphan_gitlink_before_mutation() {
    let fixture = fixture_repo();
    create(&fixture, "orphan");
    let worktree = fixture.worktree("orphan");
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);
    let submodule = fixture.main.join("modules/example");
    git(&submodule, &["checkout", "--orphan", "replacement"]);
    git(&submodule, &["rm", "-rf", "."]);
    fs::write(submodule.join("replacement.txt"), "replacement\n")
        .expect("write replacement");
    git(&submodule, &["add", "replacement.txt"]);
    git(&submodule, &["commit", "-m", "replacement"]);
    git(&submodule, &["branch", "-f", "main", "replacement"]);
    git(&submodule, &["checkout", "main"]);
    git(&submodule, &["branch", "-D", "replacement"]);
    let before = git_revision(&fixture.main, &["rev-parse", "main"]);

    let output =
        fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/orphan"]);

    assert!(
        !output.status.success(),
        "merge unexpectedly succeeded: {}",
        all(&output)
    );
    assert!(all(&output).contains("Orphan"), "{}", all(&output));
    assert_eq!(before, git_revision(&fixture.main, &["rev-parse", "main"]));
}

#[test]
fn merge_auto_fixes_fast_forwardable_orphan_gitlink() {
    let fixture = fixture_repo();
    let submodule = fixture.main.join("modules/example");
    let submodule_main_before =
        git_revision(&submodule, &["rev-parse", "main"]);
    git(&submodule, &["checkout", "--detach", "main"]);
    fs::write(submodule.join("ahead.txt"), "ahead\n").expect("write ahead");
    git(&submodule, &["add", "ahead.txt"]);
    git(
        &submodule,
        &["commit", "-m", "ahead of submodule main, no branch"],
    );
    let recorded_sha = git_revision(&submodule, &["rev-parse", "HEAD"]);
    git(&fixture.main, &["add", "modules/example"]);
    git(
        &fixture.main,
        &["commit", "-m", "bump gitlink ahead of main"],
    );
    assert_ne!(submodule_main_before, recorded_sha);

    create(&fixture, "autofix");
    let worktree = fixture.worktree("autofix");
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);

    let dry_run = fixture.run([
        "merge",
        "12345678-1234-1234-1234-123456789abc/autofix",
        "--dry-run",
    ]);
    assert!(
        dry_run.status.success(),
        "dry-run failed: {}",
        all(&dry_run)
    );
    assert!(
        all(&dry_run).contains("auto-fix gitlink"),
        "{}",
        all(&dry_run)
    );
    assert_eq!(
        submodule_main_before,
        git_revision(&submodule, &["rev-parse", "main"]),
        "dry-run must not mutate the submodule branch"
    );

    let output =
        fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/autofix"]);

    assert!(output.status.success(), "merge failed: {}", all(&output));
    assert!(
        all(&output).contains("auto-fixed gitlink"),
        "{}",
        all(&output)
    );
    assert_eq!(
        recorded_sha,
        git_revision(&submodule, &["rev-parse", "main"]),
        "submodule main must fast-forward to the recorded gitlink commit"
    );
}

#[test]
fn rebase_auto_commits_dirty_worktree_when_requested() {
    let fixture = fixture_repo();
    create(&fixture, "autocommit");
    let worktree = fixture.worktree("autocommit");
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "advance main"]);
    fs::write(worktree.join("dirty.txt"), "dirty\n").expect("write dirty");

    let output = fixture.run([
        "rebase",
        "12345678-1234-1234-1234-123456789abc/autocommit",
        "--auto-commit",
    ]);

    assert!(output.status.success(), "rebase failed: {}", all(&output));
    assert!(
        all(&output).contains("auto-committed uncommitted changes"),
        "{}",
        all(&output)
    );
    let log = git_revision(&worktree, &["log", "--oneline", "-1"]);
    assert!(log.contains("worktree-ctl auto-commit"), "{log}");
    let status = git_revision(&worktree, &["status", "--porcelain"]);
    assert!(status.is_empty(), "worktree should be clean: {status}");
}

#[test]
fn rebase_stashes_and_restores_dirty_worktree_by_default() {
    let fixture = fixture_repo();
    create(&fixture, "stash");
    let worktree = fixture.worktree("stash");
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "advance main"]);
    fs::write(worktree.join("dirty.txt"), "dirty\n").expect("write dirty");

    let output =
        fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/stash"]);

    assert!(output.status.success(), "rebase failed: {}", all(&output));
    assert!(
        all(&output).contains("stashed uncommitted changes"),
        "{}",
        all(&output)
    );
    let status = git_revision(&worktree, &["status", "--porcelain"]);
    assert!(
        status.contains("dirty.txt"),
        "expected dirty.txt restored as an uncommitted change: {status}"
    );
    let log = git_revision(&worktree, &["log", "--oneline", "-1"]);
    assert!(
        !log.contains("auto-commit"),
        "default mode must not commit anything: {log}"
    );
}

#[test]
fn merge_unblocks_ff_only_merge_by_stashing_untracked_files() {
    let fixture = fixture_repo();
    create(&fixture, "untracked");
    let worktree = fixture.worktree("untracked");
    fs::write(worktree.join("colliding.txt"), "from-branch\n")
        .expect("write branch file");
    git(&worktree, &["add", "colliding.txt"]);
    git(&worktree, &["commit", "-m", "add colliding file"]);
    fs::write(fixture.main.join("colliding.txt"), "leftover\n")
        .expect("write untracked collider");

    let output = fixture
        .run(["merge", "12345678-1234-1234-1234-123456789abc/untracked"]);

    // The stash unblocks the ff-only merge itself: before this change, git
    // refused the merge outright with "untracked working tree files would be
    // overwritten", never reaching the fast-forward at all.
    assert!(
        all(&output).contains("stashed uncommitted changes"),
        "{}",
        all(&output)
    );
    assert_eq!(
        git_revision(&fixture.main, &["rev-parse", "main"]),
        git_revision(&worktree, &["rev-parse", "HEAD"]),
        "superproject main must fast-forward despite the leftover untracked file"
    );
    // The leftover content genuinely conflicts with what the branch added at
    // the same path, so restoring the stash is refused rather than silently
    // discarded; the tool surfaces the leftover stash instead of losing it.
    assert!(
        !output.status.success(),
        "merge should report the unresolved stash: {}",
        all(&output)
    );
    assert!(all(&output).contains("stash list"), "{}", all(&output));
    assert!(
        git_revision(&fixture.main, &["stash", "list"])
            .contains("worktree-ctl autostash"),
        "leftover untracked content must remain recoverable in the stash"
    );
}

#[test]
fn merge_rejects_unresolvable_gitlink_before_mutation() {
    let fixture = fixture_repo();
    create(&fixture, "unresolvable");
    let worktree = fixture.worktree("unresolvable");
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);
    let missing_sha = "0123456789abcdef0123456789abcdef01234567";
    git(
        &fixture.main,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{missing_sha},modules/example"),
        ],
    );
    git(&fixture.main, &["commit", "-m", "record missing gitlink"]);
    let before = git_revision(&fixture.main, &["rev-parse", "main"]);

    let output = fixture
        .run(["merge", "12345678-1234-1234-1234-123456789abc/unresolvable"]);

    assert!(
        !output.status.success(),
        "merge unexpectedly succeeded: {}",
        all(&output)
    );
    let report = all(&output);
    assert!(report.contains("modules/example"), "{report}");
    assert!(report.contains(missing_sha), "{report}");
    assert!(report.contains("object database"), "{report}");
    assert_eq!(before, git_revision(&fixture.main, &["rev-parse", "main"]));
}

#[test]
fn merge_allows_backward_gitlink_and_dry_run_mutates_nothing() {
    let fixture = fixture_repo();
    create(&fixture, "behind");
    let worktree = fixture.worktree("behind");
    let submodule = fixture.main.join("modules/example");
    fs::write(submodule.join("file.txt"), "initial\nmain ahead\n")
        .expect("write main change");
    git(&submodule, &["commit", "-am", "main ahead"]);
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);
    let before = git_revision(&fixture.main, &["rev-parse", "main"]);

    let dry_run = fixture.run([
        "merge",
        "12345678-1234-1234-1234-123456789abc/behind",
        "--dry-run",
    ]);
    assert!(
        dry_run.status.success(),
        "dry-run failed: {}",
        all(&dry_run)
    );
    assert!(
        all(&dry_run).contains("skip modules/example"),
        "{}",
        all(&dry_run)
    );
    assert_eq!(before, git_revision(&fixture.main, &["rev-parse", "main"]));
    let merge =
        fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/behind"]);
    assert!(merge.status.success(), "merge failed: {}", all(&merge));
}

#[test]
fn rebase_rebases_submodule_before_superproject() {
    let fixture = fixture_repo();
    create(&fixture, "ordered");
    let worktree = fixture.worktree("ordered");
    let submodule = worktree.join("modules/example");
    let branch = "agent/12345678-1234-1234-1234-123456789abc/ordered";
    git(&submodule, &["checkout", "-b", branch]);
    fs::write(submodule.join("feature.txt"), "feature\n")
        .expect("write nested feature");
    git(&submodule, &["add", "feature.txt"]);
    git(&submodule, &["commit", "-m", "nested feature"]);
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write superproject feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "superproject feature"]);
    let main_submodule = fixture.main.join("modules/example");
    fs::write(main_submodule.join("file.txt"), "initial\nmain\n")
        .expect("write nested main");
    git(&main_submodule, &["commit", "-am", "nested main"]);
    fs::write(fixture.main.join("main.txt"), "main\n")
        .expect("write superproject main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "superproject main"]);

    let dry_run = fixture.run([
        "rebase",
        "12345678-1234-1234-1234-123456789abc/ordered",
        "--dry-run",
    ]);
    assert!(
        dry_run.status.success(),
        "dry-run failed: {}",
        all(&dry_run)
    );
    let plan = all(&dry_run);
    assert!(
        plan.find(
            "checkout agent/12345678-1234-1234-1234-123456789abc/ordered"
        ) < plan.find("rebase ").filter(|_| plan.contains("/ordered")),
        "{plan}"
    );

    let output =
        fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/ordered"]);
    assert!(output.status.success(), "rebase failed: {}", all(&output));
    git(&submodule, &["merge-base", "--is-ancestor", "main", branch]);
    assert_eq!(
        git_revision(&worktree, &["rev-parse", "HEAD:modules/example"]),
        git_revision(&submodule, &["rev-parse", branch])
    );
}

#[test]
fn rebase_reports_missing_submodule_branch_as_skipped() {
    let fixture = fixture_repo();
    create(&fixture, "skipped");

    let output =
        fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/skipped"]);

    assert!(output.status.success(), "rebase failed: {}", all(&output));
    assert!(
        all(&output).contains("skip modules/example because branch agent/12345678-1234-1234-1234-123456789abc/skipped does not exist"),
        "{}",
        all(&output)
    );
}

#[test]
fn rebase_conflict_stops_before_superproject_rebase() {
    let fixture = fixture_repo();
    create(&fixture, "conflict");
    let worktree = fixture.worktree("conflict");
    let submodule = worktree.join("modules/example");
    git(
        &submodule,
        &[
            "checkout",
            "-b",
            "agent/12345678-1234-1234-1234-123456789abc/conflict",
        ],
    );
    fs::write(submodule.join("file.txt"), "agent\n")
        .expect("write nested feature");
    git(&submodule, &["commit", "-am", "nested feature"]);
    fs::write(worktree.join("feature.txt"), "feature\n")
        .expect("write superproject feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "superproject feature"]);
    let main_submodule = fixture.main.join("modules/example");
    fs::write(main_submodule.join("file.txt"), "main\n")
        .expect("write nested main");
    git(&main_submodule, &["commit", "-am", "nested main"]);
    fs::write(fixture.main.join("main.txt"), "main\n")
        .expect("write superproject main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "superproject main"]);
    let before = git_revision(&worktree, &["rev-parse", "HEAD"]);

    let output = fixture
        .run(["rebase", "12345678-1234-1234-1234-123456789abc/conflict"]);

    assert!(
        !output.status.success(),
        "rebase unexpectedly succeeded: {}",
        all(&output)
    );
    assert!(
        all(&output).contains("submodule modules/example branch agent/12345678-1234-1234-1234-123456789abc/conflict"),
        "{}",
        all(&output)
    );
    assert_eq!(before, git_revision(&worktree, &["rev-parse", "HEAD"]));
}

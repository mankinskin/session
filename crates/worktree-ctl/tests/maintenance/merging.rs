#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn merge_refuses_non_fast_forward() {
    let fixture = fixture_repo();
    create(&fixture, "non-ff");
    let worktree = fixture.worktree("non-ff");
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "main advanced"]);

    let output = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/non-ff"]);

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
pub(super) fn merge_accepts_bottom_up_gitlink_integration() {
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
    fs::write(nested.join("file.txt"), "initial\nbottom-up\n").expect("write nested change");
    git(&nested, &["commit", "-am", "nested feature"]);
    git(&worktree, &["add", "modules/example"]);
    git(&worktree, &["commit", "-m", "bump nested gitlink"]);

    let output = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/bottom-up"]);

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
pub(super) fn merge_rejects_orphan_gitlink_before_mutation() {
    let fixture = fixture_repo();
    create(&fixture, "orphan");
    let worktree = fixture.worktree("orphan");
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "feature"]);
    let submodule = fixture.main.join("modules/example");
    git(&submodule, &["checkout", "--orphan", "replacement"]);
    git(&submodule, &["rm", "-rf", "."]);
    fs::write(submodule.join("replacement.txt"), "replacement\n").expect("write replacement");
    git(&submodule, &["add", "replacement.txt"]);
    git(&submodule, &["commit", "-m", "replacement"]);
    git(&submodule, &["branch", "-f", "main", "replacement"]);
    git(&submodule, &["checkout", "main"]);
    git(&submodule, &["branch", "-D", "replacement"]);
    let before = git_revision(&fixture.main, &["rev-parse", "main"]);

    let output = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/orphan"]);

    assert!(
        !output.status.success(),
        "merge unexpectedly succeeded: {}",
        all(&output)
    );
    assert!(all(&output).contains("Orphan"), "{}", all(&output));
    assert_eq!(before, git_revision(&fixture.main, &["rev-parse", "main"]));
}

#[test]
pub(super) fn merge_auto_fixes_fast_forwardable_orphan_gitlink() {
    let fixture = fixture_repo();
    let submodule = fixture.main.join("modules/example");
    let submodule_main_before = git_revision(&submodule, &["rev-parse", "main"]);
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
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write feature");
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

    let output = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/autofix"]);

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
pub(super) fn merge_unblocks_ff_only_merge_by_stashing_untracked_files() {
    let fixture = fixture_repo();
    create(&fixture, "untracked");
    let worktree = fixture.worktree("untracked");
    fs::write(worktree.join("colliding.txt"), "from-branch\n").expect("write branch file");
    git(&worktree, &["add", "colliding.txt"]);
    git(&worktree, &["commit", "-m", "add colliding file"]);
    fs::write(fixture.main.join("colliding.txt"), "leftover\n").expect("write untracked collider");

    let output = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/untracked"]);

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
        git_revision(&fixture.main, &["stash", "list"]).contains("worktree-ctl autostash"),
        "leftover untracked content must remain recoverable in the stash"
    );
}

#[test]
pub(super) fn merge_rejects_unresolvable_gitlink_before_mutation() {
    let fixture = fixture_repo();
    create(&fixture, "unresolvable");
    let worktree = fixture.worktree("unresolvable");
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write feature");
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

    let output = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/unresolvable"]);

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
pub(super) fn merge_allows_backward_gitlink_and_dry_run_mutates_nothing() {
    let fixture = fixture_repo();
    create(&fixture, "behind");
    let worktree = fixture.worktree("behind");
    let submodule = fixture.main.join("modules/example");
    fs::write(submodule.join("file.txt"), "initial\nmain ahead\n").expect("write main change");
    git(&submodule, &["commit", "-am", "main ahead"]);
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write feature");
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
    let merge = fixture.run(["merge", "12345678-1234-1234-1234-123456789abc/behind"]);
    assert!(merge.status.success(), "merge failed: {}", all(&merge));
}

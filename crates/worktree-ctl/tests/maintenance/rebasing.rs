#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn rebase_auto_commits_dirty_worktree_when_requested() {
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
pub(super) fn rebase_stashes_and_restores_dirty_worktree_by_default() {
    let fixture = fixture_repo();
    create(&fixture, "stash");
    let worktree = fixture.worktree("stash");
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "advance main"]);
    fs::write(worktree.join("dirty.txt"), "dirty\n").expect("write dirty");

    let output = fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/stash"]);

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
pub(super) fn rebase_rebases_submodule_before_superproject() {
    let fixture = fixture_repo();
    create(&fixture, "ordered");
    let worktree = fixture.worktree("ordered");
    let submodule = worktree.join("modules/example");
    let branch = "agent/12345678-1234-1234-1234-123456789abc/ordered";
    git(&submodule, &["checkout", "-b", branch]);
    fs::write(submodule.join("feature.txt"), "feature\n").expect("write nested feature");
    git(&submodule, &["add", "feature.txt"]);
    git(&submodule, &["commit", "-m", "nested feature"]);
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write superproject feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "superproject feature"]);
    let main_submodule = fixture.main.join("modules/example");
    fs::write(main_submodule.join("file.txt"), "initial\nmain\n").expect("write nested main");
    git(&main_submodule, &["commit", "-am", "nested main"]);
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write superproject main");
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
        plan.find("checkout agent/12345678-1234-1234-1234-123456789abc/ordered")
            < plan.find("rebase ").filter(|_| plan.contains("/ordered")),
        "{plan}"
    );

    let output = fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/ordered"]);
    assert!(output.status.success(), "rebase failed: {}", all(&output));
    git(&submodule, &["merge-base", "--is-ancestor", "main", branch]);
    assert_eq!(
        git_revision(&worktree, &["rev-parse", "HEAD:modules/example"]),
        git_revision(&submodule, &["rev-parse", branch])
    );
}

#[test]
pub(super) fn rebase_amends_the_current_generated_gitlink_checkpoint() {
    let fixture = fixture_repo();
    create(&fixture, "checkpoint");
    let worktree = fixture.worktree("checkpoint");
    let submodule = worktree.join("modules/example");
    let branch = "agent/12345678-1234-1234-1234-123456789abc/checkpoint";
    git(&submodule, &["checkout", "-b", branch]);
    fs::write(submodule.join("feature.txt"), "feature\n").expect("write nested feature");
    git(&submodule, &["add", "feature.txt"]);
    git(&submodule, &["commit", "-m", "nested feature"]);
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write superproject feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "superproject feature"]);

    let main_submodule = fixture.main.join("modules/example");
    fs::write(main_submodule.join("file.txt"), "initial\nmain one\n")
        .expect("write first main change");
    git(&main_submodule, &["commit", "-am", "main one"]);
    fs::write(fixture.main.join("main-one.txt"), "main\n")
        .expect("write first superproject main change");
    git(&fixture.main, &["add", "main-one.txt"]);
    git(&fixture.main, &["commit", "-m", "superproject main one"]);
    let first = fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/checkpoint"]);
    assert!(first.status.success(), "rebase failed: {}", all(&first));

    fs::write(
        main_submodule.join("file.txt"),
        "initial\nmain one\nmain two\n",
    )
    .expect("write second main change");
    git(&main_submodule, &["commit", "-am", "main two"]);
    fs::write(fixture.main.join("main-two.txt"), "main\n")
        .expect("write second superproject main change");
    git(&fixture.main, &["add", "main-two.txt"]);
    git(&fixture.main, &["commit", "-m", "superproject main two"]);
    let second = fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/checkpoint"]);
    assert!(second.status.success(), "rebase failed: {}", all(&second));

    assert_eq!(
        "1",
        git_revision(
            &worktree,
            &[
                "rev-list",
                "--count",
                "--grep=^rebase submodule bases onto local main",
                "HEAD"
            ],
        )
    );
    assert_eq!(
        "1",
        git_revision(
            &worktree,
            &[
                "rev-list",
                "--count",
                "--grep=^rebase submodule tips onto local main",
                "HEAD"
            ],
        )
    );
    assert_eq!(
        git_revision(&submodule, &["rev-parse", branch]),
        git_revision(&worktree, &["rev-parse", "HEAD:modules/example"]),
    );
}

#[test]
pub(super) fn rebase_reports_missing_submodule_branch_as_skipped() {
    let fixture = fixture_repo();
    create(&fixture, "skipped");

    let output = fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/skipped"]);

    assert!(output.status.success(), "rebase failed: {}", all(&output));
    assert!(
        all(&output).contains("skip modules/example because branch agent/12345678-1234-1234-1234-123456789abc/skipped does not exist"),
        "{}",
        all(&output)
    );
}

#[test]
pub(super) fn rebase_checkpoints_owned_session_artifacts_before_mutating() {
    let fixture = fixture_repo();
    create(&fixture, "session-checkpoint");
    let worktree = fixture.worktree("session-checkpoint");
    write_owned_session_record(&worktree, None);

    let output = fixture.run([
        "rebase",
        "12345678-1234-1234-1234-123456789abc/session-checkpoint",
    ]);

    assert!(output.status.success(), "rebase failed: {}", all(&output));
    assert!(
        all(&output).contains("checkpointed owned session"),
        "{output:?}"
    );
    assert_eq!(
        "",
        git_revision(&worktree, &["status", "--porcelain"]),
        "session-only changes should be committed before rebase"
    );
    assert!(
        git_revision(&worktree, &["show", "--format=", "--name-only", "HEAD"]).contains(
            ".workflow-tools/session/sessions/12345678-1234-1234-1234-123456789abc/session.json"
        )
    );
}

#[test]
pub(super) fn sync_checkpoints_main_session_mirror_before_stashing_other_changes() {
    let fixture = fixture_repo();
    create(&fixture, "session-mirror");
    let worktree = fixture.worktree("session-mirror");
    write_owned_session_record(&worktree, None);
    write_session_record(&fixture.main, &worktree, None);
    fs::write(fixture.main.join("unrelated.txt"), "preserve me\n")
        .expect("write unrelated main change");

    let output = fixture.run([
        "sync",
        "12345678-1234-1234-1234-123456789abc/session-mirror",
    ]);

    assert!(output.status.success(), "sync failed: {}", all(&output));
    assert!(
        all(&output).contains("checkpointed main session mirror"),
        "{}",
        all(&output)
    );
    assert!(
        fixture.main.join("unrelated.txt").is_file(),
        "unrelated main change must be restored"
    );
    assert_eq!(
        "",
        git_revision(&fixture.main, &["stash", "list"]),
        "session mirror must not collide with stash restoration"
    );
    assert!(
        git_revision(&fixture.main, &["log", "--format=%s", "-2"])
            .contains("worktree-ctl checkpoint session mirror")
    );
}

#[test]
pub(super) fn rebase_skips_redundant_descendant_gitlink_update() {
    let fixture = fixture_repo();
    create(&fixture, "redundant-gitlink");
    let worktree = fixture.worktree("redundant-gitlink");
    let feature_submodule = worktree.join("modules/example");
    let main_submodule = fixture.main.join("modules/example");

    fs::write(main_submodule.join("file.txt"), "initial\nintermediate\n")
        .expect("write intermediate submodule commit");
    git(&main_submodule, &["commit", "-am", "intermediate"]);
    let intermediate = git_revision(&main_submodule, &["rev-parse", "HEAD"]);

    git(&feature_submodule, &["checkout", &intermediate]);
    git(&worktree, &["add", "modules/example"]);
    git(&worktree, &["commit", "-m", "record intermediate gitlink"]);

    fs::write(
        main_submodule.join("file.txt"),
        "initial\nintermediate\nlatest\n",
    )
    .expect("write latest submodule commit");
    git(&main_submodule, &["commit", "-am", "latest"]);
    git(&fixture.main, &["add", "modules/example"]);
    git(&fixture.main, &["commit", "-m", "record latest gitlink"]);

    let output = fixture.run([
        "rebase",
        "12345678-1234-1234-1234-123456789abc/redundant-gitlink",
    ]);

    assert!(output.status.success(), "rebase failed: {}", all(&output));
    assert!(
        all(&output).contains("skipped redundant gitlink-only branch changes"),
        "{}",
        all(&output)
    );
    assert_eq!(
        git_revision(&fixture.main, &["rev-parse", "HEAD"]),
        git_revision(&worktree, &["rev-parse", "HEAD"]),
        "the redundant feature commit should be skipped"
    );
}

#[test]
pub(super) fn rebase_conflict_stops_before_superproject_rebase() {
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
    fs::write(submodule.join("file.txt"), "agent\n").expect("write nested feature");
    git(&submodule, &["commit", "-am", "nested feature"]);
    fs::write(worktree.join("feature.txt"), "feature\n").expect("write superproject feature");
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "superproject feature"]);
    let main_submodule = fixture.main.join("modules/example");
    fs::write(main_submodule.join("file.txt"), "main\n").expect("write nested main");
    git(&main_submodule, &["commit", "-am", "nested main"]);
    fs::write(fixture.main.join("main.txt"), "main\n").expect("write superproject main");
    git(&fixture.main, &["add", "main.txt"]);
    git(&fixture.main, &["commit", "-m", "superproject main"]);
    let before = git_revision(&worktree, &["rev-parse", "HEAD"]);

    let output = fixture.run(["rebase", "12345678-1234-1234-1234-123456789abc/conflict"]);

    assert!(
        !output.status.success(),
        "rebase unexpectedly succeeded: {}",
        all(&output)
    );
    assert!(
        all(&output).contains(
            "submodule modules/example branch agent/12345678-1234-1234-1234-123456789abc/conflict"
        ),
        "{}",
        all(&output)
    );
    assert_eq!(before, git_revision(&worktree, &["rev-parse", "HEAD"]));
}

#[test]
pub(super) fn sync_all_stops_after_the_oldest_worktree_conflicts() {
    let fixture = fixture_repo();
    let first = create_legacy_worktree(&fixture, "first");
    let second = create_legacy_worktree(&fixture, "second");
    fs::write(first.join("README"), "first\n").expect("write first branch change");
    git(&first, &["commit", "-am", "first branch change"]);
    fs::write(second.join("second.txt"), "second\n").expect("write second branch change");
    git(&second, &["add", "second.txt"]);
    git(&second, &["commit", "-m", "second branch change"]);
    fs::write(fixture.main.join("README"), "main\n").expect("write main conflict");
    git(&fixture.main, &["commit", "-am", "main conflict"]);

    let output = fixture.run(["sync", "--all"]);

    assert!(
        !output.status.success(),
        "sync unexpectedly succeeded: {}",
        all(&output)
    );
    assert!(
        all(&output).contains("sync first failed"),
        "oldest worktree should be attempted first: {}",
        all(&output)
    );
    assert!(
        !fixture.main.join("second.txt").exists(),
        "sync must stop before attempting the newer worktree"
    );
}

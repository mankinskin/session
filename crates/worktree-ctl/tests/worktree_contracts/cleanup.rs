#[allow(unused_imports)]
use super::*;

#[test]
pub(super) fn remove_refuses_dirty_worktree() {
    let f = fixture_repo(&tool());
    create(&f, "dirty");
    let worktree = f.worktree("dirty");
    fs::write(worktree.join("dirty.txt"), "do not lose me\n").unwrap();
    fails_with(
        &f.run(["remove", &f.name("dirty")]),
        "superproject has uncommitted changes",
    );
    assert!(worktree.join("dirty.txt").is_file());
    ok(
        &f.run(["remove", &f.name("dirty"), "--force"]),
        "forced remove",
    );
    assert!(!worktree.exists());
}

#[test]
pub(super) fn remove_refuses_superproject_ahead_of_main() {
    let fixture = fixture_repo(&tool());
    create(&fixture, "ahead");
    let worktree = fixture.worktree("ahead");
    fs::write(worktree.join("ahead.txt"), "ahead\n").unwrap();
    git(&worktree, &["add", "ahead.txt"]);
    git(&worktree, &["commit", "-m", "ahead"]);
    fails_with(
        &fixture.run(["remove", &fixture.name("ahead")]),
        "superproject is 1 commits ahead of main",
    );
    assert!(worktree.is_dir());
}

#[test]
pub(super) fn remove_refuses_dirty_submodule() {
    let fixture = fixture_repo(&tool());
    create(&fixture, "submodule-dirty");
    let worktree = fixture.worktree("submodule-dirty");
    fs::write(
        worktree.join("modules/example/file.txt"),
        "initial\ndirty\n",
    )
    .unwrap();
    fails_with(
        &fixture.run(["remove", &fixture.name("submodule-dirty")]),
        "submodule modules/example has uncommitted changes",
    );
    assert!(worktree.is_dir());
}

#[test]
pub(super) fn remove_refuses_submodule_ahead_of_main() {
    let fixture = fixture_repo(&tool());
    create(&fixture, "submodule-ahead");
    let worktree = fixture.worktree("submodule-ahead");
    let submodule = worktree.join("modules/example");
    fs::write(submodule.join("file.txt"), "initial\nahead\n").unwrap();
    git(&submodule, &["commit", "-am", "ahead"]);
    fails_with(
        &fixture.run(["remove", &fixture.name("submodule-ahead")]),
        "submodule modules/example is 1 commits ahead of main",
    );
    assert!(worktree.is_dir());
}

#[test]
pub(super) fn clean_removes_safe_worktree() {
    let fixture = fixture_repo(&tool());
    create(&fixture, "clean");
    let worktree = fixture.worktree("clean");
    ok(&fixture.run(["clean", "--all"]), "clean safe worktree");
    assert!(!worktree.exists());
}

#[test]
pub(super) fn clean_removes_empty_unregistered_worktree_debris() {
    let fixture = fixture_repo(&tool());
    let debris = fixture.main.join(".worktrees").join("stale-session");
    fs::create_dir_all(&debris).unwrap();

    ok(&fixture.run(["clean", "--all"]), "clean empty debris");

    assert!(!debris.exists());
}

#[test]
pub(super) fn clean_removes_selected_worktree_that_is_fully_behind_main() {
    let fixture = fixture_repo(&tool());
    create(&fixture, "behind");
    let worktree = fixture.worktree("behind");
    fs::write(fixture.main.join("main-only.txt"), "main\n").unwrap();
    git(&fixture.main, &["add", "main-only.txt"]);
    git(&fixture.main, &["commit", "-m", "advance main"]);

    ok(
        &fixture.run(["clean", &fixture.name("behind")]),
        "clean fully behind worktree",
    );
    assert!(!worktree.exists());
}

#[test]
pub(super) fn clean_preserves_dirty_worktree() {
    let fixture = fixture_repo(&tool());
    create(&fixture, "dirty");
    let worktree = fixture.worktree("dirty");
    fs::write(worktree.join("dirty.txt"), "preserve\n").unwrap();
    let output = fixture.run(["clean", "--all"]);
    ok(&output, "clean dirty worktree");
    assert!(all(&output).contains("superproject has uncommitted changes"));
    assert!(worktree.is_dir());
}

#[test]
pub(super) fn rename_is_remove_and_recreate() {
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
pub(super) fn rename_preserves_commit_ahead_of_gitlink() {
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

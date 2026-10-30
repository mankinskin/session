use std::{fs, path::Path, process::Command};

use git2::{IndexAddOption, Repository, Signature};
use tempfile::TempDir;

use super::WorktreeGit;

pub(crate) struct Fixture {
    temp: TempDir,
    pub(crate) main: std::path::PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let main = temp.path().join("main");
        let repository = Repository::init(&main).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        fs::write(main.join("tracked.txt"), "initial\n").unwrap();
        commit_all(&repository, "initial");
        Self { temp, main }
    }

    pub(crate) fn git(&self) -> WorktreeGit {
        WorktreeGit::open(&self.main).unwrap()
    }

    pub(crate) fn add_submodule(&self) -> String {
        self.add_submodule_named("nested")
    }

    fn add_submodule_named(&self, name: &str) -> String {
        let inner = self.temp.path().join("inner");
        let repository = Repository::open(&inner).unwrap_or_else(|_| {
            let repository = Repository::init(&inner).unwrap();
            fs::write(inner.join("inner.txt"), "inner\n").unwrap();
            commit_all(&repository, "inner");
            repository
        });
        let sha = repository.head().unwrap().target().unwrap().to_string();
        command(
            &self.main,
            [
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                inner.to_str().unwrap(),
                name,
            ],
        );
        command(&self.main, ["commit", "-am", "add nested"]);
        sha
    }
}

fn command<const N: usize>(directory: &Path, arguments: [&str; N]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn commit_all(repository: &Repository, message: &str) {
    let mut index = repository.index().unwrap();
    index
        .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let signature = Signature::now("test", "test@example.com").unwrap();
    let parents = repository
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok());
    let parent_refs = parents.iter().collect::<Vec<_>>();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parent_refs,
        )
        .unwrap();
}

#[test]
fn list_worktrees_and_branch_existence_use_git_metadata() {
    let fixture = Fixture::new();
    let git = fixture.git();
    git.create_worktree("one", "session-one", "HEAD").unwrap();
    git.create_worktree("two", "session-two", "HEAD").unwrap();
    assert_eq!(
        git.list_worktrees()
            .unwrap()
            .iter()
            .map(|worktree| worktree.name.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert!(git.branch_exists("session-one").unwrap());
    assert!(!git.branch_exists("missing").unwrap());
}

#[test]
fn dirty_and_ahead_behind_report_worktree_state() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("one", "session-one", "HEAD").unwrap();
    assert!(!git.is_dirty(&worktree.path).unwrap());
    fs::write(worktree.path.join("untracked.txt"), "untracked\n").unwrap();
    assert!(git.is_dirty(&worktree.path).unwrap());
    fs::remove_file(worktree.path.join("untracked.txt")).unwrap();
    fs::write(worktree.path.join("tracked.txt"), "changed\n").unwrap();
    assert!(git.is_dirty(&worktree.path).unwrap());
    fs::write(worktree.path.join("tracked.txt"), "advanced\n").unwrap();
    assert_eq!(git.ahead_behind(&worktree.path, "HEAD~0").unwrap(), (0, 0));
    command(&worktree.path, ["add", "tracked.txt"]);
    command(
        &worktree.path,
        [
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-m",
            "advance",
        ],
    );
    assert_eq!(git.ahead_behind(&worktree.path, "HEAD~1").unwrap(), (1, 0));
}

#[test]
fn dirty_paths_report_tracked_and_untracked_files() {
    let fixture = Fixture::new();
    let git = fixture.git();
    fs::write(fixture.main.join("tracked.txt"), "changed\n").unwrap();
    fs::write(fixture.main.join("untracked.txt"), "new\n").unwrap();

    assert_eq!(
        git.dirty_paths(&fixture.main).unwrap(),
        vec![
            super::DirtyPath {
                path: "tracked.txt".into(),
                kind: super::DirtyPathKind::Tracked,
            },
            super::DirtyPath {
                path: "untracked.txt".into(),
                kind: super::DirtyPathKind::Untracked,
            },
        ]
    );
}

#[test]
fn stash_push_creates_an_entry_with_the_requested_message() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let message = "preserve-main-changes";
    fs::write(fixture.main.join("tracked.txt"), "changed\n").unwrap();

    git.stash_push(message).unwrap();

    assert!(git.stash_contains_message(message).unwrap());
}

#[test]
fn create_worktree_populates_submodules_and_reports_gitlink() {
    let fixture = Fixture::new();
    let sha = fixture.add_submodule();
    let git = fixture.git();
    let worktree = git
        .create_worktree("with-submodule", "session-submodule", "HEAD")
        .unwrap();
    assert!(worktree.path.is_dir());
    assert_eq!(worktree.branch.as_deref(), Some("session-submodule"));
    assert!(worktree.path.join("nested").is_dir());
    assert_eq!(git.gitlink_sha(&worktree.path, "nested").unwrap(), sha);
}

#[test]
fn create_worktree_rolls_back_when_submodule_commit_is_unavailable() {
    let fixture = Fixture::new();
    fixture.add_submodule_named("good");
    fixture.add_submodule();
    let fake_sha = "0123456789012345678901234567890123456789";
    command(
        &fixture.main,
        [
            "update-index",
            "--cacheinfo",
            &format!("160000,{fake_sha},nested"),
        ],
    );
    command(&fixture.main, ["commit", "-m", "broken gitlink"]);
    let git = fixture.git();
    assert!(
        git.create_worktree("broken", "session-broken", "HEAD")
            .is_err()
    );
    assert!(!fixture.main.join(".worktrees/broken").exists());
    assert!(!git.branch_exists("session-broken").unwrap());
    assert!(git.list_worktrees().unwrap().is_empty());
}

#[test]
fn worktree_move_repairs_a_worktree_containing_a_submodule() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    fs::write(fixture.main.join(".git/info/exclude"), "marker.txt\n").unwrap();
    let git = fixture.git();
    let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
    fs::write(old.path.join("marker.txt"), "keep\n").unwrap();
    let new = fixture.main.join(".worktrees/new");

    git.worktree_move(&old.path, &new).unwrap();

    command(&new, ["rev-parse", "--git-dir"]);
    command(&new.join("nested"), ["rev-parse", "--git-dir"]);
    assert_eq!(
        fs::read_to_string(new.join("marker.txt")).unwrap(),
        "keep\n"
    );
    let paths = git
        .list_worktrees()
        .unwrap()
        .into_iter()
        .map(|worktree| worktree.path)
        .collect::<Vec<_>>();
    assert!(paths.iter().any(|path| super::paths_equal(path, &new)));
    assert!(!paths.iter().any(|path| super::paths_equal(path, &old.path)));
}

#[test]
fn worktree_move_refuses_an_existing_destination() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
    let destination = fixture.main.join(".worktrees/existing");
    fs::create_dir_all(&destination).unwrap();

    assert!(git.worktree_move(&old.path, &destination).is_err());
    assert!(old.path.exists());
}

#[test]
fn worktree_move_rolls_back_when_nested_repair_fails() {
    let fixture = Fixture::new();
    fixture.add_submodule();
    let git = fixture.git();
    let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
    let destination = fixture.main.join(".worktrees/new");
    fs::remove_dir_all(fixture.main.join("nested")).unwrap();

    assert!(git.worktree_move(&old.path, &destination).is_err());
    assert!(old.path.exists());
    command(&old.path, ["rev-parse", "--git-dir"]);
    assert!(!destination.exists());
}

#[test]
fn rename_worktree_moves_in_place_and_preserves_marker() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let old = git.create_worktree("old", "session-old", "HEAD").unwrap();
    fs::write(old.path.join("marker.txt"), "keep\n").unwrap();
    let renamed = git
        .rename_worktree("old", Path::new("new"), "session-new")
        .unwrap();
    assert!(!old.path.exists());
    assert_eq!(
        fs::read_to_string(renamed.path.join("marker.txt")).unwrap(),
        "keep\n"
    );
    assert!(!git.branch_exists("session-old").unwrap());
    assert!(git.branch_exists("session-new").unwrap());
    assert_eq!(renamed.branch.as_deref(), Some("session-new"));
}

#[test]
fn remove_and_prune_clear_worktree_registration() {
    let fixture = Fixture::new();
    let git = fixture.git();
    let worktree = git.create_worktree("one", "session-one", "HEAD").unwrap();
    git.worktree_remove_force(&worktree.path).unwrap();
    git.worktree_prune().unwrap();
    assert!(git.list_worktrees().unwrap().is_empty());
}

#[test]
fn index_rebuild_skips_missing_store_paths() {
    let fixture = TempDir::new().unwrap();
    let outcomes = super::rebuild_entity_indexes(fixture.path());

    assert!(matches!(
        outcomes.as_slice(),
        [
            super::IndexRebuildOutcome::Skipped {
                store: super::EntityStore::Ticket,
                ..
            },
            super::IndexRebuildOutcome::Skipped {
                store: super::EntityStore::Spec,
                ..
            },
        ]
    ));
}

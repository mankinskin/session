use clap::Parser;

use super::{
    BRANCH_TEMPLATE, Cli, Command, DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE,
    FINISH_READY_TO_MERGE_MARKER, PRESERVE_MAIN_CHANGES_HINT, WORKTREE_PATH_OUTPUT_PREFIX,
    WORKTREE_PATH_TEMPLATE, WorktreeSelection,
};

#[test]
fn defines_lifecycle_output_contract_constants() {
    assert_eq!(WORKTREE_PATH_OUTPUT_PREFIX, "WORKTREE_PATH=");
    assert_eq!(FINISH_READY_TO_MERGE_MARKER, "ready-to-merge");
    assert_eq!(
        DIRTY_MAIN_UNCOMMITTED_CHANGES_MESSAGE,
        "uncommitted changes"
    );
    assert_eq!(PRESERVE_MAIN_CHANGES_HINT, "preserve-main-changes");
    assert_eq!(
        WORKTREE_PATH_TEMPLATE,
        ".worktrees/<full-session-uuid>/<slug>"
    );
    assert_eq!(BRANCH_TEMPLATE, "agent/<full-session-uuid>/<slug>");
}

#[test]
fn parses_new_with_all_flags() {
    let cli = Cli::try_parse_from([
        "worktree-ctl",
        "new",
        "12345678-1234-1234-1234-123456789abc",
        "worktree-ctl",
        "--dry-run",
        "--preserve-main-changes",
    ])
    .unwrap();

    assert_eq!(
        cli.command,
        Command::New {
            session_uuid: "12345678-1234-1234-1234-123456789abc".to_owned(),
            slug: "worktree-ctl".to_owned(),
            dry_run: true,
            preserve_main_changes: true,
        }
    );
}

#[test]
fn parses_list() {
    let cli = Cli::try_parse_from(["worktree-ctl", "list"]).unwrap();

    assert_eq!(
        cli.command,
        Command::List {
            dry_run: false,
            verbose: false
        }
    );
}

#[test]
fn parses_rebase_with_dry_run() {
    let cli = Cli::try_parse_from(["worktree-ctl", "rebase", "example", "--dry-run"]).unwrap();

    assert_eq!(
        cli.command,
        Command::Rebase {
            selection: WorktreeSelection {
                names: vec!["example".to_owned()],
                worktrees: Vec::new(),
                all: false,
            },
            dry_run: true,
            auto_commit: false,
        }
    );
}

#[test]
fn parses_merge_with_dry_run() {
    let cli = Cli::try_parse_from(["worktree-ctl", "merge", "example", "--dry-run"]).unwrap();

    assert_eq!(
        cli.command,
        Command::Merge {
            selection: WorktreeSelection {
                names: vec!["example".to_owned()],
                worktrees: Vec::new(),
                all: false,
            },
            dry_run: true,
            auto_commit: false,
        }
    );
}

#[test]
fn parses_sync_with_dry_run() {
    let cli = Cli::try_parse_from(["worktree-ctl", "sync", "example", "--dry-run"]).unwrap();

    assert_eq!(
        cli.command,
        Command::Sync {
            selection: WorktreeSelection {
                names: vec!["example".to_owned()],
                worktrees: Vec::new(),
                all: false,
            },
            dry_run: true,
            auto_commit: false,
        }
    );
}

#[test]
fn parses_sync_with_auto_commit() {
    let cli = Cli::try_parse_from(["worktree-ctl", "sync", "example", "--auto-commit"]).unwrap();

    assert_eq!(
        cli.command,
        Command::Sync {
            selection: WorktreeSelection {
                names: vec!["example".to_owned()],
                worktrees: Vec::new(),
                all: false,
            },
            dry_run: false,
            auto_commit: true,
        }
    );
}

#[test]
fn parses_repeated_worktree_selection_and_all() {
    let selected = Cli::try_parse_from([
        "worktree-ctl",
        "rebase",
        "--worktree",
        "first",
        "--worktree",
        "second",
    ])
    .unwrap();
    assert_eq!(
        selected.command,
        Command::Rebase {
            selection: WorktreeSelection {
                names: Vec::new(),
                worktrees: vec!["first".to_owned(), "second".to_owned()],
                all: false,
            },
            dry_run: false,
            auto_commit: false,
        }
    );

    let all = Cli::try_parse_from(["worktree-ctl", "clean", "--all"]).unwrap();
    assert_eq!(
        all.command,
        Command::Clean {
            selection: WorktreeSelection {
                names: Vec::new(),
                worktrees: Vec::new(),
                all: true,
            },
            dry_run: false,
        }
    );
}

#[test]
fn parses_commit_pathspecs_after_double_dash() {
    let cli = Cli::try_parse_from([
        "worktree-ctl",
        "commit",
        "example",
        "--",
        "src/lib.rs",
        "README.md",
    ])
    .unwrap();
    assert_eq!(
        cli.command,
        Command::Commit {
            selection: WorktreeSelection {
                names: vec!["example".to_owned()],
                worktrees: Vec::new(),
                all: false,
            },
            message: "worktree-ctl commit".to_owned(),
            paths: vec!["src/lib.rs".into(), "README.md".into()],
            dry_run: false,
        }
    );
}

#[test]
fn parses_remove_with_force_and_dry_run() {
    let cli =
        Cli::try_parse_from(["worktree-ctl", "remove", "example", "--force", "--dry-run"]).unwrap();

    assert_eq!(
        cli.command,
        Command::Remove {
            name: "example".to_owned(),
            force: true,
            dry_run: true,
        }
    );
}

#[test]
fn parses_rename_with_dry_run() {
    let cli = Cli::try_parse_from([
        "worktree-ctl",
        "rename",
        "old-name",
        "new-name",
        "--dry-run",
    ])
    .unwrap();

    assert_eq!(
        cli.command,
        Command::Rename {
            source_name: "old-name".to_owned(),
            target_name: "new-name".to_owned(),
            dry_run: true,
        }
    );
}

#[test]
fn parses_finish_with_dry_run() {
    let cli = Cli::try_parse_from(["worktree-ctl", "finish", "example", "--dry-run"]).unwrap();

    assert_eq!(
        cli.command,
        Command::Finish {
            name: "example".to_owned(),
            dry_run: true,
        }
    );
}

#[test]
fn parses_doctor_with_dry_run() {
    let cli = Cli::try_parse_from(["worktree-ctl", "doctor", "--dry-run"]).unwrap();

    assert_eq!(cli.command, Command::Doctor { dry_run: true });
}

#[test]
fn accepts_dry_run_for_every_mutating_subcommand() {
    for args in [
        vec![
            "new",
            "12345678-1234-1234-1234-123456789abc",
            "slug",
            "--dry-run",
        ],
        vec!["rebase", "example", "--dry-run"],
        vec!["merge", "example", "--dry-run"],
        vec!["sync", "example", "--dry-run"],
        vec!["remove", "example", "--dry-run"],
        vec!["rename", "old", "new", "--dry-run"],
        vec!["finish", "example", "--dry-run"],
        vec!["doctor", "--dry-run"],
    ] {
        let mut command_line = vec!["worktree-ctl"];
        command_line.extend(args);
        assert!(Cli::try_parse_from(command_line).is_ok());
    }
}

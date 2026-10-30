#[allow(unused_imports)]
use super::*;

pub(super) fn normalize_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `run_in_terminal` command taxonomy (ticket `77eb143b` AC1). Every
/// classified command falls into exactly one category, in this priority
/// order: a read-like head wins over everything else, then a `cargo run -p
/// *-cli` invocation, then a CLI binary shadowing a loaded MCP tool, then
/// known-legitimate dev commands, else `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ShellCommandCategory {
    /// (a) `cat`/`head`/`tail`/`grep`/`find`/`ls`/... — substitutable by
    /// `peek-mcp`/`grep_search`/`read_file`.
    ReadLikeExploratory,
    /// (b) A CLI binary shadowing a loaded MCP tool (e.g. `ticket.exe get`
    /// where `ticket-mcp` is loaded).
    CliShadowingMcp,
    /// (c) `cargo run -p *-cli` — compiling/running a repo CLI at runtime
    /// instead of calling the equivalent MCP tool. Disallowed per AC6.
    CargoRunCli,
    /// (d) Legitimate dev commands: `cargo build`/`test`/`check`/`clippy`,
    /// `git`.
    LegitimateDev,
    /// Anything not matched by the above.
    Other,
}

impl ShellCommandCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellCommandCategory::ReadLikeExploratory => "read_like_exploratory",
            ShellCommandCategory::CliShadowingMcp => "cli_shadowing_mcp",
            ShellCommandCategory::CargoRunCli => "cargo_run_cli",
            ShellCommandCategory::LegitimateDev => "legitimate_dev",
            ShellCommandCategory::Other => "other",
        }
    }
}

/// The invoked binary's basename: the final path component, so
/// `./target/debug/ticket.exe` and `ticket` both yield `ticket`/`ticket.exe`
/// respectively (directory separators only, extension is left intact so a
/// bare `test` command and a `test.exe` binary can be told apart).
pub(super) fn command_basename(token: &str) -> &str {
    token.rsplit(['/', '\\']).next().unwrap_or(token)
}

/// The MCP tool-name prefix a CLI-shadowing shell command corresponds to, if
/// any, given its invoked binary `basename` and (for the ambiguous bare
/// `test` binary) its immediate next token.
pub(super) fn mcp_family_for_command(
    basename: &str,
    second_token: Option<&str>,
) -> Option<&'static str> {
    if basename == "test" {
        return second_token
            .filter(|t| TEST_CLI_SUBCOMMANDS.contains(t))
            .map(|_| "mcp_test-mcp_");
    }
    CLI_SHADOW_BASENAMES
        .iter()
        .find(|(cli_basename, _)| *cli_basename == basename)
        .map(|(_, family)| *family)
}

/// Strip a leading `cd <path> &&`/`cd <path>;` chain from a captured shell
/// command. Captured sessions frequently re-`cd` to the workspace root
/// before the command that actually matters (shells are not guaranteed to
/// preserve `cwd` across turns); classifying by the first token alone would
/// otherwise misclassify `cd /repo && ./target/debug/ticket.exe get ...` as
/// an unclassified `cd` invocation instead of `cli_shadowing_mcp` (ticket
/// `77eb143b` AC1/AC2).
pub(super) fn strip_leading_cd_chain(command: &str) -> &str {
    let mut rest = command.trim_start();
    while rest.starts_with("cd ") {
        if let Some(idx) = rest.find("&&") {
            rest = rest[idx + 2..].trim_start();
            continue;
        }
        if let Some(idx) = rest.find(';') {
            rest = rest[idx + 1..].trim_start();
            continue;
        }
        break;
    }
    rest
}

/// Whitespace-split tokens of a shell command, skipping a leading `rtk`
/// proxy prefix (see `AGENTS.md`'s `rtk <cmd>` convention).
pub(super) fn normalized_tokens(command: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = command.split_whitespace().collect();
    if tokens.first() == Some(&"rtk") {
        tokens.remove(0);
    }
    tokens
}

/// The MCP tool-name prefix shadowed by `command`, if any, after stripping a
/// leading `cd` chain and `rtk` prefix and matching the invoked binary's
/// basename.
pub(super) fn mcp_family_for_raw_command(command: &str) -> Option<&'static str> {
    let tokens = normalized_tokens(strip_leading_cd_chain(command));
    let basename = command_basename(*tokens.first()?);
    mcp_family_for_command(basename, tokens.get(1).copied())
}

/// True if `tokens` (already `cd`-chain- and `rtk`-stripped) invoke `cargo
/// run -p <crate>-cli` / `cargo run --package <crate>-cli` — compiling and
/// running a repo CLI binary at runtime instead of calling the equivalent
/// MCP tool (ticket `77eb143b` AC6).
pub(super) fn is_cargo_run_cli(tokens: &[&str]) -> bool {
    tokens.iter().any(|t| *t == "run")
        && tokens.iter().enumerate().any(|(idx, t)| {
            (*t == "-p" || *t == "--package")
                && tokens
                    .get(idx + 1)
                    .map(|v| v.ends_with("-cli"))
                    .unwrap_or(false)
        })
}

/// Classify a raw `run_in_terminal` command into its taxonomy category
/// (ticket `77eb143b` AC1).
pub fn classify_shell_command(command: &str) -> ShellCommandCategory {
    // Every category — including the read-like check below — looks past a
    // leading `cd <path> &&`/`;` chain, so a path-qualified invocation
    // classifies the same as a bare one (ticket `77eb143b` defect 1: the
    // read-like branch previously ran `command_head` on the raw,
    // un-stripped command while every other branch stripped the `cd`
    // chain first, so `cd /repo && cat file.md` fell through past
    // `ReadLikeExploratory` into `Other` instead of being recognized as a
    // read-like command).
    let stripped = strip_leading_cd_chain(command);
    if let Some(head) = command_head(stripped) {
        if SUBSTITUTABLE_SHELL_HEADS.contains(&head) {
            return ShellCommandCategory::ReadLikeExploratory;
        }
    }

    let tokens = normalized_tokens(stripped);
    let Some(&head) = tokens.first() else {
        return ShellCommandCategory::Other;
    };
    let basename = command_basename(head);
    if basename == "cargo" {
        if is_cargo_run_cli(&tokens) {
            return ShellCommandCategory::CargoRunCli;
        }
        return ShellCommandCategory::LegitimateDev;
    }
    if basename == "git" {
        return ShellCommandCategory::LegitimateDev;
    }
    if mcp_family_for_command(basename, tokens.get(1).copied()).is_some() {
        return ShellCommandCategory::CliShadowingMcp;
    }
    ShellCommandCategory::Other
}

/// The first whitespace-separated token of a shell command, skipping a
/// leading `rtk` proxy prefix (see `AGENTS.md`'s `rtk <cmd>` convention) so
/// `rtk cat foo` and `cat foo` classify identically.
pub(super) fn command_head(command: &str) -> Option<&str> {
    let mut parts = command.split_whitespace();
    let mut head = parts.next()?;
    if head == "rtk" {
        head = parts.next()?;
    }
    Some(head)
}

use std::{
    path::Path,
    process::Command as ProcessCommand,
};

pub(crate) fn run_git<const N: usize>(
    repository: &Path,
    arguments: [&str; N],
) -> Result<(), String> {
    let output = git_command(repository)
        .args(arguments)
        .output()
        .map_err(|error| format!("failed to start git: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub(crate) fn git_command(repository: &Path) -> ProcessCommand {
    let mut command = ProcessCommand::new("git");
    command.current_dir(normalized_git_path(repository));
    command
}

fn normalized_git_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let path = path
        .strip_prefix(r"\\?\UNC\")
        .map(|path| format!("//{path}"))
        .or_else(|| path.strip_prefix(r"\\?\").map(str::to_owned))
        .unwrap_or_else(|| path.into_owned());
    path.replace('\\', "/")
}

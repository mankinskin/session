use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::Command,
    time::{
        SystemTime,
        UNIX_EPOCH,
    },
};

use tempfile::TempDir;

pub const FIXTURE_SESSION_ID: &str = "fixture-local-a";
pub const LOCAL_FIXTURE_SESSION_ID: &str = "session-workspace-fixture";

pub fn local_fixture_a() -> &'static str {
    include_str!("../fixtures/local_parse_fixture_a.jsonl")
}

pub fn local_fixture_b() -> &'static str {
    include_str!("../fixtures/local_parse_fixture_b.jsonl")
}

pub fn local_fixture_c() -> &'static str {
    include_str!("../fixtures/local_parse_fixture_c.jsonl")
}

pub fn local_fixture_scenarios()
-> [(&'static str, &'static str, &'static str); 3] {
    [
        ("fixture-a.jsonl", local_fixture_a(), "fixture-local-a"),
        ("fixture-b.jsonl", local_fixture_b(), "fixture-local-b"),
        ("fixture-c.jsonl", local_fixture_c(), "fixture-local-c"),
    ]
}

pub fn repo_root_from_manifest(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir)
        .join("../..")
        .canonicalize()
        .expect("session-api crate should live under repo root")
}

pub fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

pub fn find_cargo_bin() -> Option<String> {
    if let Ok(path) = std::env::var("CARGO") {
        if !path.trim().is_empty() {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(cargo_home) = std::env::var("CARGO_HOME") {
        candidates.push(
            PathBuf::from(cargo_home)
                .join("bin")
                .join(if cfg!(windows) { "cargo.exe" } else { "cargo" }),
        );
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(userprofile)
                .join(".cargo")
                .join("bin")
                .join("cargo.exe"),
        );
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".cargo")
                .join("bin")
                .join(if cfg!(windows) { "cargo.exe" } else { "cargo" }),
        );
    }

    for candidate in candidates {
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }

    #[cfg(windows)]
    {
        if let Ok(output) = Command::new("where").arg("cargo").output() {
            if output.status.success() {
                if let Some(first) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find(|line| !line.trim().is_empty())
                {
                    return Some(first.trim().to_string());
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(output) = Command::new("which").arg("cargo").output() {
            if output.status.success() {
                if let Some(first) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find(|line| !line.trim().is_empty())
                {
                    return Some(first.trim().to_string());
                }
            }
        }
    }

    None
}

pub fn shell_single_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

pub fn write_fixture_transcript(
    dir: &Path,
    name: &str,
    content: &str,
) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, content)
        .expect("write local deterministic fixture transcript");
    path
}

pub struct ScriptWorkspaceFixture {
    _workspace: TempDir,
    pub root: PathBuf,
    pub store_root: PathBuf,
}

impl ScriptWorkspaceFixture {
    pub fn new(script_source: &Path) -> Self {
        let workspace =
            TempDir::new().expect("create isolated workspace fixture tempdir");
        let root = workspace.path().join("workspace-fixture");
        let fixture_tools = root.join("tools").join("agent-hooks");
        let fixture_transcripts = root.join("transcripts");
        let store_root = root.join("session-store");

        fs::create_dir_all(&fixture_tools)
            .expect("create fixture hook directory");
        fs::create_dir_all(&fixture_transcripts)
            .expect("create fixture transcript directory");
        fs::create_dir_all(&store_root)
            .expect("create fixture session store root");

        let fixture_script_path = fixture_tools.join("session-capture-stop.sh");
        fs::copy(script_source, &fixture_script_path)
            .expect("copy hook script into fixture workspace");

        Self {
            _workspace: workspace,
            root,
            store_root,
        }
    }

    pub fn configure_hook_command(
        &self,
        command: &mut Command,
    ) {
        command.env("MCP_MAIN_CHECKOUT", &self.root);
    }

    pub fn transcript_path(
        &self,
        file_name: &str,
    ) -> PathBuf {
        self.root.join("transcripts").join(file_name)
    }

    pub fn script_path_shell() -> String {
        PathBuf::from("tools/agent-hooks/session-capture-stop.sh")
            .to_string_lossy()
            .to_string()
    }
}

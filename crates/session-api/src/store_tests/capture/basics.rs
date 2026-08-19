use chrono::TimeZone;
use tempfile::TempDir;

use crate::{
    CopilotHookMessage,
    CopilotHookPayload,
    PersistedSessionEvents,
    PersistedSessionManifest,
    PersistedSessionTranscript,
    SESSION_SCHEMA_VERSION,
    SessionAuditSelector,
    SessionCaptureRequest,
    SessionError,
    SessionProvisioningDiagnostic,
    SessionQuery,
    SessionRole,
    SessionRuntimeInitRequest,
    SessionStoreConfig,
    SessionTicketStateResolver,
    SessionWorkflowEdgeKind,
    SessionWorkflowNodeDraft,
    SessionWorkflowNodeKind,
    SessionWorkflowNodeRequirement,
    SessionWorkflowNodeStatus,
    SessionWorktreeAllocationMode,
    SessionWorktreeCheckInRequest,
    SessionWorktreeStatus,
    store::{
        WorktreeCheckInFailurePoint,
    },
};
use uuid::Uuid;

const WORKTREE_SESSION_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const WORKTREE_SESSION_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const WORKTREE_SESSION_STRICT: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const WORKTREE_SESSION_FORWARD: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const WORKTREE_SESSION_GOOD: &str = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
const WORKTREE_SESSION_REAL: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
const RUNTIME_SESSION_MENTIONED: &str = "11111111-1111-4111-8111-111111111111";
const RUNTIME_SESSION_HANDOFF: &str = "22222222-2222-4222-8222-222222222222";

fn sample_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .with_ymd_and_hms(2026, 6, 2, 13, 0, 0)
        .single()
        .unwrap()
}

fn sample_time_later() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc
        .with_ymd_and_hms(2026, 6, 2, 13, 5, 0)
        .single()
        .unwrap()
}

fn sample_payload(
    session_id: &str,
    conversation_id: Option<&str>,
    captured_at: chrono::DateTime<chrono::Utc>,
    messages: &[&str],
) -> CopilotHookPayload {
    CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_slug: "context-engine".to_string(),
        captured_at,
        conversation_id: conversation_id.map(str::to_string),
        agent_id: Some("github-copilot-gpt-5.4".to_string()),
        model: Some("GPT-5.4".to_string()),
        trigger: Some("post-turn".to_string()),
        provisioning: None,
        messages: messages
            .iter()
            .enumerate()
            .map(|(index, content)| CopilotHookMessage {
                role: if index % 2 == 0 {
                    SessionRole::User
                } else {
                    SessionRole::Assistant
                },
                content: (*content).to_string(),
                tool_name: None,
                captured_at: None,
                event_meta: None,
            })
            .collect(),
        events: vec![],
        runtime: None,
    }
}

fn sample_request(
    session_id: &str,
    conversation_id: Option<&str>,
    captured_at: chrono::DateTime<chrono::Utc>,
    messages: &[&str],
) -> SessionCaptureRequest {
    SessionCaptureRequest::copilot(sample_payload(
        session_id,
        conversation_id,
        captured_at,
        messages,
    ))
}

fn request_with_provisioning(
    session_id: &str,
    diagnostic: Option<SessionProvisioningDiagnostic>,
) -> SessionCaptureRequest {
    let mut request = sample_request(
        session_id,
        Some("conversation-abc"),
        sample_time(),
        &["Persist this chat"],
    );
    request.payload.provisioning = diagnostic;
    request
}

fn sample_worktree_request(
    session_id: &str,
    owner_id: &str,
    ticket_id: &str,
    worktree_path: std::path::PathBuf,
    branch: &str,
) -> SessionWorktreeCheckInRequest {
    SessionWorktreeCheckInRequest {
        session_id: session_id.to_string(),
        owner_id: owner_id.to_string(),
        ticket_id: ticket_id.to_string(),
        worktree_path,
        branch: branch.to_string(),
        predecessor_session_id: None,
    }
}

fn managed_worktree(
    tempdir: &TempDir,
    session_id: &str,
    slug: &str,
    branch: &str,
) -> std::path::PathBuf {
    let main_checkout = tempdir.path();
    let repository = git2::Repository::open(main_checkout)
        .or_else(|_| git2::Repository::init(main_checkout))
        .unwrap();
    if repository.head().is_err() {
        let mut index = repository.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repository.find_tree(tree_id).unwrap();
        let signature =
            git2::Signature::now("session-api tests", "tests@example.invalid")
                .unwrap();
        repository
            .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap();
    }

    let path = main_checkout.join(".worktrees").join(session_id).join(slug);
    let status = std::process::Command::new("git")
        .current_dir(main_checkout)
        .args(["worktree", "add", "-b", branch])
        .arg(&path)
        .arg("HEAD")
        .status()
        .unwrap();
    assert!(status.success(), "failed to create managed test worktree");
    std::fs::canonicalize(path).unwrap()
}

#[test]
fn store_plan_uses_session_id_in_paths() {
    let config = SessionStoreConfig::new(".session", "context-engine");
    let plan = config
        .plan_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time(),
            &["Persist this chat"],
        ))
        .unwrap();

    assert_eq!(
        plan.paths.manifest_path,
        std::path::PathBuf::from(".session/sessions/session-abc/session.json")
    );
    assert_eq!(
        plan.paths.transcript_path,
        std::path::PathBuf::from(
            ".session/sessions/session-abc/transcript.json"
        )
    );
}

#[test]
fn store_plan_rejects_invalid_path_segments() {
    let config = SessionStoreConfig::new(".session", "context-engine");
    let mut request = sample_request(
        "session-abc",
        Some("conversation-abc"),
        sample_time(),
        &["Persist this chat"],
    );
    request.payload.session_id = "session/abc".to_string();

    let error = config.plan_capture(request).unwrap_err();

    assert!(matches!(
        error,
        SessionError::InvalidSessionId(ref value) if value == "session/abc"
    ));
}

#[test]
fn persist_capture_writes_manifest_and_transcript_files() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let plan = config
        .persist_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time(),
            &["Persist this chat"],
        ))
        .unwrap();
    let manifest_text =
        std::fs::read_to_string(&plan.paths.manifest_path).unwrap();
    let transcript_text =
        std::fs::read_to_string(&plan.paths.transcript_path).unwrap();

    let manifest: PersistedSessionManifest =
        serde_json::from_str(&manifest_text).unwrap();
    let transcript: PersistedSessionTranscript =
        serde_json::from_str(&transcript_text).unwrap();

    assert_eq!(manifest.session_id, "session-abc");
    assert_eq!(manifest.metadata.workspace_slug, "context-engine");
    assert_eq!(transcript.session_id, "session-abc");
    assert_eq!(transcript.turns.len(), 1);
    assert_eq!(transcript.turns[0].content, "Persist this chat");
}

#[test]
fn persist_capture_appends_only_new_turns_from_later_capture() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    config
        .persist_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time(),
            &["first"],
        ))
        .unwrap();

    let plan = config
        .persist_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time_later(),
            &["first", "second"],
        ))
        .unwrap();
    config
        .persist_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time_later(),
            &["first", "second"],
        ))
        .unwrap();
    let transcript_text =
        std::fs::read_to_string(&plan.paths.transcript_path).unwrap();
    let transcript: PersistedSessionTranscript =
        serde_json::from_str(&transcript_text).unwrap();

    assert_eq!(transcript.turns.len(), 2);
    assert_eq!(transcript.turns[0].content, "first");
    assert_eq!(transcript.turns[0].captured_at, sample_time());
    assert_eq!(transcript.turns[1].content, "second");
    assert_eq!(transcript.turns[1].captured_at, sample_time_later());
}

#[test]
fn persist_capture_retains_user_prompt_submit_provisioning_diagnostic() {
    let tempdir = TempDir::new().unwrap();
    let store_path = tempdir.path().join("store");
    let config = SessionStoreConfig::new(&store_path, "context-engine");

    let post_tool_use = SessionProvisioningDiagnostic {
        outcome: "skipped".to_string(),
        reason: Some("trigger_not_user_prompt_submit".to_string()),
        hook_event_name: "PostToolUse".to_string(),
    };
    let user_prompt_submit = SessionProvisioningDiagnostic {
        outcome: "created".to_string(),
        reason: None,
        hook_event_name: "userpromptsubmit".to_string(),
    };
    let stop = SessionProvisioningDiagnostic {
        outcome: "skipped".to_string(),
        reason: Some("trigger_not_user_prompt_submit".to_string()),
        hook_event_name: "Stop".to_string(),
    };

    config
        .persist_capture(request_with_provisioning(
            "session-non-user-then-user",
            Some(post_tool_use.clone()),
        ))
        .unwrap();
    config
        .persist_capture(request_with_provisioning(
            "session-non-user-then-user",
            Some(user_prompt_submit.clone()),
        ))
        .unwrap();

    let fresh_config = SessionStoreConfig::new(&store_path, "context-engine");
    assert_eq!(
        fresh_config
            .read_session("session-non-user-then-user")
            .unwrap()
            .metadata
            .provisioning,
        Some(user_prompt_submit.clone())
    );

    config
        .persist_capture(request_with_provisioning(
            "session-non-user-then-user",
            Some(post_tool_use.clone()),
        ))
        .unwrap();
    assert_eq!(
        fresh_config
            .read_session("session-non-user-then-user")
            .unwrap()
            .metadata
            .provisioning,
        Some(user_prompt_submit.clone())
    );

    config
        .persist_capture(request_with_provisioning(
            "session-non-user-first-wins",
            Some(post_tool_use.clone()),
        ))
        .unwrap();
    config
        .persist_capture(request_with_provisioning(
            "session-non-user-first-wins",
            Some(stop),
        ))
        .unwrap();
    assert_eq!(
        config
            .read_session("session-non-user-first-wins")
            .unwrap()
            .metadata
            .provisioning,
        Some(post_tool_use)
    );

    config
        .persist_capture(request_with_provisioning(
            "session-none-then-user",
            None,
        ))
        .unwrap();
    config
        .persist_capture(request_with_provisioning(
            "session-none-then-user",
            Some(user_prompt_submit.clone()),
        ))
        .unwrap();
    assert_eq!(
        config
            .read_session("session-none-then-user")
            .unwrap()
            .metadata
            .provisioning,
        Some(user_prompt_submit)
    );
}

#[test]
fn read_session_reconstructs_persisted_record() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    config
        .persist_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time(),
            &["first"],
        ))
        .unwrap();
    config
        .persist_capture(sample_request(
            "session-abc",
            Some("conversation-abc"),
            sample_time_later(),
            &["first", "second"],
        ))
        .unwrap();

    let record = config.read_session("session-abc").unwrap();

    assert_eq!(record.session_id, "session-abc");
    assert_eq!(record.started_at, sample_time());
    assert_eq!(record.captured_at, sample_time_later());
    assert_eq!(record.turns.len(), 2);
    assert_eq!(record.turns[0].content, "first");
    assert_eq!(record.turns[1].content, "second");
}

#[test]
fn capture_copilot_hook_persists_payload() {
    let tempdir = TempDir::new().unwrap();
    let config =
        SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");

    let plan = config
        .capture_copilot_hook(sample_payload(
            "session-hook",
            Some("conversation-hook"),
            sample_time(),
            &["Persist from hook"],
        ))
        .unwrap();
    let record = config.read_session("session-hook").unwrap();

    assert!(plan.paths.manifest_path.exists());
    assert_eq!(record.session_id, "session-hook");
    assert_eq!(record.turns.len(), 1);
    assert_eq!(record.turns[0].content, "Persist from hook");
}

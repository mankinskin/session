//! Session-owned decision receipts; fixtures prove plumbing, not human identity.
use session_api::{
    DecisionFault, DecisionKind, HostPromptIngress, RequesterChallengeRequest, SessionError,
    SessionRuntimeInitRequest, SessionStoreConfig, parse_decision_prompt,
};
use uuid::Uuid;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const MISSION: &str = "22222222-2222-4222-8222-222222222222";
const P: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const R: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn store() -> (tempfile::TempDir, SessionStoreConfig) {
    let dir = tempfile::tempdir().unwrap();
    let config = SessionStoreConfig::new(dir.path().join("session-store"));
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(SESSION.into()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();
    (dir, config)
}
fn request(dir: &std::path::Path, challenge: &str) -> RequesterChallengeRequest {
    RequesterChallengeRequest {
        session_id: SESSION.into(),
        challenge_id: challenge.into(),
        mission_id: Uuid::parse_str(MISSION).unwrap(),
        mission_root: dir.join("ws"),
        revision: 3,
        plan_digest: P.into(),
        review_snapshot_digest: R.into(),
    }
}
fn line(kind: &str, challenge: &str) -> String {
    format!("mission-decision {kind} {MISSION} 3 {P} {R} {challenge}")
}
fn host(name: &str, text: &str, id: Option<&str>) -> HostPromptIngress {
    HostPromptIngress::trusted_local_hook(name, text.as_bytes().to_vec(), id.map(str::to_owned))
}
fn code(error: &SessionError) -> &'static str {
    error
        .requester_code()
        .unwrap_or_else(|| panic!("no requester code: {error:?}"))
}

#[test]
fn parser_accepts_only_the_complete_standalone_line() {
    let ok = line("approve", "c1");
    let parsed = parse_decision_prompt(ok.as_bytes()).unwrap();
    assert_eq!(parsed.kind, DecisionKind::Approve);
    assert_eq!((parsed.revision, parsed.challenge_id.as_str()), (3, "c1"));
    assert!(parse_decision_prompt(format!("{ok}\n").as_bytes()).is_some());
    for kind in ["reject", "replan"] {
        assert!(parse_decision_prompt(line(kind, "c1").as_bytes()).is_some());
    }
    for bad in [
        format!("please {ok}"),
        format!("{ok} thanks"),
        format!("{ok}\nmore"),
        format!("\n{ok}"),
        format!("{ok}\r\n"),
        format!("{ok}\n\n"),
        line("approve", "c1").replace(P, &P.to_uppercase()),
        line("maybe", "c1"),
        format!("mission-decision approve {MISSION} 3 {P} {R}"),
        line("approve", "c1").replace(" 3 ", " three "),
        format!("```\n{ok}\n```"),
    ] {
        assert!(parse_decision_prompt(bad.as_bytes()).is_none(), "{bad:?}");
    }
    assert!(parse_decision_prompt(b"\xff\xfe").is_none());
}

#[test]
fn challenge_is_persisted_idempotent_and_divergent_reuse_conflicts() {
    let (dir, config) = store();
    let first = config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let again = config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(
        config.read_requester_challenge(SESSION, "c1").unwrap(),
        first
    );
    let mut changed = request(dir.path(), "c1");
    changed.review_snapshot_digest = P.into();
    let error = config.register_requester_challenge(&changed).unwrap_err();
    assert_eq!(code(&error), "requester_challenge_conflict");
    assert_eq!(
        config.render_decision_prompt(&first, DecisionKind::Approve),
        line("approve", "c1")
    );
}

#[test]
fn host_prompt_becomes_one_immutable_receipt_with_preserved_event_id() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let raw = format!("{}\n", line("approve", "c1"));
    let receipt = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, Some("host-evt-1")))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.reference.session_id, SESSION);
    assert_eq!(receipt.reference.source_event_id, "host-evt-1");
    assert_eq!(receipt.kind, DecisionKind::Approve);
    assert!(receipt.ingress.host_event_id_supplied && receipt.ingress.trusted_local_host);
    assert_eq!(receipt.read_raw_prompt().unwrap(), raw.as_bytes());
    assert_eq!(receipt.raw_prompt_sha256.len(), 64);
    let replay = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, Some("host-evt-1")))
        .unwrap()
        .unwrap();
    assert_eq!(replay.reference, receipt.reference);
    assert_eq!(
        config.read_requester_decision(&receipt.reference).unwrap(),
        receipt
    );
}

#[test]
fn absent_event_id_is_allocated_once_and_mirror_replay_keeps_identity() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let raw = line("approve", "c1");
    let a = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, None))
        .unwrap()
        .unwrap();
    assert!(!a.reference.source_event_id.is_empty());
    assert!(!a.ingress.host_event_id_supplied);
    let b = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, None))
        .unwrap()
        .unwrap();
    assert_eq!(a.reference, b.reference);
    let c = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, Some("mirror-id")))
        .unwrap_err();
    assert_eq!(code(&c), "requester_decision_conflict");
}

#[test]
fn non_host_ingress_and_non_decision_text_never_create_receipts() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let raw = line("approve", "c1");
    for name in [
        "PostToolUse",
        "Stop",
        "AssistantMessage",
        "reaction",
        "actor",
    ] {
        let error = config
            .ingest_requester_prompt(SESSION, &host(name, &raw, Some("x")))
            .unwrap_err();
        assert_eq!(code(&error), "approval_authority_unavailable", "{name}");
    }
    assert!(
        config
            .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", "just chatting", None))
            .unwrap()
            .is_none()
    );
    assert!(
        config
            .ingest_requester_prompt(
                SESSION,
                &host("UserPromptSubmit", &format!("see {raw}"), None)
            )
            .unwrap()
            .is_none()
    );
    let error = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "other"), None),
        )
        .unwrap_err();
    assert_eq!(code(&error), "approval_authority_unavailable");
}

#[test]
fn binding_mismatch_is_stale_and_creates_no_receipt() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    for raw in [
        line("approve", "c1").replace(" 3 ", " 4 "),
        line("approve", "c1").replace(P, R),
        line("approve", "c1").replace(MISSION, "33333333-3333-4333-8333-333333333333"),
    ] {
        let error = config
            .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, Some("e")))
            .unwrap_err();
        assert_eq!(code(&error), "approval_stale", "{raw}");
    }
}

#[test]
fn interrupted_event_receipt_write_is_pending_then_recovers_one_identity() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let raw = line("approve", "c1");
    let error = config
        .ingest_requester_prompt_with_fault(
            SESSION,
            &host("UserPromptSubmit", &raw, None),
            Some(DecisionFault::AfterRawPersisted),
        )
        .unwrap_err();
    assert_eq!(code(&error), "requester_decision_pending");
    assert_eq!(
        config.pending_requester_decisions(SESSION).unwrap().len(),
        1
    );
    let done = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, None))
        .unwrap()
        .unwrap();
    assert!(
        config
            .pending_requester_decisions(SESSION)
            .unwrap()
            .is_empty()
    );
    let again = config
        .ingest_requester_prompt(SESSION, &host("UserPromptSubmit", &raw, None))
        .unwrap()
        .unwrap();
    assert_eq!(done.reference, again.reference);
}

#[test]
fn consumption_is_idempotent_and_divergent_consumption_conflicts() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let receipt = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "c1"), Some("e")),
        )
        .unwrap()
        .unwrap();
    config
        .consume_requester_challenge(&receipt.reference, "grant-a")
        .unwrap();
    config
        .consume_requester_challenge(&receipt.reference, "grant-a")
        .unwrap();
    let error = config
        .consume_requester_challenge(&receipt.reference, "grant-b")
        .unwrap_err();
    assert_eq!(code(&error), "requester_decision_conflict");
    let other = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("reject", "c1"), Some("e2")),
        )
        .unwrap_err();
    assert_eq!(code(&other), "requester_decision_conflict");
}

#[test]
fn finished_session_allows_history_reads_but_no_new_challenge_or_receipt() {
    let (dir, config) = store();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let receipt = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "c1"), Some("e")),
        )
        .unwrap()
        .unwrap();
    config
        .register_requester_challenge(&request(dir.path(), "c2"))
        .unwrap();
    config
        .finish_workflow(SESSION, vec![], vec![], None)
        .unwrap();
    assert_eq!(
        config.read_requester_decision(&receipt.reference).unwrap(),
        receipt
    );
    assert!(matches!(
        config
            .register_requester_challenge(&request(dir.path(), "c3"))
            .unwrap_err(),
        SessionError::WorkspaceFinished { .. }
    ));
    let error = config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "c2"), Some("f")),
        )
        .unwrap_err();
    assert!(
        matches!(error, SessionError::WorkspaceFinished { .. }),
        "{error:?}"
    );
}

fn decision_dir(config: &SessionStoreConfig) -> std::path::PathBuf {
    config
        .root
        .join("sessions")
        .join(SESSION)
        .join("requester/c1/decision")
}
fn ingest_c1(
    config: &SessionStoreConfig,
    id: Option<&str>,
) -> session_api::RequesterDecisionReceipt {
    config
        .ingest_requester_prompt(
            SESSION,
            &host("UserPromptSubmit", &line("approve", "c1"), id),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn host_ingress_is_not_constructed_by_any_agent_surface() {
    fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.ends_with("bin/session-capture-hook") {
                    continue;
                }
                scan(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs")
                && std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("HostPromptIngress")
            {
                hits.push(path.display().to_string());
            }
        }
    }
    let mut hits = Vec::new();
    scan(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src"),
        &mut hits,
    );
    assert!(
        hits.is_empty(),
        "agent/CLI/MCP surfaces must not mint host ingress: {hits:?}"
    );
}

#[path = "requester_decision/receipt_recovery.rs"]
mod receipt_recovery;
#[allow(unused_imports)]
pub use receipt_recovery::*;

//! Integrity negatives: schema/identity drift, symlinks and wrong-store receipts fail closed.
use std::path::{Path, PathBuf};

use session_api::{
    HostPromptIngress, RequesterChallengeRequest, RequesterDecisionRef, SessionRuntimeInitRequest,
    SessionStoreConfig,
};
use uuid::Uuid;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const MISSION: &str = "22222222-2222-4222-8222-222222222222";
const P: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const R: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn request(dir: &Path, challenge: &str) -> RequesterChallengeRequest {
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
fn line(challenge: &str) -> String {
    format!("mission-decision approve {MISSION} 3 {P} {R} {challenge}")
}
fn setup() -> (tempfile::TempDir, SessionStoreConfig, RequesterDecisionRef) {
    let dir = tempfile::tempdir().unwrap();
    let config = SessionStoreConfig::new(dir.path().join("session-store"));
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(SESSION.into()),
            predecessor_run_id: None,
            force_new_run: false,
        })
        .unwrap();
    config
        .register_requester_challenge(&request(dir.path(), "c1"))
        .unwrap();
    let ingress = HostPromptIngress::trusted_local_hook(
        "UserPromptSubmit",
        line("c1").into_bytes(),
        Some("e1".into()),
    );
    let receipt = config
        .ingest_requester_prompt(SESSION, &ingress)
        .unwrap()
        .unwrap();
    (dir, config, receipt.reference)
}
fn requester_dir(config: &SessionStoreConfig) -> PathBuf {
    config.root.join("sessions").join(SESSION).join("requester")
}
fn decision_dir(config: &SessionStoreConfig) -> PathBuf {
    requester_dir(config).join("c1/decision")
}
fn edit(path: &Path, change: impl FnOnce(&mut serde_json::Value)) {
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    change(&mut value);
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}
fn corrupt(config: &SessionStoreConfig, reference: &RequesterDecisionRef) {
    let error = config.read_requester_decision(reference).unwrap_err();
    assert_eq!(
        error.requester_code(),
        Some("requester_decision_corrupt"),
        "{error:?}"
    );
}

#[test]
fn intent_identity_binding_and_schema_drift_fail_closed() {
    type Mutation = (&'static str, &'static str, serde_json::Value);
    let cases: Vec<Mutation> = vec![
        ("intent.json", "schema_version", 2.into()),
        (
            "intent.json",
            "session_id",
            "33333333-3333-4333-8333-333333333333".into(),
        ),
        ("intent.json", "challenge_id", "c9".into()),
        ("intent.json", "revision", 9.into()),
        ("intent.json", "plan_digest", R.into()),
        ("intent.json", "review_snapshot_digest", P.into()),
        (
            "intent.json",
            "mission_id",
            "33333333-3333-4333-8333-333333333333".into(),
        ),
        ("intent.json", "host_event_id_supplied", false.into()),
        ("source_event.json", "schema_version", 2.into()),
    ];
    for (file, key, value) in cases {
        let (_dir, config, reference) = setup();
        edit(&decision_dir(&config).join(file), |v| {
            v[key] = value.clone()
        });
        corrupt(&config, &reference);
    }
}

#[test]
fn unknown_fields_in_stored_records_are_rejected() {
    for file in ["intent.json", "source_event.json", "receipt.json"] {
        let (_dir, config, reference) = setup();
        edit(&decision_dir(&config).join(file), |v| {
            v["extra_authority"] = true.into()
        });
        assert!(
            config.read_requester_decision(&reference).is_err(),
            "{file}"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlinked_record_or_ancestor_never_resolves() {
    use std::os::unix::fs::symlink;
    let (dir, config, reference) = setup();
    config
        .register_requester_challenge(&request(dir.path(), "c2"))
        .unwrap();
    let source = decision_dir(&config).join("source_event.json");
    let copy = dir.path().join("source-copy.json");
    std::fs::copy(&source, &copy).unwrap();
    std::fs::remove_file(&source).unwrap();
    symlink(&copy, &source).unwrap();
    assert!(config.read_requester_decision(&reference).is_err());
    std::fs::remove_file(&source).unwrap();
    std::fs::copy(&copy, &source).unwrap();
    config.read_requester_decision(&reference).unwrap();

    let base = requester_dir(&config);
    std::fs::rename(base.join("c1"), base.join("moved")).unwrap();
    symlink(base.join("moved"), base.join("c1")).unwrap();
    assert!(config.read_requester_decision(&reference).is_err());
    assert!(
        config
            .find_requester_decision_for_challenge(SESSION, "c1")
            .is_err()
    );
    assert!(config.pending_requester_decisions(SESSION).is_err());
    let ingress = HostPromptIngress::trusted_local_hook(
        "UserPromptSubmit",
        line("c2").into_bytes(),
        Some("x".into()),
    );
    assert!(config.ingest_requester_prompt(SESSION, &ingress).is_err());
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn receipt_copied_to_another_store_root_is_not_accepted() {
    let (dir, config, reference) = setup();
    let copied = dir.path().join("copied-store");
    copy_dir(&config.root, &copied);
    let other = SessionStoreConfig::new(&copied);
    let error = other.read_requester_challenge(SESSION, "c1").unwrap_err();
    assert_eq!(error.requester_code(), Some("requester_decision_corrupt"));
    assert!(other.read_requester_decision(&reference).is_err());
    config.read_requester_decision(&reference).unwrap();
}

#[test]
fn forged_reference_is_unknown_never_pending() {
    let (_dir, config, reference) = setup();
    let forged = RequesterDecisionRef {
        decision_id: "forged".into(),
        source_event_id: "forged".into(),
        ..reference.clone()
    };
    let error = config.read_requester_decision(&forged).unwrap_err();
    assert_eq!(
        error.requester_code(),
        Some("requester_decision_unknown_reference"),
        "{error:?}"
    );
    config.read_requester_decision(&reference).unwrap();
}

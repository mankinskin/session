use session_api::{
    SessionAuditSelector,
    SessionStoreConfig,
};

fn main() {
    let tmp = std::env::temp_dir()
        .join(format!("session-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let store = SessionStoreConfig::new(tmp.clone(), "smoke-workspace");

    let transcript_path = std::path::Path::new(
        "c:/Users/linus/git/graph_app/context-engine/.session/sessions/41966513-a8fa-4b44-98fa-9c57f0437cc0/events.json",
    );
    // events.json here is our own persisted format, not a raw copilot transcript;
    // instead synthesize a minimal capture using the transcript parser test fixture path.
    println!("Skipping direct transcript ingestion in this smoke harness.");
    let _ = transcript_path;

    // Fallback smoke path: capture a tiny synthetic transcript via capture_copilot_transcript
    let jsonl = std::path::Path::new(
        "c:/Users/linus/git/graph_app/context-engine/tmp/mini.jsonl",
    )
    .to_path_buf();

    let plan = store.capture_copilot_transcript(&jsonl, "smoke").unwrap();
    println!("captured session_id={}", plan.record.session_id);

    let metrics_path = tmp
        .join("sessions")
        .join(&plan.record.session_id)
        .join("tool-metrics.json");
    let metrics_raw = std::fs::read_to_string(&metrics_path).unwrap();
    println!(
        "tool-metrics.json bytes={} content={}",
        metrics_raw.len(),
        metrics_raw
    );

    let report = store
        .delegation_cost_report(SessionAuditSelector::SessionId(
            plan.record.session_id.clone(),
        ))
        .unwrap();
    println!(
        "delegation report: {}",
        serde_json::to_string_pretty(&report).unwrap()
    );

    std::fs::remove_dir_all(&tmp).ok();
}

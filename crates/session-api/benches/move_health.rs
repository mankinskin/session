//! Move-health Criterion benchmarks for
//! [`session_api::move_domain::SessionMoveDomain`].
//!
//! Coverage: entity count and phase separation (preflight_only /
//! apply_only / preflight_plus_apply / rollback / resume).
//!
//! Coverage limitation (structural, not a gap in this benchmark): sessions
//! have no edge model. `SessionMoveDomain::related_entities` always returns
//! an empty [`MoveReferences`], so there is no link-density dimension to
//! benchmark for this domain — recorded here in the module doc rather than
//! as a benchmark, per the domain's actual capability.
//!
//! Coverage limitation (resume): resume is benchmarked as an idempotent
//! re-resume of an already-`Validated` journal, named
//! `session_move_phase_resume_idempotent_proxy`. The public API
//! (`plan_move_preflight` / `execute_move_with_journal` /
//! `resume_move_with_journal` / `rollback_move_with_journal`) has no way to
//! force an interrupted move, so a genuinely-interrupted resume path cannot
//! be exercised here.
//!
//! Store-size comparison is not included: `SessionMoveDomain::scan_store` is
//! a no-op, so apply cost has no reconciliation component that scales with
//! total store size the way `SpecMoveDomain`'s does.

use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::Command,
};

use chrono::Utc;
use criterion::{
    BatchSize,
    Criterion,
    criterion_group,
    criterion_main,
};
use session_api::{
    CopilotHookMessage,
    CopilotHookPayload,
    SessionCaptureRequest,
    SessionRole,
    SessionStoreConfig,
};
use tempfile::TempDir;
use uuid::Uuid;

const SESSION_INDEX_DIR: &str = ".session";
const WORKSPACE_SLUG: &str = "bench-workspace";

fn git_init(repo_root: &Path) {
    let status = Command::new("git")
        .current_dir(repo_root)
        .arg("init")
        .status()
        .expect("run git init");
    assert!(status.success(), "git init failed");
}

fn sample_request(session_id: &Uuid) -> SessionCaptureRequest {
    SessionCaptureRequest::copilot(CopilotHookPayload {
        session_id: session_id.to_string(),
        workspace_slug: WORKSPACE_SLUG.to_string(),
        captured_at: Utc::now(),
        conversation_id: Some("bench-conversation".to_string()),
        agent_id: Some("github-copilot".to_string()),
        model: Some("bench-model".to_string()),
        trigger: Some("bench".to_string()),
        provisioning: None,
        messages: vec![CopilotHookMessage {
            role: SessionRole::User,
            content: "bench move session".to_string(),
            tool_name: None,
            captured_at: None,
            event_meta: None,
        }],
        events: vec![],
        runtime: None,
    })
}

/// One isolated source+target workspace pair with `entity_count` persisted
/// sessions in the source store.
fn build_session_fixture(
    entity_count: usize
) -> (TempDir, SessionStoreConfig, PathBuf, Vec<Uuid>) {
    let workspace_dir = tempfile::tempdir().expect("tempdir");
    let repo = workspace_dir.path().join("repo");
    fs::create_dir_all(&repo).expect("create repo dir");
    git_init(&repo);

    let source_workspace = repo.join("source");
    let target_workspace = repo.join("target");
    fs::create_dir_all(source_workspace.join(SESSION_INDEX_DIR))
        .expect("create source .session dir");
    fs::create_dir_all(target_workspace.join(SESSION_INDEX_DIR))
        .expect("create target .session dir");

    let store = SessionStoreConfig::new(
        source_workspace.join(SESSION_INDEX_DIR),
        WORKSPACE_SLUG,
    );

    let ids: Vec<Uuid> = (0..entity_count)
        .map(|_| {
            let session_id = Uuid::new_v4();
            store
                .persist_capture(sample_request(&session_id))
                .expect("persist capture");
            session_id
        })
        .collect();

    (workspace_dir, store, target_workspace, ids)
}

// --- Entity count ---

fn bench_session_move_preflight_by_entity_count(c: &mut Criterion) {
    for &entity_count in &[10usize, 50, 200] {
        let (_workspace_dir, store, target_workspace, ids) =
            build_session_fixture(entity_count);
        let id = ids[0];
        c.bench_function(
            &format!("session_move_preflight_{entity_count}entities"),
            |b| {
                b.iter(|| {
                    let plan = store
                        .plan_move_preflight(&id, &target_workspace)
                        .expect("plan preflight");
                    criterion::black_box(plan);
                });
            },
        );
    }
}

// --- Phase separation ---

fn bench_session_move_preflight_only(c: &mut Criterion) {
    c.bench_function("session_move_phase_preflight_only", |b| {
        b.iter_batched(
            || build_session_fixture(1),
            |(_workspace_dir, store, target_workspace, ids)| {
                let plan = store
                    .plan_move_preflight(&ids[0], &target_workspace)
                    .expect("plan preflight");
                criterion::black_box(plan);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_session_move_apply_only(c: &mut Criterion) {
    c.bench_function("session_move_phase_apply_only", |b| {
        b.iter_batched(
            || {
                let (workspace_dir, store, target_workspace, ids) =
                    build_session_fixture(1);
                let plan = store
                    .plan_move_preflight(&ids[0], &target_workspace)
                    .expect("plan preflight");
                assert!(
                    plan.supported(),
                    "unexpected move blockers: {:?}",
                    plan.blockers
                );
                (workspace_dir, store, plan)
            },
            |(_workspace_dir, store, plan)| {
                let outcome = store
                    .execute_move_with_journal(&plan)
                    .expect("execute move");
                criterion::black_box(outcome);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_session_move_preflight_plus_apply(c: &mut Criterion) {
    c.bench_function("session_move_phase_preflight_plus_apply", |b| {
        b.iter_batched(
            || build_session_fixture(1),
            |(_workspace_dir, store, target_workspace, ids)| {
                let plan = store
                    .plan_move_preflight(&ids[0], &target_workspace)
                    .expect("plan preflight");
                assert!(
                    plan.supported(),
                    "unexpected move blockers: {:?}",
                    plan.blockers
                );
                let outcome = store
                    .execute_move_with_journal(&plan)
                    .expect("execute move");
                criterion::black_box(outcome);
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_session_move_rollback(c: &mut Criterion) {
    c.bench_function("session_move_phase_rollback", |b| {
        b.iter_batched(
            || {
                let (workspace_dir, store, target_workspace, ids) =
                    build_session_fixture(1);
                let plan = store
                    .plan_move_preflight(&ids[0], &target_workspace)
                    .expect("plan preflight");
                assert!(
                    plan.supported(),
                    "unexpected move blockers: {:?}",
                    plan.blockers
                );
                let outcome = store
                    .execute_move_with_journal(&plan)
                    .expect("execute move");
                (workspace_dir, store, outcome.journal.id)
            },
            |(_workspace_dir, store, journal_id)| {
                let outcome = store
                    .rollback_move_with_journal(journal_id)
                    .expect("rollback move");
                assert!(outcome.rolled_back);
                criterion::black_box(outcome);
            },
            BatchSize::SmallInput,
        );
    });
}

/// Coverage limitation: this benchmarks `resume_move_with_journal` called on
/// an already-`Validated` journal (an idempotent re-resume), since the
/// public move API cannot synthesize a genuinely-interrupted move. See the
/// module doc comment.
fn bench_session_move_resume_idempotent_proxy(c: &mut Criterion) {
    c.bench_function("session_move_phase_resume_idempotent_proxy", |b| {
        b.iter_batched(
            || {
                let (workspace_dir, store, target_workspace, ids) =
                    build_session_fixture(1);
                let plan = store
                    .plan_move_preflight(&ids[0], &target_workspace)
                    .expect("plan preflight");
                assert!(
                    plan.supported(),
                    "unexpected move blockers: {:?}",
                    plan.blockers
                );
                let outcome = store
                    .execute_move_with_journal(&plan)
                    .expect("execute move");
                (workspace_dir, store, outcome.journal.id)
            },
            |(_workspace_dir, store, journal_id)| {
                let outcome = store
                    .resume_move_with_journal(journal_id)
                    .expect("resume move");
                criterion::black_box(outcome);
            },
            BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    move_health,
    bench_session_move_preflight_by_entity_count,
    bench_session_move_preflight_only,
    bench_session_move_apply_only,
    bench_session_move_preflight_plus_apply,
    bench_session_move_rollback,
    bench_session_move_resume_idempotent_proxy,
);
criterion_main!(move_health);

mod intake_contract {
    use super::*;
    use uuid::Uuid;

    const SESSION: &str = "11111111-1111-4111-8111-111111111111";
    const MISSION_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const MISSION_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const ABC_DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[cfg(unix)]
    #[test]
    fn symlinked_or_dotdot_session_roots_are_rejected_without_writing() {
        let dir = tempfile::TempDir::new().unwrap();
        let _real = store(&dir);
        let real_root = dir.path().join("store");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real_root, &link).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(dir.path(), &alias).unwrap();
        for root in [
            link,
            alias.join("store"),
            real_root.join("..").join("store"),
        ] {
            let config = SessionStoreConfig::new(&root);
            let error = config.submit_intake(SESSION, "r", b"abc").unwrap_err();
            assert!(
                matches!(error, crate::SessionError::InvalidStorePath(_)),
                "{root:?}: {error:?}"
            );
            let error = config.read_intake(SESSION, "r").unwrap_err();
            assert!(
                matches!(error, crate::SessionError::InvalidStorePath(_)),
                "{root:?}: {error:?}"
            );
        }
        assert!(
            !real_root
                .join("sessions")
                .join(SESSION)
                .join("intake")
                .exists()
        );
    }

    #[test]
    fn operation_excludes_finish_while_held_and_rejects_reserved_replay_after_finish() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let root = dir.path().join("ws");
        let mission_a = Uuid::parse_str(MISSION_A).unwrap();
        let finish_marker = config
            .root
            .join("sessions")
            .join(SESSION)
            .join("finish.json");
        {
            let mut op = config.begin_intake_operation(SESSION, "r", b"abc").unwrap();
            op.reserve_mission(mission_a, &root).unwrap();
            assert!(
                config
                    .finish_workflow(SESSION, vec![], vec![], None)
                    .is_err()
            );
            assert!(!finish_marker.exists());
            op.complete_link(mission_a).unwrap();
        }
        {
            let mut op = config
                .begin_intake_operation(SESSION, "half", b"xyz")
                .unwrap();
            op.reserve_mission(Uuid::parse_str(MISSION_B).unwrap(), &root)
                .unwrap();
        }
        config
            .finish_workflow(SESSION, vec![], vec![], None)
            .unwrap();
        let error = config
            .begin_intake_operation(SESSION, "half", b"xyz")
            .err()
            .unwrap();
        assert!(
            matches!(error, crate::SessionError::WorkspaceFinished { .. }),
            "{error:?}"
        );
        let op = config.begin_intake_operation(SESSION, "r", b"abc").unwrap();
        assert_eq!(op.intake().linkage.as_ref().unwrap().mission_id, mission_a);
        drop(op);
        let error = config
            .begin_intake_operation(SESSION, "r", b"zzz")
            .err()
            .unwrap();
        assert!(
            matches!(error, crate::SessionError::IntakeIdempotencyConflict { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn half_written_input_is_resumed_only_with_identical_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let intake_dir = config.root.join("sessions").join(SESSION).join("intake");
        for (request, planted) in [("same", &b"abc"[..]), ("other", &b"abd"[..])] {
            std::fs::create_dir_all(intake_dir.join(request)).unwrap();
            std::fs::write(intake_dir.join(request).join("input.bin"), planted).unwrap();
        }
        let resumed = config.submit_intake(SESSION, "same", b"abc").unwrap();
        assert_eq!(resumed.record.input_digest, ABC_DIGEST);
        let error = config.submit_intake(SESSION, "other", b"abc").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeIdempotencyConflict { .. }),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(intake_dir.join("other").join("input.bin")).unwrap(),
            b"abd"
        );
    }

    fn store(dir: &tempfile::TempDir) -> SessionStoreConfig {
        let config = SessionStoreConfig::new(dir.path().join("store"));
        config
            .init_runtime_context(crate::SessionRuntimeInitRequest {
                session_id: Some(SESSION.to_string()),
                predecessor_run_id: None,
                force_new_run: false,
            })
            .unwrap();
        config
    }

    #[test]
    fn submit_persists_exact_bytes_and_digest_and_replays_identically() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let bytes: &[u8] = b"\xff\x00 non-utf8 \r\n trailing\r\n";
        let first = config.submit_intake(SESSION, "r-42", bytes).unwrap();
        assert!(first.newly_submitted);
        assert_eq!(std::fs::read(&first.input_path).unwrap(), bytes);
        assert_eq!(first.record.byte_len, bytes.len() as u64);
        let abc = config.submit_intake(SESSION, "r-abc", b"abc").unwrap();
        assert_eq!(abc.record.input_digest, ABC_DIGEST);
        let restarted = SessionStoreConfig::new(dir.path().join("store"));
        let replay = restarted.submit_intake(SESSION, "r-42", bytes).unwrap();
        assert!(!replay.newly_submitted);
        assert_eq!(replay.record, first.record);
        assert_eq!(replay.input_path, first.input_path);
        assert_eq!(
            restarted.read_intake(SESSION, "r-42").unwrap().record,
            first.record
        );
    }

    #[test]
    fn same_request_with_different_bytes_conflicts_and_keeps_original() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let first = config.submit_intake(SESSION, "r-1", b"one").unwrap();
        let error = config.submit_intake(SESSION, "r-1", b"two").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeIdempotencyConflict { .. }),
            "{error:?}"
        );
        assert_eq!(std::fs::read(&first.input_path).unwrap(), b"one");
        assert_eq!(
            config.read_intake(SESSION, "r-1").unwrap().record,
            first.record
        );
    }

    #[test]
    fn distinct_requests_are_independent_append_only_inputs() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let a = config.submit_intake(SESSION, "r-a", b"a").unwrap();
        let b = config.submit_intake(SESSION, "r-b", b"b").unwrap();
        assert_ne!(a.input_path, b.input_path);
        assert_eq!(std::fs::read(&a.input_path).unwrap(), b"a");
        assert_eq!(std::fs::read(&b.input_path).unwrap(), b"b");
    }

    #[test]
    fn invalid_request_ids_and_unknown_sessions_are_structured_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        for bad in ["", "../x", "a/b", ".hidden", "a b", &"x".repeat(129)] {
            let error = config.submit_intake(SESSION, bad, b"x").unwrap_err();
            assert!(
                matches!(error, crate::SessionError::InvalidIntakeRequestId(_)),
                "{bad:?}: {error:?}"
            );
        }
        let other = "22222222-2222-4222-8222-222222222222";
        let error = config.submit_intake(other, "r", b"x").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeSessionNotFound { .. }),
            "{error:?}"
        );
        assert!(
            !config
                .root
                .join("sessions")
                .join(other)
                .join("intake")
                .exists()
        );
    }

    #[test]
    fn missing_intake_is_distinct_from_missing_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let error = config.read_intake(SESSION, "never").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeMissing { .. }),
            "{error:?}"
        );
        let error = config
            .read_intake("22222222-2222-4222-8222-222222222222", "never")
            .unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeSessionNotFound { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn linkage_is_journaled_reserved_then_linked_and_survives_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        config.submit_intake(SESSION, "r", b"abc").unwrap();
        let root = dir.path().join("ws");
        let mission_a = Uuid::parse_str(MISSION_A).unwrap();
        let mission_b = Uuid::parse_str(MISSION_B).unwrap();
        let reserved = config
            .reserve_intake_mission(SESSION, "r", mission_a, &root)
            .unwrap();
        let link = reserved.linkage.unwrap();
        assert_eq!(link.mission_id, mission_a);
        assert_eq!(link.state, crate::SessionIntakeLinkState::Reserved);
        config
            .reserve_intake_mission(SESSION, "r", mission_a, &root)
            .unwrap();
        let error = config
            .reserve_intake_mission(SESSION, "r", mission_b, &root)
            .unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeLinkageConflict { .. }),
            "{error:?}"
        );
        let other_root = dir.path().join("other-ws");
        let error = config
            .reserve_intake_mission(SESSION, "r", mission_a, &other_root)
            .unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeLinkageConflict { .. }),
            "{error:?}"
        );
        let error = config
            .complete_intake_link(SESSION, "r", mission_b)
            .unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeLinkageConflict { .. }),
            "{error:?}"
        );
        let restarted = SessionStoreConfig::new(dir.path().join("store"));
        assert_eq!(
            restarted
                .read_intake(SESSION, "r")
                .unwrap()
                .linkage
                .unwrap()
                .state,
            crate::SessionIntakeLinkState::Reserved
        );
        let linked = restarted
            .complete_intake_link(SESSION, "r", mission_a)
            .unwrap();
        assert_eq!(
            linked.linkage.as_ref().unwrap().state,
            crate::SessionIntakeLinkState::Linked
        );
        let again = restarted
            .complete_intake_link(SESSION, "r", mission_a)
            .unwrap();
        assert_eq!(again.linkage, linked.linkage);
    }

    #[test]
    fn completion_without_reservation_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        config.submit_intake(SESSION, "r", b"abc").unwrap();
        let error = config
            .complete_intake_link(SESSION, "r", Uuid::parse_str(MISSION_A).unwrap())
            .unwrap_err();
        assert!(
            matches!(error, crate::SessionError::IntakeLinkageConflict { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn finished_session_rejects_new_intake_and_unfinished_linkage_but_replays_linked() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let root = dir.path().join("ws");
        let mission_a = Uuid::parse_str(MISSION_A).unwrap();
        config.submit_intake(SESSION, "done", b"abc").unwrap();
        config
            .reserve_intake_mission(SESSION, "done", mission_a, &root)
            .unwrap();
        config
            .complete_intake_link(SESSION, "done", mission_a)
            .unwrap();
        config.submit_intake(SESSION, "half", b"xyz").unwrap();
        config
            .finish_workflow(SESSION, vec![], vec![], None)
            .unwrap();
        let error = config.submit_intake(SESSION, "new", b"abc").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::WorkspaceFinished { .. }),
            "{error:?}"
        );
        assert!(
            !config
                .root
                .join("sessions")
                .join(SESSION)
                .join("intake")
                .join("new")
                .exists()
        );
        let replay = config.submit_intake(SESSION, "done", b"abc").unwrap();
        assert_eq!(replay.linkage.unwrap().mission_id, mission_a);
        assert!(config.submit_intake(SESSION, "done", b"zzz").is_err());
        let error = config
            .reserve_intake_mission(SESSION, "half", Uuid::parse_str(MISSION_B).unwrap(), &root)
            .unwrap_err();
        assert!(
            matches!(error, crate::SessionError::WorkspaceFinished { .. }),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_or_unsupported_finish_marker_blocks_intake_before_any_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = store(&dir);
        let session_dir = config.root.join("sessions").join(SESSION);
        let marker = session_dir.join("finish.json");
        std::os::unix::fs::symlink(dir.path().join("missing-target"), &marker).unwrap();
        let error = config.submit_intake(SESSION, "r", b"abc").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::InvalidStorePath(_)),
            "{error:?}"
        );
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        let error = config.submit_intake(SESSION, "r", b"abc").unwrap_err();
        assert!(
            matches!(error, crate::SessionError::InvalidStorePath(_)),
            "{error:?}"
        );
        assert!(!session_dir.join("intake").exists());
    }
}

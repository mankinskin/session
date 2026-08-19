use crate::{
    SessionTerminalCreateRequest,
    SessionTerminalStatus,
};

#[test]
fn terminal_observer_persists_bounded_output_and_rejects_append_after_close() {
    let tempdir = TempDir::new().unwrap();
    let config = SessionStoreConfig::new(tempdir.path().join("store"), "context-engine");
    let session_id = uuid::Uuid::new_v4().to_string();
    config
        .init_runtime_context(SessionRuntimeInitRequest {
            session_id: Some(session_id.clone()),
            ..Default::default()
        })
        .unwrap();

    let terminal = config
        .create_terminal_observer(SessionTerminalCreateRequest {
            session_id: session_id.clone(),
            label: "human terminal".to_string(),
            cwd: Some(tempdir.path().to_path_buf()),
        })
        .unwrap();
    config
        .append_terminal_output(&session_id, &terminal.terminal_id, "first\n".to_string())
        .unwrap();
    config
        .append_terminal_output(&session_id, &terminal.terminal_id, "second\n".to_string())
        .unwrap();

    let peek = config
        .peek_terminal_output(&session_id, &terminal.terminal_id, 0, 1)
        .unwrap();
    assert_eq!(peek.events.len(), 1);
    assert_eq!(peek.events[0].output, "first\n");
    assert!(peek.has_more);

    let closed = config
        .close_terminal_observer(&session_id, &terminal.terminal_id)
        .unwrap();
    assert_eq!(closed.status, SessionTerminalStatus::Closed);
    assert!(matches!(
        config.append_terminal_output(&session_id, &terminal.terminal_id, "late\n".to_string()),
        Err(SessionError::TerminalClosed { .. })
    ));
}
use ascurra_harness::{
    domain::{AppState, RunState},
    storage::Store,
};

#[test]
fn deleting_chat_removes_tab_and_persisted_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().into()).unwrap();
    let mut state = AppState::default();
    let project = state.add_project("Test".into(), ".".into());
    let first = state.selected_chat.unwrap();
    let second = state.add_chat(project).unwrap();
    store.save(&state).unwrap();
    state.delete_chat(second).unwrap();
    assert_eq!(state.selected_chat, Some(first));
    assert!(!state.open_tabs.contains(&second));
    assert!(state.chat(second).is_none());
    store.save(&state).unwrap();
    assert!(store.load().unwrap().chat(second).is_none());
    assert_eq!(
        std::fs::read_dir(dir.path().join("histories"))
            .unwrap()
            .count(),
        1
    );
    state.delete_chat(first).unwrap();
    assert_eq!(state.selected_chat, None);
    assert!(state.open_tabs.is_empty());
    assert!(state.delete_chat(first).is_err());
}

#[test]
fn active_chats_cannot_be_deleted_and_background_deletion_keeps_selection() {
    let mut state = AppState::default();
    let project = state.add_project("Test".into(), ".".into());
    let first = state.selected_chat.unwrap();
    let second = state.add_chat(project).unwrap();
    for status in [
        RunState::Running,
        RunState::WaitingForInput,
        RunState::WaitingForPermission,
    ] {
        state.chat_mut(first).unwrap().state = status;
        assert!(state.delete_chat(first).is_err());
        assert!(state.chat(first).is_some());
        assert!(state.open_tabs.contains(&first));
    }
    state.chat_mut(first).unwrap().state = RunState::Idle;
    state.delete_chat(first).unwrap();
    assert_eq!(state.selected_chat, Some(second));
}

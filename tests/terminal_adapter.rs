#[cfg(windows)]
#[test]
fn conpty_is_interactive_scoped_resizable_and_owned() {
    use ascurra_harness::terminal::Session;
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let session = Session::start(root.path(), "cmd.exe", &["/Q"], 24, 100).unwrap();
    session
        .write(b"set \"HARNESS_PTY=pty_verified\"\r")
        .unwrap();
    session.write(b"echo [%HARNESS_PTY%]\r").unwrap();
    session
        .write(b"echo scoped>terminal-fixture.txt\r")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if session.text().contains("[pty_verified]")
            && root.path().join("terminal-fixture.txt").exists()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PTY did not respond: {}",
            session.text()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        std::fs::read_to_string(root.path().join("terminal-fixture.txt"))
            .unwrap()
            .contains("scoped")
    );
    session.resize(30, 120).unwrap();
    assert_eq!(session.parser.lock().unwrap().screen().size(), (30, 120));
    session.scroll(500);
    session.scroll(-500);
    assert_eq!(session.parser.lock().unwrap().screen().scrollback(), 0);
    session.stop();
    assert!(session.write(b"echo should_not_run\r").is_err());
    drop(session);
}

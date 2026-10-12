use ascurra_harness::logcat::{self, StreamEvent};
use std::{path::Path, time::Duration};
use tokio::sync::{mpsc, watch};
fn fixture(root: &Path) -> std::path::PathBuf {
    let executable = root.join(if cfg!(windows) {
        "adb-fixture.exe"
    } else {
        "adb-fixture"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/adb_fixture.rs");
    let rustc = std::env::var_os("RUSTC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("CARGO_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(
                        std::env::var_os("USERPROFILE")
                            .or_else(|| std::env::var_os("HOME"))
                            .unwrap(),
                    )
                    .join(".cargo")
                })
                .join(if cfg!(windows) {
                    "bin/rustc.exe"
                } else {
                    "bin/rustc"
                })
        });
    let output = std::process::Command::new(rustc)
        .arg(source)
        .args(["--edition", "2024", "-o"])
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}
#[test]
fn adb_stream_is_live_reports_disconnect_and_stops_under_backpressure() {
    let temp = tempfile::tempdir().unwrap();
    let binary = fixture(temp.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let devices = logcat::devices(&binary).await.unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices[0].available);
        assert!(!devices[1].available);
        let (cancel, stop) = watch::channel(false);
        let (tx, mut rx) = mpsc::channel(2);
        let task = tokio::spawn(logcat::stream(
            binary.clone(),
            "emulator-5554".into(),
            stop,
            tx,
        ));
        let mut batches = Vec::new();
        for _ in 0..2 {
            let event = tokio::time::timeout(Duration::from_secs(3), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let StreamEvent::Lines(lines) = event else {
                panic!("Expected live lines");
            };
            assert!(!lines.is_empty());
            assert!(lines[0].text.contains("línea α"));
            batches.push(lines);
        }
        assert_ne!(
            batches[0].last().unwrap().text,
            batches[1].last().unwrap().text,
            "New messages must arrive while the process remains active"
        );
        cancel.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();

        let (_cancel, stop) = watch::channel(false);
        let (tx, mut rx) = mpsc::channel(2);
        let task = tokio::spawn(logcat::stream(binary.clone(), "missing".into(), stop, tx));
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        match event {
            StreamEvent::End(Err(error)) => assert!(error.contains("device disconnected")),
            _ => panic!("Disconnect must be reported"),
        }
        task.await.unwrap();

        let (cancel, stop) = watch::channel(false);
        let (tx, _rx) = mpsc::channel(2);
        let task = tokio::spawn(logcat::stream(binary, "flood".into(), stop, tx));
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    });
}

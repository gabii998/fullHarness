use ascurra_harness::lint::{self, Kind, Tool};
use std::path::Path;
fn fixture(root: &Path) -> std::path::PathBuf {
    let executable = root.join(if cfg!(windows) {
        "lint-fixture.exe"
    } else {
        "lint-fixture"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/lint_fixture.rs");
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
fn lint_exit_one_is_a_report_and_logs_persist_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let binary = fixture(temp.path());
    let root = temp.path().canonicalize().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let tool = Tool {
            kind: Kind::Text,
            program: binary.clone(),
            args: vec![],
            name: "Fixture".into(),
            reports: false,
        };
        let logs = root.join("logs");
        let report = lint::run(&root, &tool, &logs, receiver).await;
        assert_eq!(report.problems.len(), 2);
        assert!(report.status.contains("1 errores"));
        assert_eq!(
            lint::cached(&logs, &root).unwrap().problems,
            report.problems
        );
        assert!(lint::folder(&logs, &root).join("stderr.log").exists());
        drop(sender);
        let mut tool = tool;
        tool.args = vec!["wait".into()];
        let (sender, receiver) = tokio::sync::watch::channel(false);
        let job = lint::run(&root, &tool, &logs, receiver);
        let stop = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            sender.send(true).unwrap();
        };
        let (report, _) = tokio::join!(job, stop);
        assert!(report.status.contains("cancelada"));
        assert!(report.problems.is_empty());
    });
}

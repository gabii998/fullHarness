use ascurra_harness::docker::Client;
use std::path::Path;
fn fixture(root: &Path) -> std::path::PathBuf {
    let executable = root.join(if cfg!(windows) {
        "docker-fixture.exe"
    } else {
        "docker-fixture"
    });
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/docker_fixture.rs");
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
fn project_scoping_is_rechecked_before_actions_and_logs_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let binary = fixture(temp.path());
    let root = temp.path().canonicalize().unwrap();
    std::fs::write(root.join("compose.yaml"), "services: {}\n").unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let client = Client::with_binary(binary);
        let snapshot = client.snapshot(&root).await.unwrap();
        assert_eq!(snapshot.containers.len(), 1);
        assert_eq!(snapshot.containers[0].service, "db");
        let owned = "a".repeat(64);
        let other = "b".repeat(64);
        assert!(client.action(&root, &other, "stop").await.is_err());
        assert!(!root.join("mutated").exists());
        assert!(client.action(&root, &owned, "unknown").await.is_err());
        client.action(&root, &owned, "start").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("mutated")).unwrap(),
            "start"
        );
        client.action(&root, &owned, "stop").await.unwrap();
        client.action(&root, &owned, "restart").await.unwrap();
        let logs = client.action(&root, &owned, "logs").await.unwrap();
        assert!(logs.contains("stdout log") && logs.contains("stderr log"));
        std::fs::write(root.join("moved"), "").unwrap();
        assert!(client.action(&root, &owned, "stop").await.is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("mutated")).unwrap(),
            "restart"
        );
        client.up(&root).await.unwrap();
        assert!(root.join("compose-up").exists());
        std::fs::write(root.join("compose.override.yaml"), "services: {}\n").unwrap();
        client.up(&root).await.unwrap();
        assert!(
            std::fs::read_to_string(root.join("compose-up"))
                .unwrap()
                .contains("compose.override.yaml")
        );
    });
}

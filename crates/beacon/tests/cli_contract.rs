use std::process::Command;

#[test]
fn exposes_the_v01_command_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .arg("--help")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    for command in ["check", "upgrade", "doctor", "history", "config"] {
        assert!(stdout.contains(command), "help did not contain {command}");
    }
}

#[test]
fn history_json_uses_the_versioned_envelope() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["history", "--json"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "ok");
    assert!(value["data"].is_array());
}

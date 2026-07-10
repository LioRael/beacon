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
    assert!(stdout.contains("--no-color"));
    assert!(stdout.contains("--verbose"));
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
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "ok");
    assert!(value["data"].is_array());
}

#[test]
fn check_json_v2_separates_tools_and_inventories_and_uses_explicit_nulls() {
    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert!(value["data"]["tools"].is_array());
    assert!(value["data"]["inventories"].is_array());
    let node = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "node")
        .unwrap();
    assert_eq!(node["status"], "missing");
    assert!(node["installation"].is_null());
    assert!(node["update"].is_null());
}

#[test]
fn partial_check_returns_structured_json_and_exit_two() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();
    let node = path.path().join("node");
    std::fs::write(&node, "#!/bin/sh\nprintf 'not-a-version\\n'\n").unwrap();
    std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "partial");
    assert_eq!(value["errors"][0]["code"], "tool_failed");
    assert_eq!(value["errors"][0]["target"], "tool:node");
}

#[test]
fn fatal_json_commands_still_return_the_v2_envelope() {
    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["upgrade", "node", "--yes", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "error");
    assert!(value["data"].is_null());
    assert_eq!(value["errors"][0]["code"], "fatal_error");
    assert!(value["errors"][0]["target"].is_null());
}

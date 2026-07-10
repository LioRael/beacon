use beacon::{
    Manager, ToolStatus, command::CommandSpec, envelope::Envelope, orchestrator::run_until_failure,
    redact::redact, versions::classify_manager,
};

#[test]
fn homebrew_upgrade_always_has_a_target() {
    assert!(CommandSpec::brew_upgrade("").is_err());
    let command = CommandSpec::brew_upgrade("wget").unwrap();
    assert_eq!(command.program, "brew");
    assert_eq!(command.args, ["upgrade", "wget"]);
}

#[test]
fn manager_is_classified_from_active_executable_path() {
    assert_eq!(
        classify_manager("/opt/homebrew/bin/node"),
        Manager::Homebrew
    );
    assert_eq!(
        classify_manager("/Users/alice/.local/share/mise/installs/node/26/bin/node"),
        Manager::Mise
    );
}

#[test]
fn sensitive_output_is_redacted() {
    let text = "Authorization: Bearer abc123 token=secret /Users/alice/project";
    let result = redact(text, Some("/Users/alice"));
    assert!(!result.contains("abc123"));
    assert!(!result.contains("secret"));
    assert!(!result.contains("/Users/alice"));
}

#[test]
fn json_envelope_has_a_stable_schema() {
    let value = serde_json::to_value(Envelope::ok(vec![ToolStatus::Current])).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "ok");
    assert!(value["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn upgrade_queue_stops_after_first_failure() {
    let mut calls = Vec::new();
    let result = run_until_failure(["node", "go", "rust"], |name| {
        calls.push(name.to_string());
        async move {
            if name == "go" {
                Err(anyhow::anyhow!("boom"))
            } else {
                Ok(())
            }
        }
    })
    .await;

    assert!(result.is_err());
    assert_eq!(calls, ["node", "go"]);
}

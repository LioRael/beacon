#![cfg(unix)]

use std::{os::unix::fs::PermissionsExt, process::Command};

struct BrewFixture {
    home: tempfile::TempDir,
    bin: std::path::PathBuf,
    log: std::path::PathBuf,
}

fn fixture() -> BrewFixture {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("upgrade.log");
    std::fs::create_dir_all(&bin).unwrap();
    let brew = bin.join("brew");
    std::fs::write(&brew, r#"#!/bin/sh
case "$*" in
  "update") ;;
  "list --formula --versions") case "$BREW_KIND" in formula|both) echo 'shared 1.0.0';; esac ;;
  "list --cask --versions") case "$BREW_KIND" in cask|both) echo 'shared 2.0.0';; esac ;;
  "--prefix") echo '/opt/homebrew' ;;
  "outdated --json=v2")
    case "$BREW_KIND" in
      formula) echo '{"formulae":[{"name":"shared","installed_versions":["1.0.0"],"current_version":"1.1.0"}],"casks":[]}' ;;
      cask) echo '{"formulae":[],"casks":[{"name":"shared","installed_versions":["2.0.0"],"current_version":"2.1.0"}]}' ;;
      both) echo '{"formulae":[{"name":"shared","installed_versions":["1.0.0"],"current_version":"1.1.0"}],"casks":[{"name":"shared","installed_versions":["2.0.0"],"current_version":"2.1.0"}]}' ;;
      *) echo '{"formulae":[],"casks":[]}' ;;
    esac ;;
  "upgrade --formula shared") echo "$*" >> "$BREW_LOG" ;;
  "upgrade --cask shared") echo "$*" >> "$BREW_LOG" ;;
  "info --json=v2 shared")
    case "$BREW_KIND" in
      cask) echo '{"formulae":[],"casks":[{"installed":["2.1.0"]}]}' ;;
      *) echo '{"formulae":[{"installed":[{"version":"1.1.0"}]}],"casks":[]}' ;;
    esac ;;
  *) echo "unexpected: $*" >&2; exit 64 ;;
esac
"#).unwrap();
    std::fs::set_permissions(&brew, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["config", "path"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success());
    let config = std::path::PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(config, "schema_version = 2\nenabled_tools = []\nenabled_inventories = [\"homebrew\"]\nhistory_limit = 20\ncommand_timeout_seconds = 5\n").unwrap();
    BrewFixture { home, bin, log }
}

fn upgrade(fixture: &BrewFixture, kind: &str, target: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["upgrade", target, "--yes", "--json"])
        .env("HOME", fixture.home.path())
        .env("PATH", &fixture.bin)
        .env("BREW_KIND", kind)
        .env("BREW_LOG", &fixture.log)
        .output()
        .unwrap()
}

#[test]
fn qualified_and_unique_legacy_targets_execute_a_qualified_action() {
    for (kind, target, action) in [
        ("both", "brew:formula:shared", "upgrade --formula shared\n"),
        ("cask", "brew:cask:shared", "upgrade --cask shared\n"),
        ("formula", "brew:shared", "upgrade --formula shared\n"),
    ] {
        let fixture = fixture();
        let output = upgrade(&fixture, kind, target);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["status"], "ok");
        assert_eq!(std::fs::read_to_string(fixture.log).unwrap(), action);
    }
}

#[test]
fn ambiguous_and_unknown_legacy_targets_do_not_execute() {
    for (kind, target, message) in [
        ("both", "brew:shared", "is ambiguous"),
        ("none", "brew:missing", "not actionable"),
    ] {
        let fixture = fixture();
        let output = upgrade(&fixture, kind, target);
        assert_eq!(output.status.code(), Some(1));
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["status"], "error");
        assert_eq!(value["errors"][0]["code"], "fatal_error");
        assert!(
            value["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains(message)
        );
        assert!(!fixture.log.exists());
    }
}

#[test]
fn targetless_upgrade_does_not_execute_brew() {
    let fixture = fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["upgrade", "--yes", "--json"])
        .env("HOME", fixture.home.path())
        .env("PATH", &fixture.bin)
        .env("BREW_KIND", "formula")
        .env("BREW_LOG", &fixture.log)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!fixture.log.exists());
}

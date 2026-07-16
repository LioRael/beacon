#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command};

struct Fixture {
    home: tempfile::TempDir,
    bin: std::path::PathBuf,
    skill: std::path::PathBuf,
}

impl Fixture {
    fn new(source_type: &str, version: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        let skill = home.path().join(".agents/skills/demo");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "before\n").unwrap();
        std::fs::write(
            home.path().join(".agents/.skill-lock.json"),
            format!(
                r#"{{"version":3,"skills":{{"demo":{{"source":"org/repo","sourceType":"{source_type}","sourceUrl":"https://token@example.com/org/repo?secret=1","ref":"main","skillPath":"skills/demo","skillFolderHash":"tree-one"}}}}}}"#
            ),
        )
        .unwrap();
        let script = format!(
            r#"#!/bin/sh
test "$DISABLE_TELEMETRY" = 1 || {{ echo telemetry-was-not-disabled >&2; exit 70; }}
case "$*" in
  "--version") echo '11.17.0' ;;
  "--yes skills@^1.5.18 --version") echo 'skills {version}' ;;
  "--yes skills@^1.5.18 list --global --json") printf '[{{"name":"demo","path":"{}","scope":"global","agents":["codex","claude"]}}]\n' ;;
  "--yes skills@^1.5.18 update demo --global --yes")
    mkdir -p "$HOME/.agents/skills/demo"
    printf 'after\n' > "$HOME/.agents/skills/demo/SKILL.md"
    ;;
  *) echo "unexpected: $*" >&2; exit 64 ;;
esac
"#,
            skill.display()
        );
        let executable = bin.join("npx");
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_config(home.path(), &["skills"]);
        Self { home, bin, skill }
    }

    fn beacon(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_beacon"));
        command
            .env("HOME", self.home.path())
            .env("PATH", &self.bin)
            .current_dir(self.home.path());
        command
    }
}

fn write_config(home: &Path, inventories: &[&str]) {
    let config = home.join("Library/Application Support/Beacon/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let inventories = inventories
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        config,
        format!(
            "schema_version = 4\nenabled_tools = []\ndisabled_tools = []\nenabled_inventories = [{inventories}]\ndisabled_inventories = []\ntool_catalog_version = 1\ninventory_catalog_version = 1\nhistory_limit = 20\ncommand_timeout_seconds = 5\n"
        ),
    )
    .unwrap();
}

fn check(fixture: &Fixture) -> (std::process::ExitStatus, serde_json::Value) {
    let output = fixture.beacon().args(["check", "--json"]).output().unwrap();
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status, value)
}

#[test]
fn check_uses_an_isolated_mirror_and_reports_scoped_file_changes() {
    let fixture = Fixture::new("github", "1.5.18");
    let (status, value) = check(&fixture);

    assert!(status.success(), "{value}");
    assert_eq!(
        std::fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
        "before\n"
    );
    let report = &value["data"]["inventories"][0];
    assert_eq!(report["id"], "skill:global:demo");
    assert_eq!(report["kind"], "agent-skill");
    assert_eq!(report["scope"], "global");
    assert_eq!(report["status"], "outdated");
    assert_eq!(report["installation_source"], "github");
    assert_eq!(report["update_manager"], "skills");
    assert_eq!(report["source_locator"], "https://example.com/org/repo");
    assert!(
        report["current"]["raw"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(report["changes"][0]["path"], "SKILL.md");
    assert_eq!(report["changes"][0]["kind"], "modified");
    assert_eq!(report["action"]["target_mode"], "rolling");
    assert_eq!(
        report["action"]["command"]["args"],
        serde_json::json!([
            "--yes",
            "skills@^1.5.18",
            "update",
            "demo",
            "--global",
            "--yes"
        ])
    );
    assert!(report.get("agents").is_none());
}

#[test]
fn bunx_is_used_when_npx_is_unavailable() {
    let fixture = Fixture::new("github", "1.5.18");
    let npx = fixture.bin.join("npx");
    let bunx = fixture.bin.join("bunx");
    let script = std::fs::read_to_string(&npx)
        .unwrap()
        .replace("--yes skills@^1.5.18", "skills@^1.5.18");
    std::fs::remove_file(npx).unwrap();
    std::fs::write(&bunx, script).unwrap();
    std::fs::set_permissions(&bunx, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (status, value) = check(&fixture);
    assert!(status.success(), "{value}");
    let action = &value["data"]["inventories"][0]["action"]["command"];
    assert!(action["program"].as_str().unwrap().ends_with("/bunx"));
    assert_eq!(
        action["args"],
        serde_json::json!(["skills@^1.5.18", "update", "demo", "--global", "--yes"])
    );
}

#[test]
fn missing_or_incompatible_manager_is_partial_only_when_enabled() {
    let fixture = Fixture::new("github", "2.0.0");
    let (status, value) = check(&fixture);
    assert_eq!(status.code(), Some(2));
    assert_eq!(value["status"], "partial");
    assert_eq!(value["data"]["inventories"][0]["id"], "skills");
    assert!(
        value["data"]["inventories"][0]["detail"]
            .as_str()
            .unwrap()
            .contains(">=1.5.18,<2.0.0")
    );

    write_config(fixture.home.path(), &[]);
    let (status, value) = check(&fixture);
    assert!(status.success());
    assert_eq!(value["status"], "ok");
    assert!(value["data"]["inventories"].as_array().unwrap().is_empty());
}

#[test]
fn a_fresh_catalog_enables_skills_only_after_the_capability_probe_passes() {
    let fixture = Fixture::new("github", "1.5.18");
    std::fs::remove_file(
        fixture
            .home
            .path()
            .join("Library/Application Support/Beacon/config.toml"),
    )
    .unwrap();
    let output = fixture
        .beacon()
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["data"]["enabled_inventories"],
        serde_json::json!(["skills"])
    );
    assert_eq!(value["data"]["inventory_catalog_version"], 1);
}

#[test]
fn unsafe_receipt_source_is_visible_but_unmanaged_without_partial_status() {
    let fixture = Fixture::new("local", "1.5.18");
    let (status, value) = check(&fixture);
    assert!(status.success(), "{value}");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["data"]["inventories"][0]["status"], "unmanaged");
    assert!(value["data"]["inventories"][0]["action"].is_null());
}

#[test]
fn upgrade_mutates_only_through_skills_and_records_global_scope() {
    let fixture = Fixture::new("github", "1.5.18");
    let output = fixture
        .beacon()
        .args(["upgrade", "skill:global:demo", "--yes", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
        "after\n"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"][0]["resource_scope"], "global");
    assert!(value["data"][0]["scope_locator"].is_null());

    let history = fixture
        .beacon()
        .args(["history", "--json"])
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(value["data"][0]["tool"], "skill:global:demo");
    assert_eq!(value["data"][0]["resource_scope"], "global");
    assert_eq!(value["data"][0]["installation_source"], "github");
    assert_eq!(value["data"][0]["update_manager"], "skills");
}

#[test]
fn failed_real_update_retains_only_the_canonical_recovery_copy_and_receipt() {
    let fixture = Fixture::new("github", "1.5.18");
    let executable = fixture.bin.join("npx");
    std::fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
test "$DISABLE_TELEMETRY" = 1 || exit 70
case "$*" in
  "--version") echo '11.17.0' ;;
  "--yes skills@^1.5.18 --version") echo '1.5.18' ;;
  "--yes skills@^1.5.18 list --global --json") printf '[{{"name":"demo","path":"{}","scope":"global","agents":["codex"]}}]\n' ;;
  "--yes skills@^1.5.18 update demo --global --yes")
    if [ "$HOME" = "{}" ]; then
      echo 'real update failed' >&2
      exit 9
    fi
    printf 'after\n' > "$HOME/.agents/skills/demo/SKILL.md"
    ;;
  *) exit 64 ;;
esac
"#,
            fixture.skill.display(),
            fixture.home.path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = fixture
        .beacon()
        .args(["upgrade", "skill:global:demo", "--yes", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
        "before\n"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["status"], "error");
    assert!(value["errors"][0]["message"].as_str().unwrap().contains(
        "Recovery copy retained at ~/Library/Application Support/Beacon/recovery/skills"
    ));

    let recovery_root = fixture
        .home
        .path()
        .join("Library/Application Support/Beacon/recovery/skills");
    let entries = std::fs::read_dir(recovery_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        std::fs::read_to_string(entries[0].join("skill/SKILL.md")).unwrap(),
        "before\n"
    );
    assert!(entries[0].join("receipt.json").is_file());
    assert!(entries[0].join("RECOVERY.txt").is_file());
    assert_eq!(
        std::fs::read_dir(&entries[0]).unwrap().count(),
        3,
        "Beacon must not create agent-specific recovery paths"
    );
}

#[test]
fn a_global_content_change_without_a_receipt_change_becomes_unmanaged() {
    let fixture = Fixture::new("github", "1.5.18");
    let (status, first) = check(&fixture);
    assert!(status.success(), "{first}");

    std::fs::write(fixture.skill.join("SKILL.md"), "local edit\n").unwrap();
    let (status, second) = check(&fixture);
    assert!(status.success(), "{second}");
    let report = &second["data"]["inventories"][0];
    assert_eq!(report["status"], "unmanaged");
    assert!(
        report["detail"]
            .as_str()
            .unwrap()
            .contains("without a receipt change")
    );
}

#[test]
fn project_scope_uses_the_nearest_lock_and_guards_local_modifications() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    let skill = project.path().join(".agents/skills/demo");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "before\n").unwrap();
    let computed_hash = upstream_directory_hash(&skill);
    std::fs::write(
        project.path().join("skills-lock.json"),
        format!(
            r#"{{"version":1,"skills":{{"demo":{{"source":"org/repo","sourceType":"github","sourceUrl":"https://example.com/org/repo","ref":"main","skillPath":"skills/demo","computedHash":"{computed_hash}"}}}}}}"#
        ),
    )
    .unwrap();
    let executable = bin.join("npx");
    std::fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
test "$DISABLE_TELEMETRY" = 1 || exit 70
case "$*" in
  "--version") echo '11.17.0' ;;
  "--yes skills@^1.5.18 --version") echo '1.5.18' ;;
  "--yes skills@^1.5.18 list --global --json") printf '[]\n' ;;
  "--yes skills@^1.5.18 list --json") printf '[{{"name":"demo","path":"{}","scope":"project","agents":[]}}]\n' ;;
  "--yes skills@^1.5.18 update demo --project --yes") printf 'after\n' > "$PWD/.agents/skills/demo/SKILL.md" ;;
  *) exit 64 ;;
esac
"#,
            skill.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    write_config(home.path(), &["skills"]);

    let run = || {
        Command::new(env!("CARGO_BIN_EXE_beacon"))
            .args(["check", "--json"])
            .env("HOME", home.path())
            .env("PATH", &bin)
            .current_dir(project.path().join(".agents/skills/demo"))
            .output()
            .unwrap()
    };
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["inventories"][0]["id"], "skill:project:demo");
    assert_eq!(value["data"]["inventories"][0]["status"], "outdated");
    assert_eq!(
        std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        "before\n"
    );

    std::fs::write(skill.join("SKILL.md"), "local edit\n").unwrap();
    let output = run();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["inventories"][0]["status"], "unmanaged");
}

#[test]
fn check_aggregates_global_and_project_scopes_and_rejects_an_ambiguous_bare_name() {
    let fixture = Fixture::new("github", "1.5.18");
    let project = tempfile::tempdir().unwrap();
    let project_skill = project.path().join(".agents/skills/demo");
    std::fs::create_dir_all(&project_skill).unwrap();
    std::fs::write(project_skill.join("SKILL.md"), "project before\n").unwrap();
    let computed_hash = upstream_directory_hash(&project_skill);
    std::fs::write(
        project.path().join("skills-lock.json"),
        format!(
            r#"{{"version":1,"skills":{{"demo":{{"source":"org/project","sourceType":"github","sourceUrl":"https://example.com/org/project","ref":"main","skillPath":"skills/demo","computedHash":"{computed_hash}"}}}}}}"#
        ),
    )
    .unwrap();
    let executable = fixture.bin.join("npx");
    std::fs::write(
        &executable,
        format!(
            r#"#!/bin/sh
test "$DISABLE_TELEMETRY" = 1 || exit 70
case "$*" in
  "--version") echo '11.17.0' ;;
  "--yes skills@^1.5.18 --version") echo '1.5.18' ;;
  "--yes skills@^1.5.18 list --global --json") printf '[{{"name":"demo","path":"{}","scope":"global","agents":[]}}]\n' ;;
  "--yes skills@^1.5.18 list --json") printf '[{{"name":"demo","path":"{}","scope":"project","agents":[]}}]\n' ;;
  "--yes skills@^1.5.18 update demo --global --yes") printf 'global after\n' > "$HOME/.agents/skills/demo/SKILL.md" ;;
  "--yes skills@^1.5.18 update demo --project --yes") printf 'project after\n' > "$PWD/.agents/skills/demo/SKILL.md" ;;
  *) exit 64 ;;
esac
"#,
            fixture.skill.display(),
            project_skill.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = fixture
        .beacon()
        .args(["check", "--json"])
        .current_dir(&project_skill)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids = value["data"]["inventories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|report| report["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["skill:global:demo", "skill:project:demo"]);

    let ambiguous = fixture
        .beacon()
        .args(["upgrade", "demo", "--yes", "--json"])
        .current_dir(project.path())
        .output()
        .unwrap();
    assert_eq!(ambiguous.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&ambiguous.stdout).unwrap();
    assert!(
        value["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("ambiguous")
    );
    assert_eq!(
        std::fs::read_to_string(fixture.skill.join("SKILL.md")).unwrap(),
        "before\n"
    );
    assert_eq!(
        std::fs::read_to_string(project_skill.join("SKILL.md")).unwrap(),
        "project before\n"
    );
}

fn upstream_directory_hash(root: &Path) -> String {
    let mut paths = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort();
    let mut hasher = Sha256::new();
    for path in paths {
        let relative = path.strip_prefix(root).unwrap().to_string_lossy();
        hasher.update(relative.as_bytes());
        hasher.update(std::fs::read(path).unwrap());
    }
    format!("{:x}", hasher.finalize())
}

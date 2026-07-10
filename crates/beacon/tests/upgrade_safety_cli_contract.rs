#![cfg(unix)]

use std::{
    ffi::OsString,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn write_executable(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn write_config(home: &Path, tools: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["config", "path"])
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(output.status.success());
    let config = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let tools = tools
        .iter()
        .map(|tool| format!("\"{tool}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        config,
        format!(
            "schema_version = 2\nenabled_tools = [{tools}]\nenabled_inventories = []\nhistory_limit = 20\ncommand_timeout_seconds = 5\n"
        ),
    )
    .unwrap();
}

fn path_with(bin: &Path) -> OsString {
    // Keep /usr/bin and /bin so /usr/bin/which and shell utilities remain available
    // while fixture tools still win via PATH order.
    std::env::join_paths([bin.as_os_str(), "/usr/bin".as_ref(), "/bin".as_ref()]).unwrap()
}

struct DenoFixture {
    home: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    version: PathBuf,
}

fn deno_fixture(mode: &str) -> DenoFixture {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".deno/bin");
    let state = home.path().join("fixture-state");
    std::fs::create_dir_all(&state).unwrap();
    let log = state.join("upgrade.log");
    let version = state.join("version");
    std::fs::write(&version, "2.1.0\n").unwrap();
    write_executable(
        &bin.join("deno"),
        &format!(
            r#"#!/bin/sh
VERSION_FILE="{version}"
LOG_FILE="{log}"
MODE="{mode}"
case "$1" in
  --version)
    read ver < "$VERSION_FILE"
    printf 'deno %s\n' "$ver"
    ;;
  upgrade)
    if [ "$2" = "--dry-run" ]; then
      printf 'A new release of Deno is available: 2.1.1\n'
      if [ "$MODE" = "stale" ]; then
        printf '2.1.5\n' > "$VERSION_FILE"
      fi
      exit 0
    fi
    if [ "$2" = "--version" ]; then
      printf 'upgrade --version %s\n' "$3" >> "$LOG_FILE"
      if [ "$MODE" = "exact-success" ] || [ "$MODE" = "post-identity" ]; then
        printf '%s\n' "$3" > "$VERSION_FILE"
      elif [ "$MODE" = "exact-wrong" ]; then
        printf '2.1.9\n' > "$VERSION_FILE"
      fi
      if [ "$MODE" = "post-identity" ]; then
        # Replace the PATH entry with a symlink whose canonical target is outside
        # `/.deno/` so post-upgrade claim revalidation sees an identity change
        # after exact version verification has already succeeded.
        relocated="$(dirname "$VERSION_FILE")/relocated-bin"
        mkdir -p "$relocated"
        printf '%s\n' '#!/bin/sh' > "$relocated/deno"
        printf '%s\n' "VERSION_FILE=\"$VERSION_FILE\"" >> "$relocated/deno"
        printf '%s\n' 'read ver < "$VERSION_FILE"' >> "$relocated/deno"
        printf '%s\n' 'printf "deno %s\n" "$ver"' >> "$relocated/deno"
        chmod 755 "$relocated/deno"
        rm -f "$0"
        ln -s "$relocated/deno" "$0"
      fi
      exit 0
    fi
    echo "unexpected upgrade args: $*" >&2
    exit 64
    ;;
  *)
    echo "unexpected: $*" >&2
    exit 64
    ;;
esac
"#,
            version = version.display(),
            log = log.display(),
            mode = mode,
        ),
    );
    write_config(home.path(), &["deno"]);
    DenoFixture {
        home,
        bin,
        log,
        version,
    }
}

struct BunFixture {
    home: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
}

fn bun_fixture(mode: &str) -> BunFixture {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".bun/bin");
    let state = home.path().join("fixture-state");
    std::fs::create_dir_all(&state).unwrap();
    let log = state.join("upgrade.log");
    let version = state.join("version");
    // floating-below starts lower so an intermediate result can land above old
    // but still below the confirmed expected floor.
    let initial = if mode == "floating-below" {
        "1.0.0"
    } else {
        "1.2.0"
    };
    let latest_tag = if mode == "floating-below" {
        "bun-v2.0.0"
    } else {
        "bun-v1.2.1"
    };
    std::fs::write(&version, format!("{initial}\n")).unwrap();
    write_executable(
        &bin.join("bun"),
        &format!(
            r#"#!/bin/sh
VERSION_FILE="{version}"
LOG_FILE="{log}"
MODE="{mode}"
case "$1" in
  --version)
    read ver < "$VERSION_FILE"
    printf '%s\n' "$ver"
    ;;
  upgrade)
    printf 'upgrade\n' >> "$LOG_FILE"
    if [ "$MODE" = "floating-success" ]; then
      printf '1.2.2\n' > "$VERSION_FILE"
    elif [ "$MODE" = "floating-below" ]; then
      printf '1.5.0\n' > "$VERSION_FILE"
    fi
    exit 0
    ;;
  *)
    echo "unexpected: $*" >&2
    exit 64
    ;;
esac
"#,
            version = version.display(),
            log = log.display(),
            mode = mode,
        ),
    );
    write_executable(
        &bin.join("curl"),
        &format!("#!/bin/sh\nprintf '{{\"tag_name\":\"{latest_tag}\"}}\\n'\n"),
    );
    write_config(home.path(), &["bun"]);
    BunFixture { home, bin, log }
}

fn upgrade_json(home: &Path, bin: &Path, targets: &[&str]) -> (i32, serde_json::Value) {
    let mut args = vec!["upgrade".to_string()];
    args.extend(targets.iter().map(|target| (*target).to_string()));
    args.extend(["--yes".into(), "--json".into()]);
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(&args)
        .env("HOME", home)
        .env("PATH", path_with(bin))
        .output()
        .unwrap();
    let code = output.status.code().unwrap_or(-1);
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "invalid json (code {code}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (code, value)
}

fn history_json(home: &Path) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["history", "--json"])
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn stale_plan_aborts_before_mutation_and_persists_recovery() {
    let fixture = deno_fixture("stale");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["deno"]);

    assert_eq!(code, 1, "{value}");
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["code"], "upgrade_failed");
    assert_eq!(value["errors"][0]["target"], "tool:deno");
    let message = value["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.to_lowercase().contains("drift")
            || message.to_lowercase().contains("changed")
            || message.contains("PATH"),
        "expected drift guidance, got {message}"
    );
    assert!(
        message.contains("Inspect PATH") || message.contains("reinstall"),
        "expected recovery guidance, got {message}"
    );
    assert!(
        !fixture.log.exists() || std::fs::read_to_string(&fixture.log).unwrap().is_empty(),
        "upgrade command must not run on stale plan"
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.version).unwrap().trim(),
        "2.1.5",
        "stale fixture must mutate observed version during planning/preflight"
    );

    let history = history_json(fixture.home.path());
    let entries = history["data"].as_array().unwrap();
    let failed = entries
        .iter()
        .find(|entry| {
            entry["operation"] == "upgrade"
                && entry["tool"] == "deno"
                && entry["status"] == "failed"
        })
        .unwrap_or_else(|| panic!("failed upgrade must be persisted: {history}"));
    let summary = failed["summary"].as_str().unwrap();
    assert!(
        summary.contains("Inspect PATH") || summary.contains("reinstall"),
        "history must persist recovery guidance, got {summary}"
    );
    assert!(
        failed.get("manager").is_none(),
        "history must not expose ambiguous manager field: {failed}"
    );
    assert_eq!(failed["installation_source"], "deno-official");
    assert_eq!(failed["update_manager"], "deno-official");
    assert!(
        !summary.contains(fixture.home.path().to_str().unwrap()),
        "history summary must be redacted, got {summary}"
    );
}

#[test]
fn exact_upgrade_succeeds_only_when_result_equals_expected_version() {
    let fixture = deno_fixture("exact-success");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["deno"]);

    assert_eq!(code, 0, "{value}");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["data"][0]["tool"], "deno");
    assert_eq!(value["data"][0]["status"], "success");
    assert_eq!(value["data"][0]["old_version"], "2.1.0");
    assert_eq!(value["data"][0]["new_version"], "2.1.1");
    assert_eq!(value["data"][0]["action"]["target_mode"], "exact");
    assert_eq!(
        value["data"][0]["action"]["expected_version"]["normalized"],
        "2.1.1"
    );
    assert_eq!(
        value["data"][0]["action"]["command"]["args"],
        serde_json::json!(["upgrade", "--version", "2.1.1"])
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.log).unwrap(),
        "upgrade --version 2.1.1\n"
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.version).unwrap().trim(),
        "2.1.1"
    );

    let history = history_json(fixture.home.path());
    let success = history["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| {
            entry["operation"] == "upgrade"
                && entry["tool"] == "deno"
                && entry["status"] == "success"
        })
        .unwrap_or_else(|| panic!("successful upgrade must be persisted: {history}"));
    assert!(success.get("manager").is_none(), "{success}");
    assert_eq!(success["installation_source"], "deno-official");
    assert_eq!(success["update_manager"], "deno-official");
    assert_eq!(success["old_version"], "2.1.0");
    assert_eq!(success["new_version"], "2.1.1");
}

#[test]
fn exact_upgrade_fails_when_result_differs_from_expected_version() {
    let fixture = deno_fixture("exact-wrong");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["deno"]);

    assert_eq!(code, 1, "{value}");
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["code"], "upgrade_failed");
    let message = value["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("exact verification") || message.contains("expected"),
        "expected exact verification failure, got {message}"
    );
    assert!(
        message.contains("Inspect PATH") || message.contains("reinstall"),
        "expected recovery guidance, got {message}"
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.log).unwrap(),
        "upgrade --version 2.1.1\n"
    );
}

#[test]
fn exact_upgrade_fails_when_result_is_unchanged() {
    let fixture = deno_fixture("exact-noop");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["deno"]);

    assert_eq!(code, 1, "{value}");
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["code"], "upgrade_failed");
    let message = value["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("exact verification") || message.contains("expected"),
        "expected exact verification failure, got {message}"
    );
}

#[test]
fn floating_upgrade_succeeds_when_result_is_newer_and_not_below_expected() {
    let fixture = bun_fixture("floating-success");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["bun"]);

    assert_eq!(code, 0, "{value}");
    assert_eq!(value["status"], "ok");
    assert_eq!(value["data"][0]["tool"], "bun");
    assert_eq!(value["data"][0]["old_version"], "1.2.0");
    assert_eq!(value["data"][0]["new_version"], "1.2.2");
    assert_eq!(value["data"][0]["action"]["target_mode"], "floating");
    assert_eq!(
        value["data"][0]["action"]["expected_version"]["normalized"],
        "1.2.1"
    );
    assert_eq!(std::fs::read_to_string(&fixture.log).unwrap(), "upgrade\n");
}

#[test]
fn floating_upgrade_fails_when_result_is_not_newer() {
    let fixture = bun_fixture("floating-noop");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["bun"]);

    assert_eq!(code, 1, "{value}");
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["code"], "upgrade_failed");
    let message = value["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("floating verification") || message.contains("newer"),
        "expected floating verification failure, got {message}"
    );
    assert_eq!(std::fs::read_to_string(&fixture.log).unwrap(), "upgrade\n");
}

#[test]
fn floating_upgrade_fails_when_result_is_above_old_but_below_expected() {
    let fixture = bun_fixture("floating-below");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["bun"]);

    assert_eq!(code, 1, "{value}");
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["code"], "upgrade_failed");
    let message = value["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("floating verification") || message.contains("newer"),
        "expected floating verification failure, got {message}"
    );
    assert_eq!(std::fs::read_to_string(&fixture.log).unwrap(), "upgrade\n");
}

#[test]
fn post_upgrade_identity_change_is_detected_after_exact_success() {
    let fixture = deno_fixture("post-identity");
    let (code, value) = upgrade_json(fixture.home.path(), &fixture.bin, &["deno"]);

    assert_eq!(code, 1, "{value}");
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["code"], "upgrade_failed");
    let message = value["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("source or updater changed")
            || message.contains("active executable changed")
            || message.contains("installation missing")
            || message.contains("updater missing"),
        "expected post-upgrade identity failure, got {message}"
    );
    assert!(
        message.contains("Inspect PATH") || message.contains("reinstall"),
        "expected recovery guidance, got {message}"
    );
    assert_eq!(
        std::fs::read_to_string(&fixture.log).unwrap(),
        "upgrade --version 2.1.1\n"
    );
}

#[test]
fn upgrade_queue_stops_after_first_failure_and_skips_later_targets() {
    let home = tempfile::tempdir().unwrap();
    let state = home.path().join("fixture-state");
    std::fs::create_dir_all(&state).unwrap();
    let deno_log = state.join("deno.log");
    let bun_log = state.join("bun.log");
    let deno_version = state.join("deno.version");
    let bun_version = state.join("bun.version");
    std::fs::write(&deno_version, "2.1.0\n").unwrap();
    std::fs::write(&bun_version, "1.2.0\n").unwrap();

    let deno_bin = home.path().join(".deno/bin");
    let bun_bin = home.path().join(".bun/bin");
    write_executable(
        &deno_bin.join("deno"),
        &format!(
            r#"#!/bin/sh
VERSION_FILE="{version}"
LOG_FILE="{log}"
case "$1" in
  --version)
    read ver < "$VERSION_FILE"
    printf 'deno %s\n' "$ver"
    ;;
  upgrade)
    if [ "$2" = "--dry-run" ]; then
      printf 'A new release of Deno is available: 2.1.1\n'
      exit 0
    fi
    if [ "$2" = "--version" ]; then
      printf 'upgrade --version %s\n' "$3" >> "$LOG_FILE"
      exit 0
    fi
    exit 64
    ;;
  *) exit 64 ;;
esac
"#,
            version = deno_version.display(),
            log = deno_log.display(),
        ),
    );
    write_executable(
        &bun_bin.join("bun"),
        &format!(
            r#"#!/bin/sh
VERSION_FILE="{version}"
LOG_FILE="{log}"
case "$1" in
  --version)
    read ver < "$VERSION_FILE"
    printf '%s\n' "$ver"
    ;;
  upgrade)
    printf 'upgrade\n' >> "$LOG_FILE"
    exit 0
    ;;
  *) exit 64 ;;
esac
"#,
            version = bun_version.display(),
            log = bun_log.display(),
        ),
    );
    write_executable(
        &bun_bin.join("curl"),
        "#!/bin/sh\nprintf '{\"tag_name\":\"bun-v1.2.1\"}\\n'\n",
    );
    write_config(home.path(), &["bun", "deno"]);

    // Combined PATH: both tool bins first, then system paths.
    let path = std::env::join_paths([
        bun_bin.as_os_str(),
        deno_bin.as_os_str(),
        "/usr/bin".as_ref(),
        "/bin".as_ref(),
    ])
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["upgrade", "bun", "deno", "--yes", "--json"])
        .env("HOME", home.path())
        .env("PATH", &path)
        .output()
        .unwrap();
    let code = output.status.code().unwrap_or(-1);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("invalid json: {}", String::from_utf8_lossy(&output.stdout)));

    // bun sorts before deno; floating-noop style failure stops the queue.
    assert_eq!(code, 1, "{value}");
    assert_eq!(value["status"], "error");
    assert_eq!(value["errors"][0]["target"], "tool:bun");
    assert!(
        value["data"]
            .as_array()
            .map(|items| items.is_empty())
            .unwrap_or(true),
        "no successful upgrades expected: {value}"
    );
    assert_eq!(std::fs::read_to_string(&bun_log).unwrap(), "upgrade\n");
    assert!(
        !deno_log.exists() || std::fs::read_to_string(&deno_log).unwrap().is_empty(),
        "later queue item must not mutate after failure"
    );
}

#[test]
fn current_and_missing_tools_cannot_enter_upgrade_selection() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".deno/bin");
    write_executable(
        &bin.join("deno"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'deno 2.1.1\\n'; elif [ \"$1\" = upgrade ] && [ \"$2\" = --dry-run ]; then printf 'A new release of Deno is available: 2.1.1\\n'; else exit 64; fi\n",
    );
    write_config(home.path(), &["deno", "bun"]);

    let missing = upgrade_json(home.path(), &bin, &["bun"]);
    assert_eq!(missing.0, 1, "{}", missing.1);
    assert_eq!(missing.1["status"], "error");
    assert!(
        missing.1["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("not installed")
            || missing.1["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("not actionable"),
        "{}",
        missing.1
    );

    let current = upgrade_json(home.path(), &bin, &["deno"]);
    assert_eq!(current.0, 1, "{}", current.1);
    assert_eq!(current.1["status"], "error");
    assert!(
        current.1["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("not actionable"),
        "{}",
        current.1
    );
}

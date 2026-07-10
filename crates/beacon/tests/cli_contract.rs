use std::process::Command;

#[cfg(unix)]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn check_json(
    home: &std::path::Path,
    bin: &std::path::Path,
    current_dir: Option<&std::path::Path>,
) -> serde_json::Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_beacon"));
    command
        .args(["check", "--json"])
        .env("HOME", home)
        .env("PATH", bin);
    if let Some(current_dir) = current_dir {
        command.current_dir(current_dir);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn tool_report<'a>(value: &'a serde_json::Value, tool: &str) -> &'a serde_json::Value {
    value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == tool)
        .unwrap()
}

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
    assert!(node["diagnostics"].is_object());
}

#[test]
fn doctor_json_v2_separates_tools_and_inventories() {
    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["doctor", "node", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "ok");
    assert!(value["errors"].as_array().unwrap().is_empty());
    assert_eq!(value["data"]["tools"].as_array().unwrap().len(), 1);
    assert!(value["data"]["inventories"].is_array());
}

#[test]
fn human_check_shows_source_to_updater_column() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let install = fixture.path().join("mise/installs/node/20");
    let bin = install.join("bin");
    write_executable(&bin.join("node"), "#!/bin/sh\nprintf 'v20.0.0\\n'\n");
    write_executable(
        &bin.join("mise"),
        &format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  'ls --json') printf '{{\"node\":[{{\"version\":\"20\",\"requested_version\":\"20\",\"install_path\":\"{}\"}}]}}\\n' ;;\n  'latest node@20') printf '20.1.0\\n' ;;\nesac\n",
            install.display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--no-color"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .env("TERM", "dumb")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("SOURCE → UPDATER"),
        "human table must show SOURCE → UPDATER: {stdout}"
    );
    assert!(
        stdout.contains("mise → mise")
            || stdout
                .lines()
                .any(|line| line.contains("mise") && line.contains("→")),
        "human rows must present source → updater ownership: {stdout}"
    );
}

#[test]
fn check_json_keeps_progress_silent_on_stderr() {
    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();
    write_executable(
        &path.path().join("node"),
        "#!/bin/sh\nprintf 'v20.0.0\\n'\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .env("TERM", "xterm-256color")
        .output()
        .unwrap();

    assert!(
        output.stderr.is_empty(),
        "JSON mode must suppress progress on stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert!(value["data"]["tools"].is_array());
}

#[test]
fn config_show_json_uses_the_v2_envelope() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["config", "show", "--json"])
        .env("HOME", home.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "ok");
    assert_eq!(value["data"]["schema_version"], 2);
    assert!(value["errors"].as_array().unwrap().is_empty());
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
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains(&0x1b));
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
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains(&0x1b));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "error");
    assert!(value["data"].is_null());
    assert_eq!(value["errors"][0]["code"], "fatal_error");
    assert!(value["errors"][0]["target"].is_null());
}

#[test]
fn check_json_reports_the_active_rustup_channel_without_reading_project_policy() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let bin = home.path().join(".cargo/bin");
    let rustc = bin.join("rustc");
    write_executable(
        &bin.join("rustup"),
        &format!(
            "#!/bin/sh\nif [ \"${{0##*/}}\" = rustc ]; then printf 'rustc 1.80.0 (fixture)\\n'; exit; fi\ncase \"$1 $2\" in\n  'show active-toolchain') printf 'stable-aarch64-apple-darwin (default)\\n' ;;\n  'which rustc') printf '{}\\n' ;;\n  'check ') printf 'stable-aarch64-apple-darwin - Update available : 1.80.0 -> 1.81.0\\n' ;;\nesac\n",
            rustc.display()
        ),
    );
    std::fs::hard_link(bin.join("rustup"), &rustc).unwrap();
    let policy = project.path().join("rust-toolchain.toml");
    let original = "[toolchain]\nchannel = \"nightly\"\n";
    std::fs::write(&policy, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rust = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "rust")
        .unwrap();
    assert_eq!(rust["status"], "outdated");
    assert_eq!(rust["update"]["manager"], "rustup");
    assert_eq!(
        rust["update"]["action"]["command"]["args"],
        serde_json::json!(["update", "stable-aarch64-apple-darwin"])
    );
    assert_eq!(std::fs::read_to_string(policy).unwrap(), original);
}

#[test]
fn check_json_reports_homebrew_go_with_an_explicit_formula_action() {
    let home = tempfile::tempdir().unwrap();
    let prefix = tempfile::tempdir().unwrap();
    let bin = prefix.path().join("bin");
    write_executable(
        &bin.join("go"),
        "#!/bin/sh\nprintf 'go version go1.22.0 darwin/arm64\\n'\n",
    );
    write_executable(
        &bin.join("brew"),
        &format!(
            "#!/bin/sh\ncase \"$1 $2 $3\" in\n  'list --formula --versions') printf 'go 1.22.0\\n' ;;\n  'list --cask --versions') : ;;\n  '--prefix  ') printf '{}\\n' ;;\n  'info --json=v2 go') printf '{{\"formulae\":[{{\"versions\":{{\"stable\":\"1.23.0\"}}}}]}}\\n' ;;\n  'outdated --json=v2 ') printf '{{\"formulae\":[],\"casks\":[]}}\\n' ;;\nesac\n",
            prefix.path().display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let go = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "go")
        .unwrap();
    assert_eq!(go["installation"]["source"], "homebrew");
    assert_eq!(go["update"]["manager"], "homebrew");
    assert_eq!(
        go["update"]["action"]["command"]["args"],
        serde_json::json!(["upgrade", "--formula", "go"])
    );
}

#[test]
fn check_json_preserves_the_active_global_mise_go_selector() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let install = fixture.path().join("mise/installs/go/1.22");
    let bin = install.join("bin");
    write_executable(
        &bin.join("go"),
        "#!/bin/sh\nprintf 'go version go1.22.0 darwin/arm64\\n'\n",
    );
    write_executable(
        &bin.join("mise"),
        &format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  'ls --json') printf '{{\"go\":[{{\"version\":\"1.22.0\",\"requested_version\":\"1.22\",\"install_path\":\"{}\"}}]}}\\n' ;;\n  'latest go@1.22') printf '1.22.1\\n' ;;\nesac\n",
            install.display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let go = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "go")
        .unwrap();
    assert_eq!(go["installation"]["source"], "mise");
    assert_eq!(go["update"]["manager"], "mise");
    assert_eq!(
        go["update"]["action"]["command"]["args"],
        serde_json::json!(["use", "-g", "go@1.22"])
    );
}

#[test]
fn check_json_reports_rustup_query_failure_as_a_partial_schema_v2_result() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join(".cargo/bin");
    write_executable(
        &bin.join("rustc"),
        "#!/bin/sh\nprintf 'rustc 1.80.0 (fixture)\\n'\n",
    );
    write_executable(&bin.join("rustup"), "#!/bin/sh\nexit 1\n");

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["status"], "partial");
    let rust = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "rust")
        .unwrap();
    assert_eq!(rust["status"], "failed");
    assert!(rust["installation"].is_object());
    assert!(rust["update"].is_null());
    assert_eq!(value["errors"][0]["code"], "tool_failed");
    assert_eq!(value["errors"][0]["target"], "tool:rust");
}

#[test]
fn check_json_reports_npm_global_pnpm_with_an_exact_pinned_action() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("node_modules/.bin");
    write_executable(&bin.join("pnpm"), "#!/bin/sh\nprintf '10.0.0\\n'\n");
    write_executable(
        &bin.join("npm"),
        "#!/bin/sh\nif [ \"$1\" = prefix ]; then printf '/fixture/npm-global\\n'; else printf '10.1.0\\n'; fi\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let pnpm = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "pnpm")
        .unwrap();
    assert_eq!(pnpm["installation"]["source"], "npm-global");
    assert_eq!(pnpm["update"]["manager"], "npm");
    assert_eq!(pnpm["update"]["action"]["target_mode"], "exact");
    assert_eq!(
        pnpm["update"]["action"]["command"]["args"],
        serde_json::json!(["install", "--global", "pnpm@10.1.0"])
    );
}

#[test]
fn project_package_manager_pin_keeps_corepack_pnpm_unmanaged_and_untouched() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("corepack/shims");
    write_executable(&bin.join("pnpm"), "#!/bin/sh\nprintf '10.0.0\\n'\n");
    write_executable(&bin.join("corepack"), "#!/bin/sh\nprintf '0.31.0\\n'\n");
    let package_json = project.path().join("package.json");
    let original = "{\n  \"packageManager\": \"pnpm@10.0.0\"\n}\n";
    std::fs::write(&package_json, original).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let pnpm = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "pnpm")
        .unwrap();
    assert_eq!(pnpm["status"], "unmanaged");
    assert_eq!(pnpm["installation"]["source"], "corepack");
    assert!(pnpm["update"].is_null());
    assert_eq!(std::fs::read_to_string(package_json).unwrap(), original);
}

#[test]
fn ancestor_project_mise_selector_keeps_node_unmanaged_and_untouched() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("mise/installs/node/22/bin");
    write_executable(&bin.join("node"), "#!/bin/sh\nprintf 'v22.0.0\\n'\n");
    write_executable(
        &bin.join("mise"),
        &format!(
            "#!/bin/sh\nprintf '{{\"node\":[{{\"version\":\"22.0.0\",\"requested_version\":\"lts\",\"install_path\":\"{}\"}}]}}\\n'\n",
            fixture.path().join("mise/installs/node/22").display()
        ),
    );
    let config = project.path().join(".mise.toml");
    let original = "[tools]\nnode = \"lts\"\n";
    std::fs::write(&config, original).unwrap();
    let nested = project.path().join("packages/app");
    std::fs::create_dir_all(&nested).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .current_dir(nested)
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let node = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "node")
        .unwrap();
    assert_eq!(node["status"], "unmanaged");
    assert_eq!(node["installation"]["source"], "mise");
    assert!(node["update"].is_null());
    assert_eq!(std::fs::read_to_string(config).unwrap(), original);
}

#[test]
fn global_mise_pnpm_preserves_selector_in_latest_and_upgrade_action() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("mise/installs/pnpm/10/bin");
    write_executable(&bin.join("pnpm"), "#!/bin/sh\nprintf '10.0.0\\n'\n");
    write_executable(
        &bin.join("mise"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = ls ]; then printf '{{\"pnpm\":[{{\"version\":\"10.0.0\",\"requested_version\":\"lts\",\"install_path\":\"{}\"}}]}}\\n'; else printf '10.1.0\\n'; fi\n",
            fixture.path().join("mise/installs/pnpm/10").display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let pnpm = value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "pnpm")
        .unwrap();
    assert_eq!(pnpm["status"], "outdated");
    assert_eq!(pnpm["installation"]["source"], "mise");
    assert_eq!(pnpm["update"]["manager"], "mise");
    assert_eq!(pnpm["update"]["action"]["target_mode"], "floating");
    assert_eq!(
        pnpm["update"]["action"]["command"]["args"],
        serde_json::json!(["use", "-g", "pnpm@lts"])
    );
}

#[test]
fn official_bun_and_deno_report_safe_channel_specific_actions() {
    for (tool, current, latest, manager, mode, expected_args) in [
        (
            "bun",
            "1.2.0",
            "1.2.1",
            "bun-official",
            "floating",
            serde_json::json!(["upgrade"]),
        ),
        (
            "deno",
            "2.1.0",
            "2.1.1",
            "deno-official",
            "exact",
            serde_json::json!(["upgrade", "--version", "2.1.1"]),
        ),
    ] {
        let home = tempfile::tempdir().unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let bin = fixture.path().join(format!(".{tool}/bin"));
        let tool_body = if tool == "deno" {
            format!(
                "#!/bin/sh\nif [ \"$1\" = upgrade ]; then printf 'A new release of Deno is available: {latest}\\n'; else printf 'deno {current}\\n'; fi\n"
            )
        } else {
            format!("#!/bin/sh\nprintf '{current}\\n'\n")
        };
        write_executable(&bin.join(tool), &tool_body);
        if tool == "bun" {
            write_executable(
                &bin.join("curl"),
                &format!("#!/bin/sh\nprintf '{{\"tag_name\":\"bun-v{latest}\"}}\\n'\n"),
            );
        }

        let value = check_json(home.path(), &bin, None);
        assert_eq!(value["schema_version"], 2);
        let report = tool_report(&value, tool);
        assert_eq!(report["status"], "outdated");
        assert_eq!(report["installation"]["source"], manager);
        assert_eq!(report["update"]["manager"], manager);
        assert_eq!(report["update"]["action"]["target_mode"], mode);
        assert_eq!(report["update"]["action"]["command"]["args"], expected_args);
    }
}

#[test]
fn homebrew_and_global_mise_manage_bun_and_deno_through_shared_claims() {
    for tool in ["bun", "deno"] {
        for manager in ["homebrew", "mise"] {
            let home = tempfile::tempdir().unwrap();
            let fixture = tempfile::tempdir().unwrap();
            let bin = if manager == "homebrew" {
                fixture.path().join("homebrew/bin")
            } else {
                fixture
                    .path()
                    .join(format!("mise/installs/{tool}/1.2.0/bin"))
            };
            write_executable(
                &bin.join(tool),
                &format!("#!/bin/sh\nprintf '{tool} 1.2.0\\n'\n"),
            );
            if manager == "homebrew" {
                write_executable(
                    &bin.join("brew"),
                    &format!(
                        "#!/bin/sh\nif [ \"$1\" = update ]; then exit 0;\nelif [ \"$1\" = list ] && [ \"$2\" = --formula ]; then printf '{tool} 1.2.0\\n';\nelif [ \"$1\" = list ] && [ \"$2\" = --cask ]; then exit 0;\nelif [ \"$1\" = --prefix ]; then printf '{}\\n';\nelif [ \"$1\" = info ]; then printf '{{\"formulae\":[{{\"versions\":{{\"stable\":\"1.2.1\"}}}}],\"casks\":[]}}\\n';\nelif [ \"$1\" = outdated ]; then printf '{{\"formulae\":[],\"casks\":[]}}\\n';\nelse exit 1; fi\n",
                        fixture.path().join("homebrew").display()
                    ),
                );
            } else {
                write_executable(
                    &bin.join("mise"),
                    &format!(
                        "#!/bin/sh\nif [ \"$1\" = ls ]; then printf '{{\"{tool}\":[{{\"version\":\"1.2.0\",\"requested_version\":\"latest\",\"install_path\":\"{}\"}}]}}\\n'; else printf '1.2.1\\n'; fi\n",
                        fixture
                            .path()
                            .join(format!("mise/installs/{tool}/1.2.0"))
                            .display()
                    ),
                );
            }

            let value = check_json(home.path(), &bin, None);
            let report = tool_report(&value, tool);
            assert_eq!(report["status"], "outdated", "{tool} via {manager}");
            assert_eq!(report["installation"]["source"], manager);
            assert_eq!(report["update"]["manager"], manager);
        }
    }
}

#[test]
fn bun_and_deno_stay_unmanaged_without_reliable_or_global_provenance() {
    for (tool, project_mise) in [("bun", false), ("deno", true)] {
        let home = tempfile::tempdir().unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let bin = if project_mise {
            fixture
                .path()
                .join(format!("mise/installs/{tool}/1.2.0/bin"))
        } else {
            fixture.path().join("custom/bin")
        };
        write_executable(
            &bin.join(tool),
            &format!("#!/bin/sh\nprintf '{tool} 1.2.0\\n'\n"),
        );
        if project_mise {
            write_executable(
                &bin.join("mise"),
                &format!(
                    "#!/bin/sh\nif [ \"$1\" = ls ]; then printf '{{\"{tool}\":[{{\"version\":\"1.2.0\",\"install_path\":\"{}\"}}]}}\\n'; else exit 99; fi\n",
                    fixture
                        .path()
                        .join(format!("mise/installs/{tool}/1.2.0"))
                        .display()
                ),
            );
            std::fs::write(
                project.path().join(".mise.toml"),
                format!("[tools]\n{tool} = \"1.2.0\"\n"),
            )
            .unwrap();
        }

        let value = check_json(home.path(), &bin, Some(project.path()));
        let report = tool_report(&value, tool);
        assert_eq!(report["status"], "unmanaged");
        assert!(report["update"].is_null());
        if project_mise {
            assert_eq!(report["installation"]["source"], "mise");
        } else {
            assert!(report["installation"]["source"].is_null());
        }
    }
}
fn uv_report(value: &serde_json::Value) -> &serde_json::Value {
    value["data"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == "uv")
        .unwrap()
}

#[test]
fn missing_uv_has_no_latest_or_upgrade_action() {
    let home = tempfile::tempdir().unwrap();
    let path = tempfile::tempdir().unwrap();

    let check = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .unwrap();

    assert!(check.status.success());
    let value: serde_json::Value = serde_json::from_slice(&check.stdout).unwrap();
    let uv = uv_report(&value);
    assert_eq!(uv["status"], "missing");
    assert!(uv["installation"].is_null());
    assert!(uv["update"].is_null());

    let upgrade = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["upgrade", "uv", "--yes", "--json"])
        .env("HOME", home.path())
        .env("PATH", path.path())
        .output()
        .unwrap();
    assert_eq!(upgrade.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&upgrade.stdout).unwrap();
    assert_eq!(value["status"], "error");
    assert!(value["data"].is_null());
}

#[test]
fn uv_without_a_reliable_updater_stays_unmanaged() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join(".local/bin");
    write_executable(
        &bin.join("uv"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'uv 0.6.0 (fixture)\\n'; exit 0; fi\nprintf 'self-update disabled\\n' >&2\nexit 1\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let uv = uv_report(&value);
    assert_eq!(uv["status"], "unmanaged");
    assert!(uv["installation"]["source"].is_null());
    assert!(uv["update"].is_null());
}

#[test]
fn uv_standalone_reports_latest_and_an_exact_action() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join(".local/bin");
    write_executable(
        &bin.join("uv"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'uv 0.6.0 (fixture)\\n'; else printf 'Would update uv from 0.6.0 to 0.7.0\\n'; fi\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let uv = uv_report(&value);
    assert_eq!(uv["status"], "outdated");
    assert_eq!(uv["installation"]["source"], "uv-standalone");
    assert_eq!(uv["update"]["manager"], "uv-standalone");
    assert_eq!(uv["update"]["latest"]["normalized"], "0.7.0");
    assert_eq!(uv["update"]["action"]["target_mode"], "exact");
    assert_eq!(
        uv["update"]["action"]["command"]["args"],
        serde_json::json!(["self", "update", "0.7.0"])
    );
}

#[test]
fn uv_diagnostic_installations_are_visible_but_unmanaged() {
    for (source, relative_bin) in [
        ("pip", ".venv/bin"),
        ("pipx", "pipx/venvs/uv/bin"),
        ("cargo", ".cargo/bin"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let bin = fixture.path().join(relative_bin);
        write_executable(
            &bin.join("uv"),
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'uv 0.6.0 (fixture)\\n'; exit 0; fi\nexit 1\n",
        );

        let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
            .args(["doctor", "uv", "--json"])
            .env("HOME", home.path())
            .env("PATH", &bin)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{source}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let uv = uv_report(&value);
        assert_eq!(uv["status"], "unmanaged", "{source}");
        assert_eq!(uv["installation"]["source"], source, "{source}");
        assert!(uv["update"].is_null(), "{source}");
        assert!(
            uv["diagnostics"]["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|evidence| evidence["claim"] == "source" && evidence["id"] == source),
            "{source}"
        );
    }
}

#[test]
fn global_mise_uv_preserves_its_selector() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let bin = fixture.path().join("mise/installs/uv/0.6/bin");
    write_executable(
        &bin.join("uv"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'uv 0.6.0 (fixture)\\n'; exit 0; fi\nexit 1\n",
    );
    write_executable(
        &bin.join("mise"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = ls ]; then printf '{{\"uv\":[{{\"version\":\"0.6.0\",\"requested_version\":\"0.6\",\"install_path\":\"{}\"}}]}}\\n'; else printf '0.6.1\\n'; fi\n",
            fixture.path().join("mise/installs/uv/0.6").display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let uv = uv_report(&value);
    assert_eq!(uv["installation"]["source"], "mise");
    assert_eq!(uv["update"]["manager"], "mise");
    assert_eq!(uv["update"]["action"]["target_mode"], "floating");
    assert_eq!(
        uv["update"]["action"]["command"]["args"],
        serde_json::json!(["use", "-g", "uv@0.6"])
    );
}

#[test]
fn homebrew_uv_reports_a_targeted_floating_action() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let prefix = fixture.path().join("homebrew");
    let bin = prefix.join("bin");
    write_executable(
        &bin.join("uv"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'uv 0.6.0 (fixture)\\n'; exit 0; fi\nexit 1\n",
    );
    write_executable(
        &bin.join("brew"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  update) exit 0 ;;\n  'list --formula --versions') printf 'uv 0.6.0\\n' ;;\n  'list --cask --versions') exit 0 ;;\n  --prefix) printf '{}\\n' ;;\n  'outdated --json=v2') printf '{{\"formulae\":[],\"casks\":[]}}\\n' ;;\n  'info --json=v2 uv') printf '{{\"formulae\":[{{\"versions\":{{\"stable\":\"0.7.0\"}}}}],\"casks\":[]}}\\n' ;;\n  'list --formula uv') printf '{}/bin/uv\\n' ;;\n  *) exit 1 ;;\nesac\n",
            prefix.display(),
            prefix.display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["check", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let uv = uv_report(&value);
    assert_eq!(uv["installation"]["source"], "homebrew");
    assert_eq!(uv["update"]["manager"], "homebrew");
    assert_eq!(uv["update"]["action"]["target_mode"], "floating");
    assert_eq!(
        uv["update"]["action"]["command"]["args"],
        serde_json::json!(["upgrade", "--formula", "uv"])
    );
}

#[test]
fn conflicting_uv_receipts_are_unmanaged_in_doctor_json() {
    let home = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let prefix = fixture.path().join("managed");
    let bin = prefix.join("bin");
    write_executable(
        &bin.join("uv"),
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'uv 0.6.0 (fixture)\\n'; exit 0; fi\nexit 1\n",
    );
    write_executable(
        &bin.join("mise"),
        &format!(
            "#!/bin/sh\nprintf '{{\"uv\":[{{\"version\":\"0.6.0\",\"requested_version\":\"latest\",\"install_path\":\"{}\"}}]}}\\n'\n",
            prefix.display()
        ),
    );
    write_executable(
        &bin.join("brew"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  'list --formula --versions') printf 'uv 0.6.0\\n' ;;\n  'list --cask --versions') exit 0 ;;\n  --prefix) printf '{}\\n' ;;\n  'outdated --json=v2') printf '{{\"formulae\":[],\"casks\":[]}}\\n' ;;\n  *) exit 1 ;;\nesac\n",
            prefix.display()
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_beacon"))
        .args(["doctor", "uv", "--json"])
        .env("HOME", home.path())
        .env("PATH", &bin)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let uv = uv_report(&value);
    assert_eq!(uv["status"], "unmanaged");
    assert!(uv["installation"]["source"].is_null());
    assert!(uv["update"].is_null());
    let conflicts = uv["diagnostics"]["conflicts"].as_array().unwrap();
    assert!(conflicts.iter().any(|item| item["id"] == "homebrew"));
    assert!(conflicts.iter().any(|item| item["id"] == "mise"));
}

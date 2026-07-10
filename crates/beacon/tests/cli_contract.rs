use std::process::Command;

#[cfg(unix)]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
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

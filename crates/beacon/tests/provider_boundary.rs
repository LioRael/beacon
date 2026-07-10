use anyhow::Result;
use async_trait::async_trait;
use beacon::{
    command::CommandSpec,
    config::Config,
    providers::{
        ClaimConfidence, CommandExecutor, ManagerClaims, ManagerEvidence, ManagerId,
        ManagerSnapshot, ProgressSink, ProviderContext, RefreshPolicy, SourceClaim, SourceId,
        TargetMode, ToolId, ToolVersion, UpdaterClaim, UpgradeAction, check_all_with_context,
        install_manager_registry, resolve_claims, tool_registry, verify_versions,
    },
    runner::CommandOutput,
};
use std::sync::Mutex;

#[derive(Default)]
struct FakeExecutor {
    calls: Mutex<Vec<(CommandSpec, u64)>>,
}

#[derive(Default)]
struct ProviderFakeExecutor {
    calls: Mutex<Vec<CommandSpec>>,
}

#[derive(Default)]
struct RustupFakeExecutor;

#[derive(Default)]
struct SharedMiseExecutor {
    calls: Mutex<Vec<CommandSpec>>,
}

#[async_trait]
impl CommandExecutor for SharedMiseExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        self.calls.lock().unwrap().push(command.clone());
        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args == ["node"] => {
                "/fixture/mise/installs/node/20/bin/node\n"
            }
            ("/usr/bin/which", args) if args == ["go"] => "/fixture/mise/installs/go/1.22/bin/go\n",
            ("/fixture/mise/installs/node/20/bin/node", args) if args == ["--version"] => {
                "v20.0.0\n"
            }
            ("/fixture/mise/installs/go/1.22/bin/go", args) if args == ["version"] => {
                "go version go1.22.0 darwin/arm64\n"
            }
            ("mise", args) if args == ["ls", "--json"] => {
                r#"{"node":[{"version":"20"}],"go":[{"version":"1.22"}]}"#
            }
            ("mise", args) if args == ["latest", "node@20"] => "20.1.0\n",
            ("mise", args) if args == ["latest", "go@1.22"] => "1.22.1\n",
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[async_trait]
impl CommandExecutor for RustupFakeExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        let stdout = match command.args.as_slice() {
            [show, active] if show == "show" && active == "active-toolchain" => {
                "stable-aarch64-apple-darwin (default)\n"
            }
            [check] if check == "check" => {
                "stable-aarch64-apple-darwin - Update available : 1.80.0 -> 1.81.0\n"
            }
            _ => anyhow::bail!("unexpected rustup command: {:?}", command.args),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[async_trait]
impl CommandExecutor for ProviderFakeExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        self.calls.lock().unwrap().push(command.clone());
        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args == ["brew"] => {
                anyhow::bail!("brew not found on fixture PATH")
            }
            ("/usr/bin/which", args) if args == ["npm"] => "/fixture/mise/installs/node/bin/npm\n",
            ("/fixture/mise/installs/node/bin/npm", args) if args == ["--version"] => "10.0.0\n",
            ("npm", args) if args == ["view", "npm", "version"] => "11.0.0\n",
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[async_trait]
impl CommandExecutor for FakeExecutor {
    async fn execute(&self, command: &CommandSpec, timeout_seconds: u64) -> Result<CommandOutput> {
        self.calls
            .lock()
            .unwrap()
            .push((command.clone(), timeout_seconds));
        let stdout = match command.program.as_str() {
            "/usr/bin/which" => "/fixture/bin/node\n",
            "/fixture/bin/node" => "v22.14.0\n",
            other => panic!("unexpected command: {other}"),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[derive(Default)]
struct RecordingProgress {
    events: Mutex<Vec<String>>,
}

impl ProgressSink for RecordingProgress {
    fn started(&self, label: &str) {
        self.events.lock().unwrap().push(format!("start:{label}"));
    }

    fn finished(&self, label: &str) {
        self.events.lock().unwrap().push(format!("finish:{label}"));
    }
}

#[tokio::test]
async fn adapter_detection_uses_the_injected_executor_and_reads_version_once() {
    let executor = FakeExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 17);
    let adapter = tool_registry()
        .iter()
        .find(|adapter| adapter.id().as_str() == "node")
        .unwrap();

    let detected = adapter.detect(&context).await.unwrap();

    assert_eq!(detected.id.as_str(), "node");
    assert_eq!(detected.executable, "/fixture/bin/node");
    assert_eq!(detected.version.raw(), "v22.14.0");
    assert_eq!(detected.version.display(), "22.14.0");
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, CommandSpec::new("/usr/bin/which", ["node"]));
    assert_eq!(
        calls[1].0,
        CommandSpec::new("/fixture/bin/node", ["--version"])
    );
    assert!(calls.iter().all(|(_, timeout)| *timeout == 17));
    assert_eq!(
        progress.events.lock().unwrap().as_slice(),
        [
            "start:Reading Node.js version",
            "finish:Reading Node.js version",
        ]
    );
}

#[tokio::test]
async fn provider_orchestration_uses_fake_path_and_commands_end_to_end() {
    let executor = ProviderFakeExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec!["npm".into()],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();

    assert_eq!(reports.tools.len(), 1);
    assert_eq!(reports.tools[0].id.as_str(), "npm");
    let installation = reports.tools[0].installation.as_ref().unwrap();
    let update = reports.tools[0].update.as_ref().unwrap();
    assert_eq!(installation.current.display(), "10.0.0");
    assert_eq!(installation.source.as_ref().unwrap().as_str(), "mise");
    assert_eq!(update.manager.as_str(), "npm");
    assert_eq!(update.latest.display(), "11.0.0");
    let calls = executor.calls.lock().unwrap();
    assert!(calls.contains(&CommandSpec::new("/usr/bin/which", ["brew"])));
    assert!(calls.contains(&CommandSpec::new("/usr/bin/which", ["npm"])));
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == CommandSpec::new("mise", ["ls", "--json"]))
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == CommandSpec::new("npm", ["prefix", "--global"]))
            .count(),
        1
    );
}

#[tokio::test]
async fn shared_manager_snapshot_runs_once_and_preserves_mise_selectors() {
    let executor = SharedMiseExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec!["node".into(), "go".into()],
        enabled_inventories: vec![],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, true, &context)
        .await
        .unwrap();

    assert_eq!(reports.tools.len(), 2);
    let calls = executor.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| **call == CommandSpec::new("mise", ["ls", "--json"]))
            .count(),
        1
    );
    let node = reports
        .tools
        .iter()
        .find(|report| report.id == "node")
        .unwrap();
    assert_eq!(
        node.update.as_ref().unwrap().action.command,
        CommandSpec::new("mise", ["use", "-g", "node@20"])
    );
}

#[test]
fn provider_contract_ids_and_upgrade_actions_are_validated_and_typed() {
    assert!(ToolId::new("Node JS").is_err());
    assert!(SourceId::new("").is_err());
    assert!(ManagerId::new("npm/global").is_err());
    assert!(serde_json::from_str::<ToolId>(r#""Node JS""#).is_err());

    let expected = ToolVersion::new("v2.0.0", Some("2.0.0".into())).unwrap();
    let action = UpgradeAction {
        manager: ManagerId::new("npm").unwrap(),
        command: CommandSpec::new("npm", ["install", "--global", "npm@2.0.0"]),
        expected_version: expected.clone(),
        target_mode: TargetMode::Exact,
    };

    assert_eq!(expected.display(), "2.0.0");
    assert_eq!(action.expected_version, expected);
    assert_eq!(action.target_mode, TargetMode::Exact);
}

#[test]
fn provider_registries_are_compile_time_built_ins() {
    let tools = tool_registry()
        .iter()
        .map(|adapter| adapter.id().to_string())
        .collect::<Vec<_>>();
    let managers = install_manager_registry()
        .iter()
        .map(|manager| manager.id().to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        tools,
        ["rust", "node", "npm", "pnpm", "go", "bun", "deno", "uv"]
    );
    assert_eq!(
        managers,
        [
            "homebrew",
            "mise",
            "rustup",
            "npm",
            "corepack",
            "bun-official",
            "deno-official",
            "uv-standalone",
        ]
    );
}

#[test]
fn claim_resolution_ranks_evidence_and_refuses_equal_top_claims() {
    let path = ManagerClaims {
        source: Some(SourceClaim {
            source: SourceId::new("mise").unwrap(),
            confidence: ClaimConfidence::PathHeuristic,
            evidence: "path contains mise".into(),
        }),
        updater: None,
    };
    let receipt = ManagerClaims {
        source: Some(SourceClaim {
            source: SourceId::new("homebrew").unwrap(),
            confidence: ClaimConfidence::Receipt,
            evidence: "brew receipt".into(),
        }),
        updater: Some(UpdaterClaim {
            manager: ManagerId::new("homebrew").unwrap(),
            confidence: ClaimConfidence::Receipt,
            evidence: "brew receipt".into(),
        }),
    };

    let resolved = resolve_claims([path.clone(), receipt.clone()]);
    assert_eq!(resolved.source.unwrap().source.as_str(), "homebrew");
    assert_eq!(resolved.updater.unwrap().manager.as_str(), "homebrew");
    assert!(resolved.conflicts.is_empty());

    let tied = resolve_claims([receipt.clone(), receipt]);
    assert!(tied.source.is_none());
    assert!(tied.updater.is_none());
    assert_eq!(tied.conflicts.len(), 4);
}

#[test]
fn ambiguous_source_does_not_block_a_unique_reliable_updater() {
    let updater = UpdaterClaim {
        manager: ManagerId::new("npm").unwrap(),
        confidence: ClaimConfidence::Receipt,
        evidence: "npm global receipt".into(),
    };
    let resolved = resolve_claims([
        ManagerClaims {
            source: Some(SourceClaim {
                source: SourceId::new("mise").unwrap(),
                confidence: ClaimConfidence::CanonicalPath,
                evidence: "mise path".into(),
            }),
            updater: Some(updater),
        },
        ManagerClaims {
            source: Some(SourceClaim {
                source: SourceId::new("homebrew").unwrap(),
                confidence: ClaimConfidence::CanonicalPath,
                evidence: "brew path".into(),
            }),
            updater: None,
        },
    ]);

    assert!(resolved.source.is_none());
    assert_eq!(resolved.updater.unwrap().manager.as_str(), "npm");
}

#[test]
fn path_linked_receipt_outranks_heuristics_and_selects_the_exact_brew_kind() {
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id().as_str() == "homebrew")
        .unwrap();
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("node").unwrap(),
        executable: "/custom/bin/node".into(),
        version: ToolVersion::new("20.0.0", Some("20.0.0".into())).unwrap(),
    };
    let snapshot = ManagerSnapshot {
        manager: ManagerId::new("homebrew").unwrap(),
        evidence: vec![
            ManagerEvidence {
                kind: "receipt:formula".into(),
                value: "node /custom/bin/node".into(),
            },
            ManagerEvidence {
                kind: "receipt:cask".into(),
                value: "node /other/bin/node".into(),
            },
        ],
    };

    let claims = manager.claim(&tool, &snapshot);
    let action = manager
        .upgrade(
            &tool,
            &ToolVersion::new("20.1.0", Some("20.1.0".into())).unwrap(),
            &snapshot,
        )
        .unwrap();

    assert_eq!(claims.source.unwrap().confidence, ClaimConfidence::Receipt);
    assert_eq!(
        action.command,
        CommandSpec::new("brew", ["upgrade", "--formula", "node"])
    );
}

#[test]
fn exact_and_floating_verification_follow_confirmed_versions() {
    let old = ToolVersion::new("1.0.0", Some("1.0.0".into())).unwrap();
    let expected = ToolVersion::new("2.0.0", Some("2.0.0".into())).unwrap();
    let exact_wrong = ToolVersion::new("2.0.1", Some("2.0.1".into())).unwrap();
    let floating_ok = ToolVersion::new("2.1.0", Some("2.1.0".into())).unwrap();
    let compare = |a: &ToolVersion, b: &ToolVersion| {
        Ok(semver::Version::parse(a.display())?.cmp(&semver::Version::parse(b.display())?))
    };

    assert!(verify_versions(TargetMode::Exact, &old, &expected, &expected, compare).is_ok());
    assert!(verify_versions(TargetMode::Exact, &old, &expected, &exact_wrong, compare).is_err());
    assert!(verify_versions(TargetMode::Floating, &old, &expected, &floating_ok, compare).is_ok());
    assert!(verify_versions(TargetMode::Floating, &old, &expected, &old, compare).is_err());
}

#[tokio::test]
async fn rustup_manager_retains_the_active_channel_for_latest_and_upgrade() {
    let executor = RustupFakeExecutor;
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 5);
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id().as_str() == "rustup")
        .unwrap();
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("rust").unwrap(),
        executable: "/Users/alice/.cargo/bin/rustc".into(),
        version: ToolVersion::new("1.80.0", Some("1.80.0".into())).unwrap(),
    };

    let snapshot = manager
        .snapshot(&context, RefreshPolicy::Cached)
        .await
        .unwrap();
    let latest = manager.latest(&tool, &snapshot, &context).await.unwrap();
    let action = manager.upgrade(&tool, &latest, &snapshot).unwrap();

    assert_eq!(latest.display(), "1.81.0");
    assert_eq!(
        action.command,
        CommandSpec::new("rustup", ["update", "stable-aarch64-apple-darwin"])
    );
}

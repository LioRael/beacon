use anyhow::Result;
use async_trait::async_trait;
use beacon::{
    command::CommandSpec,
    config::Config,
    providers::{
        ClaimConfidence, CommandExecutor, MAX_INDEPENDENT_DETECTION_CONCURRENCY, ManagerClaims,
        ManagerEvidence, ManagerId, ManagerSnapshot, ProgressSink, ProviderContext, RefreshPolicy,
        SourceClaim, SourceId, TargetMode, ToolId, ToolVersion, UpdaterClaim, UpgradeAction,
        check_all_with_context, install_manager_registry, resolve_claims, tool_registry,
        verify_versions,
    },
    runner::CommandOutput,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

struct MismatchedRustupExecutor;
struct FailingRustupExecutor;

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
                r#"{"node":[{"version":"20","requested_version":"20","install_path":"/fixture/mise/installs/node/20"}],"go":[{"version":"1.23","requested_version":"latest","install_path":"/fixture/mise/installs/go/1.23"},{"version":"1.22","requested_version":"1.22","install_path":"/fixture/mise/installs/go/1.22"}]}"#
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
            [which, rustc, toolchain, channel]
                if which == "which"
                    && rustc == "rustc"
                    && toolchain == "--toolchain"
                    && channel == "stable-aarch64-apple-darwin" =>
            {
                "/Users/alice/.rustup/toolchains/stable-aarch64-apple-darwin/bin/rustc\n"
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
impl CommandExecutor for MismatchedRustupExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args == ["rustc"] => "/Users/alice/.cargo/bin/rustc\n",
            ("/Users/alice/.cargo/bin/rustc", args) if args == ["--version"] => {
                "rustc 1.80.0 (fixture)\n"
            }
            ("/usr/bin/which", _) => anyhow::bail!("manager unavailable"),
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[async_trait]
impl CommandExecutor for FailingRustupExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args == ["rustc"] => "/Users/alice/.cargo/bin/rustc\n",
            ("/Users/alice/.cargo/bin/rustc", args) if args == ["--version"] => {
                "rustc 1.80.0 (fixture)\n"
            }
            ("/usr/bin/which", args) if args == ["rustup"] => "/fixture/bin/rustup\n",
            ("rustup", args) if args == ["show", "active-toolchain"] => {
                anyhow::bail!("rustup state unavailable")
            }
            ("/usr/bin/which", _) => anyhow::bail!("manager unavailable"),
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
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
    assert_eq!(
        update.action.command,
        CommandSpec::new("npm", ["install", "--global", "npm@11.0.0"])
    );
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
async fn missing_tool_never_queries_latest_or_builds_an_update() {
    let executor = ProviderFakeExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec!["node".into()],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, true, &context)
        .await
        .unwrap();

    assert_eq!(reports.tools[0].status, beacon::ToolStatus::Missing);
    assert!(reports.tools[0].update.is_none());
    assert!(executor.calls.lock().unwrap().iter().all(|call| {
        !(call.program == "mise" && call.args.first().is_some_and(|arg| arg == "latest"))
            && !(call.program == "npm" && call.args.first().is_some_and(|arg| arg == "view"))
    }));
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
    let go = reports
        .tools
        .iter()
        .find(|report| report.id == "go")
        .unwrap();
    assert_eq!(
        go.installation
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .as_str(),
        "mise"
    );
    assert_eq!(
        go.update.as_ref().unwrap().action.command,
        CommandSpec::new("mise", ["use", "-g", "go@1.22"])
    );
}

#[test]
fn homebrew_go_action_always_targets_the_formula() {
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id().as_str() == "homebrew")
        .unwrap();
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("go").unwrap(),
        executable: "/opt/homebrew/bin/go".into(),
        version: ToolVersion::new("go1.22.0", Some("1.22.0".into())).unwrap(),
    };
    let snapshot = ManagerSnapshot {
        manager: ManagerId::new("homebrew").unwrap(),
        evidence: vec![ManagerEvidence {
            kind: "receipt:formula".into(),
            value: "go 1.22.0 /opt/homebrew/bin/go /opt/homebrew/opt/go".into(),
        }],
    };
    let latest = ToolVersion::new("1.23.0", Some("1.23.0".into())).unwrap();

    let claims = manager.claim(&tool, &snapshot);
    let action = manager.upgrade(&tool, &latest, &snapshot).unwrap();

    assert_eq!(claims.updater.unwrap().manager.as_str(), "homebrew");
    assert_eq!(
        action.command,
        CommandSpec::new("brew", ["upgrade", "--formula", "go"])
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
fn uv_adapter_parses_and_compares_versions() {
    let adapter = tool_registry()
        .iter()
        .find(|adapter| adapter.id().as_str() == "uv")
        .unwrap();
    let current = adapter
        .parse_version("uv 0.6.0 (fixture 2026-01-01)")
        .unwrap();
    let latest = adapter.parse_version("uv 0.7.0").unwrap();

    assert_eq!(current.raw(), "0.6.0");
    assert_eq!(current.display(), "0.6.0");
    assert_eq!(
        adapter.compare(&current, &latest).unwrap(),
        std::cmp::Ordering::Less
    );
}

#[test]
fn project_mise_uv_is_diagnostic_only() {
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id().as_str() == "mise")
        .unwrap();
    let executable = "/Users/alice/.local/share/mise/installs/uv/0.6/bin/uv";
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("uv").unwrap(),
        executable: executable.into(),
        version: ToolVersion::new("0.6.0", Some("0.6.0".into())).unwrap(),
    };
    let claims = manager.claim(
        &tool,
        &ManagerSnapshot {
            manager: manager.id(),
            evidence: vec![
                ManagerEvidence {
                    kind: "receipt".into(),
                    value: format!("uv {executable}"),
                },
                ManagerEvidence {
                    kind: "project-policy:uv".into(),
                    value: "project mise selection".into(),
                },
            ],
        },
    );

    assert_eq!(claims.source.unwrap().source.as_str(), "mise");
    assert!(claims.updater.is_none());
}

#[test]
fn conflicting_uv_receipts_produce_no_update_claim() {
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("uv").unwrap(),
        executable: "/custom/bin/uv".into(),
        version: ToolVersion::new("0.6.0", Some("0.6.0".into())).unwrap(),
    };
    let claims = ["homebrew", "mise"].map(|manager_id| {
        let manager = install_manager_registry()
            .iter()
            .find(|manager| manager.id().as_str() == manager_id)
            .unwrap();
        manager.claim(
            &tool,
            &ManagerSnapshot {
                manager: manager.id(),
                evidence: vec![ManagerEvidence {
                    kind: if manager_id == "homebrew" {
                        "receipt:formula".into()
                    } else {
                        "receipt".into()
                    },
                    value: "uv /custom/bin/uv".into(),
                }],
            },
        )
    });

    let resolved = resolve_claims(claims);
    assert!(resolved.source.is_none());
    assert!(resolved.updater.is_none());
    assert_eq!(resolved.conflicts.len(), 4);
}

#[test]
fn pnpm_supported_channels_choose_matching_source_updater_and_action() {
    struct Case {
        manager: &'static str,
        executable: &'static str,
        evidence: Vec<ManagerEvidence>,
        source: &'static str,
        command: CommandSpec,
        mode: TargetMode,
    }

    let cases = [
        Case {
            manager: "homebrew",
            executable: "/opt/homebrew/bin/pnpm",
            evidence: vec![ManagerEvidence {
                kind: "receipt:formula".into(),
                value: "pnpm 10.0.0 /opt/homebrew/bin/pnpm /opt/homebrew/opt/pnpm".into(),
            }],
            source: "homebrew",
            command: CommandSpec::new("brew", ["upgrade", "--formula", "pnpm"]),
            mode: TargetMode::Floating,
        },
        Case {
            manager: "mise",
            executable: "/Users/alice/.local/share/mise/installs/pnpm/10/bin/pnpm",
            evidence: vec![
                ManagerEvidence {
                    kind: "receipt".into(),
                    value: "pnpm /Users/alice/.local/share/mise/installs/pnpm/10/bin/pnpm".into(),
                },
                ManagerEvidence {
                    kind: "selector:pnpm".into(),
                    value: "lts".into(),
                },
            ],
            source: "mise",
            command: CommandSpec::new("mise", ["use", "-g", "pnpm@lts"]),
            mode: TargetMode::Floating,
        },
        Case {
            manager: "npm",
            executable: "/Users/alice/.npm-global/lib/node_modules/pnpm/bin/pnpm.cjs",
            evidence: vec![],
            source: "npm-global",
            command: CommandSpec::new("npm", ["install", "--global", "pnpm@10.1.0"]),
            mode: TargetMode::Exact,
        },
        Case {
            manager: "corepack",
            executable: "/Users/alice/.cache/node/corepack/shims/pnpm",
            evidence: vec![],
            source: "corepack",
            command: CommandSpec::new("corepack", ["prepare", "pnpm@10.1.0", "--activate"]),
            mode: TargetMode::Exact,
        },
    ];

    for case in cases {
        let manager = install_manager_registry()
            .iter()
            .find(|manager| manager.id().as_str() == case.manager)
            .unwrap();
        let tool = beacon::providers::DetectedTool {
            id: ToolId::new("pnpm").unwrap(),
            executable: case.executable.into(),
            version: ToolVersion::new("10.0.0", Some("10.0.0".into())).unwrap(),
        };
        let snapshot = ManagerSnapshot {
            manager: manager.id(),
            evidence: case.evidence,
        };
        let claims = manager.claim(&tool, &snapshot);
        let action = manager
            .upgrade(
                &tool,
                &ToolVersion::new("10.1.0", Some("10.1.0".into())).unwrap(),
                &snapshot,
            )
            .unwrap();

        assert_eq!(claims.source.unwrap().source.as_str(), case.source);
        assert_eq!(claims.updater.unwrap().manager.as_str(), case.manager);
        assert_eq!(action.command, case.command);
        assert_eq!(action.target_mode, case.mode);
    }
}

#[test]
fn project_policy_evidence_makes_node_ecosystem_claims_unmanaged() {
    for manager_id in ["mise", "npm", "corepack"] {
        let manager = install_manager_registry()
            .iter()
            .find(|manager| manager.id().as_str() == manager_id)
            .unwrap();
        let executable = match manager_id {
            "mise" => "/Users/alice/.local/share/mise/installs/pnpm/10/bin/pnpm",
            "npm" => "/Users/alice/.npm-global/lib/node_modules/pnpm/bin/pnpm.cjs",
            "corepack" => "/Users/alice/.cache/node/corepack/shims/pnpm",
            _ => unreachable!(),
        };
        let tool = beacon::providers::DetectedTool {
            id: ToolId::new("pnpm").unwrap(),
            executable: executable.into(),
            version: ToolVersion::new("10.0.0", Some("10.0.0".into())).unwrap(),
        };
        let mut evidence = vec![ManagerEvidence {
            kind: "project-policy:pnpm".into(),
            value: "project configuration owns pnpm selection".into(),
        }];
        if manager_id == "mise" {
            evidence.push(ManagerEvidence {
                kind: "receipt".into(),
                value: format!("pnpm {executable}"),
            });
        }
        let snapshot = ManagerSnapshot {
            manager: manager.id(),
            evidence,
        };

        let claims = manager.claim(&tool, &snapshot);

        assert!(
            claims.source.is_some(),
            "{manager_id} should remain diagnostic"
        );
        assert!(
            claims.updater.is_none(),
            "{manager_id} must not edit project policy"
        );
    }
}

#[test]
fn corepack_pnpm_inside_a_mise_node_runtime_is_not_claimed_as_mise_pnpm() {
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("pnpm").unwrap(),
        executable:
            "/Users/alice/.local/share/mise/installs/node/22/lib/node_modules/corepack/dist/pnpm.js"
                .into(),
        version: ToolVersion::new("10.0.0", Some("10.0.0".into())).unwrap(),
    };
    let claims = install_manager_registry().iter().filter_map(|manager| {
        let evidence = if manager.id().as_str() == "mise" {
            vec![ManagerEvidence {
                kind: "receipt".into(),
                value: "node /Users/alice/.local/share/mise/installs/node/22/bin/node".into(),
            }]
        } else {
            vec![]
        };
        let claims = manager.claim(
            &tool,
            &ManagerSnapshot {
                manager: manager.id(),
                evidence,
            },
        );
        (claims.source.is_some() || claims.updater.is_some()).then_some(claims)
    });

    let resolved = resolve_claims(claims);

    assert_eq!(resolved.source.unwrap().source.as_str(), "corepack");
    assert_eq!(resolved.updater.unwrap().manager.as_str(), "corepack");
    assert!(resolved.conflicts.is_empty());
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
fn claim_evidence_redacts_the_home_directory() {
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id().as_str() == "mise")
        .unwrap();
    let home = std::env::var("HOME").unwrap();
    let tool = beacon::providers::DetectedTool {
        id: ToolId::new("node").unwrap(),
        executable: format!("{home}/.local/share/mise/installs/node/22/bin/node"),
        version: ToolVersion::new("22.0.0", Some("22.0.0".into())).unwrap(),
    };

    let claims = manager.claim(
        &tool,
        &ManagerSnapshot {
            manager: ManagerId::new("mise").unwrap(),
            evidence: vec![],
        },
    );
    let evidence = claims.source.unwrap().evidence;

    assert!(evidence.contains("~/.local/share/mise/installs/node/22/bin/node"));
    assert!(!evidence.contains(&home));
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

#[tokio::test]
async fn rust_is_unmanaged_when_active_rustc_is_not_the_active_rustup_channels_binary() {
    let executor = MismatchedRustupExecutor;
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 5);
    let config = Config {
        enabled_tools: vec!["rust".into()],
        enabled_inventories: vec![],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();
    let rust = &reports.tools[0];

    assert_eq!(rust.status, beacon::ToolStatus::Unmanaged);
    assert!(rust.installation.is_some());
    assert!(rust.update.is_none());
}

#[tokio::test]
async fn rustup_query_failure_is_reported_as_failed_instead_of_unmanaged() {
    let executor = FailingRustupExecutor;
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 5);
    let config = Config {
        enabled_tools: vec!["rust".into()],
        enabled_inventories: vec![],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();
    let rust = &reports.tools[0];

    assert_eq!(rust.status, beacon::ToolStatus::Failed);
    assert!(rust.installation.is_some());
    assert!(rust.update.is_none());
    assert!(
        rust.detail
            .as_deref()
            .unwrap()
            .contains("manager query failed")
    );
}

/// Tracks how many independent tool version reads run at once.
struct ConcurrencyProbeExecutor {
    active: AtomicUsize,
    max_active: AtomicUsize,
    version_reads: AtomicUsize,
}

impl Default for ConcurrencyProbeExecutor {
    fn default() -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            version_reads: AtomicUsize::new(0),
        }
    }
}

impl ConcurrencyProbeExecutor {
    fn is_tool_version_read(command: &CommandSpec) -> bool {
        let path = command.program.as_str();
        let version_args = command.args == ["--version"] || command.args == ["version"];
        version_args
            && (path.ends_with("/node")
                || path.ends_with("/npm")
                || path.ends_with("/pnpm")
                || path.ends_with("/bun")
                || path.ends_with("/deno")
                || path.ends_with("/uv")
                || path.ends_with("/rustc")
                || path.ends_with("/go"))
    }
}

#[async_trait]
impl CommandExecutor for ConcurrencyProbeExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        let is_version = Self::is_tool_version_read(command);
        if is_version {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            self.version_reads.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
        }

        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args.len() == 1 => {
                format!("/fixture/bin/{}\n", args[0])
            }
            (path, args) if path.starts_with("/fixture/bin/") && args == ["--version"] => {
                match path {
                    "/fixture/bin/node" => "v20.0.0\n".into(),
                    "/fixture/bin/npm" => "10.0.0\n".into(),
                    "/fixture/bin/pnpm" => "9.0.0\n".into(),
                    "/fixture/bin/bun" => "1.1.0\n".into(),
                    "/fixture/bin/deno" => "deno 1.40.0 (release, aarch64-apple-darwin)\n".into(),
                    "/fixture/bin/uv" => "uv 0.6.0\n".into(),
                    "/fixture/bin/rustc" => "rustc 1.80.0 (fixture)\n".into(),
                    _ => "0.0.0\n".into(),
                }
            }
            ("/fixture/bin/go", args) if args == ["version"] => {
                "go version go1.22.0 darwin/arm64\n".into()
            }
            ("mise", args) if args == ["ls", "--json"] => "{}\n".into(),
            ("npm", args) if args == ["prefix", "--global"] => "/fixture/npm-global\n".into(),
            ("corepack", args) if args == ["--version"] => "0.24.0\n".into(),
            ("rustup", args) if args == ["show", "active-toolchain"] => {
                "stable-aarch64-apple-darwin (default)\n".into()
            }
            ("bun", args) if args == ["--version"] => "1.1.0\n".into(),
            ("deno", args) if args == ["--version"] => {
                "deno 1.40.0 (release, aarch64-apple-darwin)\n".into()
            }
            ("uv", args) if args == ["self", "update", "--dry-run"] => {
                "uv is already at the latest version 0.6.0\n".into()
            }
            ("npm", args) if args.first().map(String::as_str) == Some("view") => "11.0.0\n".into(),
            ("mise", args) if args.first().map(String::as_str) == Some("latest") => {
                "1.0.1\n".into()
            }
            ("rustup", args) if args == ["check"] => {
                "stable-aarch64-apple-darwin - Up to date : 1.80.0\n".into()
            }
            ("curl", _) => r#"{"tag_name":"bun-v1.1.0"}"#.into(),
            ("deno", args) if args.first().map(String::as_str) == Some("upgrade") => {
                "Current version: 1.40.0\nLatest version: 1.40.0\n".into()
            }
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout,
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn independent_tool_detection_never_exceeds_max_concurrency() {
    let executor = ConcurrencyProbeExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: tool_registry()
            .iter()
            .map(|adapter| adapter.id().to_string())
            .collect(),
        enabled_inventories: vec![],
        ..Config::default()
    };

    let _reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();

    let max = executor.max_active.load(Ordering::SeqCst);
    let reads = executor.version_reads.load(Ordering::SeqCst);
    assert!(
        reads >= MAX_INDEPENDENT_DETECTION_CONCURRENCY,
        "expected enough concurrent version reads to exercise the bound, got {reads}"
    );
    assert!(
        max <= MAX_INDEPENDENT_DETECTION_CONCURRENCY,
        "independent detection concurrency {max} exceeded bound {}",
        MAX_INDEPENDENT_DETECTION_CONCURRENCY
    );
    assert_eq!(MAX_INDEPENDENT_DETECTION_CONCURRENCY, 4);
}

/// Completes tools in reverse registry order so order is not completion-order by accident.
struct ReverseCompletionExecutor {
    calls: Mutex<Vec<CommandSpec>>,
}

impl Default for ReverseCompletionExecutor {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl CommandExecutor for ReverseCompletionExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        self.calls.lock().unwrap().push(command.clone());
        let is_version_read = command.args == ["--version"] || command.args == ["version"];
        if is_version_read {
            let delay_ms = if command.program.ends_with("/node") {
                5
            } else if command.program.ends_with("/go") {
                1
            } else if command.program.ends_with("/npm") {
                3
            } else {
                2
            };
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }

        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args.len() == 1 => {
                format!("/fixture/bin/{}\n", args[0])
            }
            ("/fixture/bin/node", args) if args == ["--version"] => "v20.0.0\n".into(),
            ("/fixture/bin/npm", args) if args == ["--version"] => "10.0.0\n".into(),
            ("/fixture/bin/go", args) if args == ["version"] => {
                "go version go1.22.0 darwin/arm64\n".into()
            }
            ("mise", args) if args == ["ls", "--json"] => {
                r#"{"node":[{"version":"20","requested_version":"20","install_path":"/fixture/bin"}],"go":[{"version":"1.22","requested_version":"1.22","install_path":"/fixture/bin"}]}"#.into()
            }
            ("mise", args) if args.first().map(String::as_str) == Some("latest") => {
                match args.get(1).map(String::as_str) {
                    Some("node@20") => "20.1.0\n".into(),
                    Some("go@1.22") => "1.22.1\n".into(),
                    _ => "1.0.1\n".into(),
                }
            }
            ("npm", args) if args == ["prefix", "--global"] => "/fixture/npm-global\n".into(),
            ("npm", args) if args == ["view", "npm", "version"] => "11.0.0\n".into(),
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout,
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn tool_reports_preserve_compile_time_registry_order() {
    let executor = ReverseCompletionExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec!["go".into(), "node".into(), "npm".into()],
        enabled_inventories: vec![],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();

    let ids = reports
        .tools
        .iter()
        .map(|report| report.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["node", "npm", "go"]);
}

struct RefreshPolicyExecutor {
    calls: Mutex<Vec<CommandSpec>>,
}

impl Default for RefreshPolicyExecutor {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl CommandExecutor for RefreshPolicyExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        self.calls.lock().unwrap().push(command.clone());
        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args == ["brew"] => "/fixture/bin/brew\n",
            ("/usr/bin/which", _) => anyhow::bail!("not on path"),
            ("brew", args) if args == ["update"] => "",
            ("brew", args) if args == ["list", "--formula", "--versions"] => "wget 1.21.4\n",
            ("brew", args) if args == ["list", "--cask", "--versions"] => "firefox 120.0\n",
            ("brew", args) if args == ["--prefix"] => "/opt/homebrew\n",
            ("brew", args) if args == ["outdated", "--json=v2"] => {
                r#"{"formulae":[{"name":"zlib","installed_versions":["1.2"],"current_version":"1.3"},{"name":"wget","installed_versions":["1.21.3"],"current_version":"1.21.4"}],"casks":[{"name":"firefox","installed_versions":["119.0"],"current_version":"120.0"}]}"#
            }
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn check_refresh_runs_brew_update_while_doctor_uses_cached_reads() {
    let progress = RecordingProgress::default();

    let refresh_executor = RefreshPolicyExecutor::default();
    let refresh_context = ProviderContext::new(&refresh_executor, &progress, 9);
    let config = Config {
        enabled_tools: vec![],
        enabled_inventories: vec!["homebrew".into()],
        ..Config::default()
    };
    let _ = check_all_with_context(&config, true, &refresh_context)
        .await
        .unwrap();
    let refresh_calls = refresh_executor.calls.lock().unwrap().clone();
    assert!(
        refresh_calls
            .iter()
            .any(|call| *call == CommandSpec::new("brew", ["update"])),
        "check/upgrade preparation must force manager refresh"
    );

    let cached_executor = RefreshPolicyExecutor::default();
    let cached_context = ProviderContext::new(&cached_executor, &progress, 9);
    let _ = check_all_with_context(&config, false, &cached_context)
        .await
        .unwrap();
    let cached_calls = cached_executor.calls.lock().unwrap().clone();
    assert!(
        cached_calls
            .iter()
            .all(|call| *call != CommandSpec::new("brew", ["update"])),
        "doctor must avoid state-mutating refresh"
    );
    assert!(
        cached_calls
            .iter()
            .any(|call| *call == CommandSpec::new("brew", ["list", "--formula", "--versions"])),
        "doctor may still perform read-only manager queries"
    );
}

#[tokio::test]
async fn inventory_items_use_stable_sorted_order() {
    let executor = RefreshPolicyExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec![],
        enabled_inventories: vec!["homebrew".into()],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();
    let ids = reports
        .inventories
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "brew:cask:firefox",
            "brew:formula:wget",
            "brew:formula:zlib",
        ]
    );
}

struct IsolationExecutor {
    calls: Mutex<Vec<CommandSpec>>,
}

impl Default for IsolationExecutor {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl CommandExecutor for IsolationExecutor {
    async fn execute(&self, command: &CommandSpec, _timeout_seconds: u64) -> Result<CommandOutput> {
        self.calls.lock().unwrap().push(command.clone());
        let stdout = match (command.program.as_str(), command.args.as_slice()) {
            ("/usr/bin/which", args) if args == ["node"] => {
                "/fixture/mise/installs/node/20/bin/node\n"
            }
            ("/usr/bin/which", args) if args == ["rustc"] => "/Users/alice/.cargo/bin/rustc\n",
            ("/usr/bin/which", args) if args == ["mise"] => "/fixture/bin/mise\n",
            ("/usr/bin/which", args) if args == ["rustup"] => "/fixture/bin/rustup\n",
            ("/usr/bin/which", _) => anyhow::bail!("not on path"),
            ("/fixture/mise/installs/node/20/bin/node", args) if args == ["--version"] => {
                "v20.0.0\n"
            }
            ("/Users/alice/.cargo/bin/rustc", args) if args == ["--version"] => {
                "rustc 1.80.0 (fixture)\n"
            }
            ("mise", args) if args == ["ls", "--json"] => {
                r#"{"node":[{"version":"20","requested_version":"20","install_path":"/fixture/mise/installs/node/20"}]}"#
            }
            ("mise", args) if args == ["latest", "node@20"] => "20.1.0\n",
            ("rustup", args) if args == ["show", "active-toolchain"] => {
                anyhow::bail!("rustup state unavailable")
            }
            _ => anyhow::bail!("unexpected command: {} {:?}", command.program, command.args),
        };
        Ok(CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn manager_failure_only_affects_dependent_tool_reports() {
    let executor = IsolationExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec!["rust".into(), "node".into()],
        enabled_inventories: vec![],
        ..Config::default()
    };

    let reports = check_all_with_context(&config, false, &context)
        .await
        .unwrap();

    assert_eq!(reports.tools.len(), 2);
    let rust = reports
        .tools
        .iter()
        .find(|report| report.id == "rust")
        .unwrap();
    let node = reports
        .tools
        .iter()
        .find(|report| report.id == "node")
        .unwrap();

    assert_eq!(rust.status, beacon::ToolStatus::Failed);
    assert!(rust.update.is_none());
    assert!(
        rust.detail
            .as_deref()
            .unwrap_or_default()
            .contains("manager query failed")
    );

    assert_ne!(node.status, beacon::ToolStatus::Failed);
    assert_eq!(
        node.installation
            .as_ref()
            .unwrap()
            .source
            .as_ref()
            .unwrap()
            .as_str(),
        "mise"
    );
    assert!(node.update.is_some());
}

#[tokio::test]
async fn shared_manager_snapshot_is_created_at_most_once_per_operation() {
    let executor = SharedMiseExecutor::default();
    let progress = RecordingProgress::default();
    let context = ProviderContext::new(&executor, &progress, 9);
    let config = Config {
        enabled_tools: vec!["node".into(), "go".into()],
        enabled_inventories: vec![],
        ..Config::default()
    };

    let _ = check_all_with_context(&config, true, &context)
        .await
        .unwrap();

    let snapshot_calls = executor
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| **call == CommandSpec::new("mise", ["ls", "--json"]))
        .count();
    assert_eq!(snapshot_calls, 1);
}

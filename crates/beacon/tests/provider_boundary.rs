use anyhow::Result;
use async_trait::async_trait;
use beacon::{
    command::CommandSpec,
    config::Config,
    providers::{
        CommandExecutor, ManagerId, ProgressSink, ProviderContext, RefreshPolicy, SourceId,
        TargetMode, ToolId, ToolVersion, UpgradeAction, check_all_with_context,
        install_manager_registry, tool_registry,
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

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].id, "npm");
    assert_eq!(reports[0].current.as_deref(), Some("10.0.0"));
    assert_eq!(reports[0].latest.as_deref(), Some("11.0.0"));
    let calls = executor.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0], CommandSpec::new("/usr/bin/which", ["brew"]));
    assert_eq!(calls[1], CommandSpec::new("/usr/bin/which", ["npm"]));
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

    assert_eq!(tools, ["rust", "node", "npm", "pnpm", "go"]);
    assert_eq!(managers, ["homebrew", "mise", "rustup", "npm"]);
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

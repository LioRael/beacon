use crate::{
    Manager, ToolReport, ToolStatus,
    command::CommandSpec,
    config::Config,
    ui::Ui,
    versions::{manager_for_executable, version_number},
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
};

macro_rules! validated_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                let valid = !value.is_empty()
                    && value.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_' | b'.' | b':')
                    });
                if !valid {
                    anyhow::bail!("invalid provider id `{value}`");
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

validated_id!(ToolId);
validated_id!(SourceId);
validated_id!(ManagerId);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolVersion {
    raw: String,
    normalized: Option<String>,
}

impl ToolVersion {
    pub fn new(raw: impl Into<String>, normalized: Option<String>) -> Result<Self> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            anyhow::bail!("tool version cannot be empty");
        }
        if normalized
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            anyhow::bail!("normalized tool version cannot be empty");
        }
        Ok(Self { raw, normalized })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn normalized(&self) -> Option<&str> {
        self.normalized.as_deref()
    }

    pub fn display(&self) -> &str {
        self.normalized.as_deref().unwrap_or(&self.raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetMode {
    Exact,
    Floating,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpgradeAction {
    pub manager: ManagerId,
    pub command: CommandSpec,
    pub expected_version: ToolVersion,
    pub target_mode: TargetMode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DetectedTool {
    pub id: ToolId,
    pub executable: String,
    pub version: ToolVersion,
}

#[async_trait]
pub trait CommandExecutor: Send + Sync {
    async fn execute(
        &self,
        command: &CommandSpec,
        timeout_seconds: u64,
    ) -> Result<crate::runner::CommandOutput>;
}

pub trait ProgressSink: Send + Sync {
    fn started(&self, _label: &str) {}
    fn finished(&self, _label: &str) {}
}

pub struct ProviderContext<'a> {
    executor: &'a dyn CommandExecutor,
    progress: &'a dyn ProgressSink,
    timeout_seconds: u64,
}

impl<'a> ProviderContext<'a> {
    pub fn new(
        executor: &'a dyn CommandExecutor,
        progress: &'a dyn ProgressSink,
        timeout_seconds: u64,
    ) -> Self {
        Self {
            executor,
            progress,
            timeout_seconds,
        }
    }

    pub async fn execute(
        &self,
        label: &str,
        command: &CommandSpec,
    ) -> Result<crate::runner::CommandOutput> {
        self.progress.started(label);
        let result = self
            .executor
            .execute(command, self.timeout_seconds)
            .await
            .map_err(|error| {
                let home = std::env::var("HOME").ok();
                anyhow::anyhow!(crate::redact::redact(
                    &format!("{error:#}"),
                    home.as_deref()
                ))
            });
        self.progress.finished(label);
        result
    }

    async fn execute_silent(&self, command: &CommandSpec) -> Result<crate::runner::CommandOutput> {
        self.executor
            .execute(command, self.timeout_seconds)
            .await
            .map_err(|error| {
                let home = std::env::var("HOME").ok();
                anyhow::anyhow!(crate::redact::redact(
                    &format!("{error:#}"),
                    home.as_deref()
                ))
            })
    }
}

#[async_trait]
pub trait ToolAdapter: Send + Sync {
    fn id(&self) -> ToolId;
    fn display_name(&self) -> &'static str;
    async fn detect(&self, context: &ProviderContext<'_>) -> Result<DetectedTool>;
    fn parse_version(&self, output: &str) -> Result<ToolVersion>;
    fn compare(&self, current: &ToolVersion, latest: &ToolVersion) -> Result<Ordering>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefreshPolicy {
    Cached,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerEvidence {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerSnapshot {
    pub manager: ManagerId,
    pub evidence: Vec<ManagerEvidence>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ClaimConfidence {
    PathHeuristic,
    CanonicalPath,
    Receipt,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceClaim {
    pub source: SourceId,
    pub confidence: ClaimConfidence,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdaterClaim {
    pub manager: ManagerId,
    pub confidence: ClaimConfidence,
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ManagerClaims {
    pub source: Option<SourceClaim>,
    pub updater: Option<UpdaterClaim>,
}

#[async_trait]
pub trait InstallManager: Send + Sync {
    fn id(&self) -> ManagerId;
    async fn snapshot(
        &self,
        context: &ProviderContext<'_>,
        refresh: RefreshPolicy,
    ) -> Result<ManagerSnapshot>;
    fn claim(&self, tool: &DetectedTool, snapshot: &ManagerSnapshot) -> ManagerClaims;
    async fn latest(
        &self,
        tool: &DetectedTool,
        snapshot: &ManagerSnapshot,
        context: &ProviderContext<'_>,
    ) -> Result<ToolVersion>;
    fn upgrade(
        &self,
        tool: &DetectedTool,
        latest: &ToolVersion,
        snapshot: &ManagerSnapshot,
    ) -> Result<UpgradeAction>;
}

struct BuiltinToolAdapter {
    id: &'static str,
    display_name: &'static str,
    executable: &'static str,
    version_args: &'static [&'static str],
}

#[async_trait]
impl ToolAdapter for BuiltinToolAdapter {
    fn id(&self) -> ToolId {
        ToolId::new(self.id).expect("built-in tool IDs are valid")
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    async fn detect(&self, context: &ProviderContext<'_>) -> Result<DetectedTool> {
        let location = context
            .execute_silent(&CommandSpec::new("/usr/bin/which", [self.executable]))
            .await?;
        let observed_path = location
            .stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .context("tool is not available on PATH")?
            .to_string();
        let observed = context
            .execute(
                &format!("Reading {} version", self.display_name),
                &CommandSpec::new(&observed_path, self.version_args.iter().copied()),
            )
            .await?;
        let version = self.parse_version(&observed.stdout)?;
        Ok(DetectedTool {
            id: self.id(),
            executable: observed_path,
            version,
        })
    }

    fn parse_version(&self, output: &str) -> Result<ToolVersion> {
        let normalized =
            version_number(output).context("version output did not contain a version")?;
        let raw = output
            .split_whitespace()
            .find(|token| token.contains(&normalized))
            .unwrap_or(&normalized)
            .trim_matches(|character: char| {
                !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+' | '_'))
            })
            .to_string();
        ToolVersion::new(raw, Some(normalized))
    }

    fn compare(&self, current: &ToolVersion, latest: &ToolVersion) -> Result<Ordering> {
        let current = semver::Version::parse(current.display())?;
        let latest = semver::Version::parse(latest.display())?;
        Ok(current.cmp(&latest))
    }
}

static RUST_ADAPTER: BuiltinToolAdapter = BuiltinToolAdapter {
    id: "rust",
    display_name: "Rust",
    executable: "rustc",
    version_args: &["--version"],
};
static NODE_ADAPTER: BuiltinToolAdapter = BuiltinToolAdapter {
    id: "node",
    display_name: "Node.js",
    executable: "node",
    version_args: &["--version"],
};
static NPM_ADAPTER: BuiltinToolAdapter = BuiltinToolAdapter {
    id: "npm",
    display_name: "npm",
    executable: "npm",
    version_args: &["--version"],
};
static PNPM_ADAPTER: BuiltinToolAdapter = BuiltinToolAdapter {
    id: "pnpm",
    display_name: "pnpm",
    executable: "pnpm",
    version_args: &["--version"],
};
static GO_ADAPTER: BuiltinToolAdapter = BuiltinToolAdapter {
    id: "go",
    display_name: "Go",
    executable: "go",
    version_args: &["version"],
};
static TOOL_REGISTRY: [&'static dyn ToolAdapter; 5] = [
    &RUST_ADAPTER,
    &NODE_ADAPTER,
    &NPM_ADAPTER,
    &PNPM_ADAPTER,
    &GO_ADAPTER,
];

pub fn tool_registry() -> &'static [&'static dyn ToolAdapter] {
    &TOOL_REGISTRY
}

async fn detect_tool(id: &str, context: &ProviderContext<'_>) -> Option<DetectedTool> {
    let adapter = tool_registry()
        .iter()
        .find(|adapter| adapter.id().as_str() == id)?;
    adapter.detect(context).await.ok()
}

#[derive(Clone, Copy)]
enum BuiltinManagerKind {
    Homebrew,
    Mise,
    Rustup,
    Npm,
}

struct BuiltinInstallManager {
    id: &'static str,
    kind: BuiltinManagerKind,
}

impl BuiltinInstallManager {
    fn snapshot_command(&self, refresh: RefreshPolicy) -> CommandSpec {
        match (self.kind, refresh) {
            (BuiltinManagerKind::Homebrew, RefreshPolicy::Refresh) => {
                CommandSpec::new("brew", ["update"])
            }
            (BuiltinManagerKind::Homebrew, RefreshPolicy::Cached) => {
                CommandSpec::new("brew", ["--prefix"])
            }
            (BuiltinManagerKind::Mise, _) => CommandSpec::new("mise", ["data", "dir"]),
            (BuiltinManagerKind::Rustup, _) => {
                CommandSpec::new("rustup", ["show", "active-toolchain"])
            }
            (BuiltinManagerKind::Npm, _) => CommandSpec::new("npm", ["prefix", "--global"]),
        }
    }

    fn owns_path(&self, path: &str) -> bool {
        let canonical = std::fs::canonicalize(path)
            .unwrap_or_else(|_| PathBuf::from(path))
            .to_string_lossy()
            .into_owned();
        match self.kind {
            BuiltinManagerKind::Homebrew => canonical.contains("/homebrew/"),
            BuiltinManagerKind::Mise => canonical.contains("/mise/"),
            BuiltinManagerKind::Rustup => {
                canonical.contains("/.cargo/") || canonical.contains("/rustup/")
            }
            BuiltinManagerKind::Npm => false,
        }
    }
}

#[async_trait]
impl InstallManager for BuiltinInstallManager {
    fn id(&self) -> ManagerId {
        ManagerId::new(self.id).expect("built-in manager IDs are valid")
    }

    async fn snapshot(
        &self,
        context: &ProviderContext<'_>,
        refresh: RefreshPolicy,
    ) -> Result<ManagerSnapshot> {
        let output = context
            .execute(
                &format!("Reading {} manager state", self.id),
                &self.snapshot_command(refresh),
            )
            .await?;
        Ok(ManagerSnapshot {
            manager: self.id(),
            evidence: vec![ManagerEvidence {
                kind: if matches!(self.kind, BuiltinManagerKind::Rustup) {
                    "active-toolchain".into()
                } else {
                    "manager-output".into()
                },
                value: crate::redact::redact(
                    output.stdout.trim(),
                    std::env::var("HOME").ok().as_deref(),
                ),
            }],
        })
    }

    fn claim(&self, tool: &DetectedTool, snapshot: &ManagerSnapshot) -> ManagerClaims {
        let path_owned = self.owns_path(&tool.executable);
        let npm_updates_itself =
            matches!(self.kind, BuiltinManagerKind::Npm) && tool.id.as_str() == "npm";
        let source = path_owned.then(|| SourceClaim {
            source: SourceId::new(self.id).expect("built-in source IDs are valid"),
            confidence: ClaimConfidence::CanonicalPath,
            evidence: "active executable canonical path".into(),
        });
        let updater = (path_owned || npm_updates_itself).then(|| UpdaterClaim {
            manager: self.id(),
            confidence: if path_owned {
                ClaimConfidence::CanonicalPath
            } else {
                ClaimConfidence::PathHeuristic
            },
            evidence: if npm_updates_itself && !snapshot.evidence.is_empty() {
                "npm manager snapshot and active npm executable".into()
            } else {
                "active executable canonical path".into()
            },
        });
        ManagerClaims { source, updater }
    }

    async fn latest(
        &self,
        tool: &DetectedTool,
        snapshot: &ManagerSnapshot,
        context: &ProviderContext<'_>,
    ) -> Result<ToolVersion> {
        let command = match self.kind {
            BuiltinManagerKind::Homebrew => {
                CommandSpec::new("brew", ["info", "--json=v2", tool.id.as_str()])
            }
            BuiltinManagerKind::Mise => CommandSpec::new("mise", ["latest", tool.id.as_str()]),
            BuiltinManagerKind::Rustup => CommandSpec::new("rustup", ["check"]),
            BuiltinManagerKind::Npm => {
                CommandSpec::new("npm", ["view", tool.id.as_str(), "version"])
            }
        };
        let output = context
            .execute(&format!("Checking latest {} version", tool.id), &command)
            .await?;
        let normalized = match self.kind {
            BuiltinManagerKind::Homebrew => {
                let document: serde_json::Value = serde_json::from_str(&output.stdout)
                    .context("invalid Homebrew info response")?;
                document["formulae"]
                    .as_array()
                    .and_then(|items| items.first())
                    .and_then(|item| item["versions"]["stable"].as_str())
                    .or_else(|| {
                        document["casks"]
                            .as_array()
                            .and_then(|items| items.first())
                            .and_then(|item| item["version"].as_str())
                    })
                    .map(str::to_string)
                    .context("Homebrew info response had no latest version")?
            }
            BuiltinManagerKind::Rustup => {
                let channel = snapshot
                    .evidence
                    .iter()
                    .find(|evidence| evidence.kind == "active-toolchain")
                    .and_then(|evidence| evidence.value.split_whitespace().next())
                    .context("rustup snapshot had no active toolchain")?;
                output
                    .stdout
                    .lines()
                    .find(|line| line.starts_with(channel))
                    .and_then(|line| {
                        line.split_whitespace()
                            .filter_map(version_number)
                            .next_back()
                    })
                    .context("rustup check had no update for the active toolchain")?
            }
            BuiltinManagerKind::Mise | BuiltinManagerKind::Npm => {
                version_number(&output.stdout).context("latest output had no version")?
            }
        };
        ToolVersion::new(normalized.clone(), Some(normalized))
    }

    fn upgrade(
        &self,
        tool: &DetectedTool,
        latest: &ToolVersion,
        snapshot: &ManagerSnapshot,
    ) -> Result<UpgradeAction> {
        let (command, target_mode) = match self.kind {
            BuiltinManagerKind::Homebrew => (
                CommandSpec::brew_upgrade(tool.id.as_str())?,
                TargetMode::Floating,
            ),
            BuiltinManagerKind::Mise => (
                CommandSpec::new("mise", ["use", "-g", &format!("{}@latest", tool.id)]),
                TargetMode::Floating,
            ),
            BuiltinManagerKind::Rustup => (
                CommandSpec::new(
                    "rustup",
                    [
                        "update",
                        snapshot
                            .evidence
                            .iter()
                            .find(|evidence| evidence.kind == "active-toolchain")
                            .and_then(|evidence| evidence.value.split_whitespace().next())
                            .context("rustup snapshot had no active toolchain")?,
                    ],
                ),
                TargetMode::Floating,
            ),
            BuiltinManagerKind::Npm => (
                CommandSpec::new(
                    "npm",
                    [
                        "install",
                        "--global",
                        &format!("{}@{}", tool.id, latest.display()),
                    ],
                ),
                TargetMode::Exact,
            ),
        };
        Ok(UpgradeAction {
            manager: self.id(),
            command,
            expected_version: latest.clone(),
            target_mode,
        })
    }
}

static HOMEBREW_MANAGER: BuiltinInstallManager = BuiltinInstallManager {
    id: "homebrew",
    kind: BuiltinManagerKind::Homebrew,
};
static MISE_MANAGER: BuiltinInstallManager = BuiltinInstallManager {
    id: "mise",
    kind: BuiltinManagerKind::Mise,
};
static RUSTUP_MANAGER: BuiltinInstallManager = BuiltinInstallManager {
    id: "rustup",
    kind: BuiltinManagerKind::Rustup,
};
static NPM_MANAGER: BuiltinInstallManager = BuiltinInstallManager {
    id: "npm",
    kind: BuiltinManagerKind::Npm,
};
static INSTALL_MANAGER_REGISTRY: [&'static dyn InstallManager; 4] = [
    &HOMEBREW_MANAGER,
    &MISE_MANAGER,
    &RUSTUP_MANAGER,
    &NPM_MANAGER,
];

pub fn install_manager_registry() -> &'static [&'static dyn InstallManager] {
    &INSTALL_MANAGER_REGISTRY
}

#[derive(Debug, Default, Deserialize)]
struct BrewOutdated {
    #[serde(default)]
    formulae: Vec<BrewItem>,
    #[serde(default)]
    casks: Vec<BrewItem>,
}
#[derive(Debug, Deserialize)]
struct BrewItem {
    name: String,
    #[serde(default)]
    installed_versions: Vec<String>,
    current_version: Option<String>,
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
}

async fn locate_executable(name: &str, context: &ProviderContext<'_>) -> Option<PathBuf> {
    let output = context
        .execute_silent(&CommandSpec::new("/usr/bin/which", [name]))
        .await
        .ok()?;
    let observed = PathBuf::from(output.stdout.lines().next()?.trim());
    (!observed.as_os_str().is_empty()).then_some(observed)
}

struct SystemCommandExecutor {
    verbose: bool,
}

#[async_trait]
impl CommandExecutor for SystemCommandExecutor {
    async fn execute(
        &self,
        command: &CommandSpec,
        timeout_seconds: u64,
    ) -> Result<crate::runner::CommandOutput> {
        crate::runner::run_with_output(command, timeout_seconds, self.verbose).await
    }
}

impl ProgressSink for Ui {
    fn started(&self, label: &str) {
        self.start_progress(label);
    }

    fn finished(&self, _label: &str) {
        self.finish_progress();
    }
}

async fn output(
    program: &str,
    args: &[&str],
    context: &ProviderContext<'_>,
    label: &str,
) -> Result<String> {
    Ok(context
        .execute(label, &CommandSpec::new(program, args.iter().copied()))
        .await?
        .stdout)
}

async fn other_source(tool: &str, active: Manager, context: &ProviderContext<'_>) -> Vec<String> {
    if active == Manager::Mise || locate_executable("mise", context).await.is_none() {
        return vec![];
    }
    match output(
        "mise",
        &["ls", tool],
        context,
        &format!("Checking alternate {tool} installations"),
    )
    .await
    {
        Ok(text) if !text.trim().is_empty() && !text.contains("No versions") => vec![format!(
            "mise: {}",
            text.lines().next().unwrap_or_default().trim()
        )],
        _ => vec![],
    }
}

fn status(current: Option<&String>, latest: Option<&String>) -> ToolStatus {
    match (current, latest) {
        (None, _) => ToolStatus::Missing,
        (Some(a), Some(b)) if a != b => ToolStatus::Outdated,
        (Some(_), _) => ToolStatus::Current,
    }
}

#[allow(clippy::too_many_arguments)]
fn report(
    id: &str,
    name: &str,
    current: Option<String>,
    latest: Option<String>,
    manager: Manager,
    executable: Option<PathBuf>,
    other_sources: Vec<String>,
    action: Option<CommandSpec>,
) -> ToolReport {
    let status = status(current.as_ref(), latest.as_ref());
    ToolReport {
        id: id.into(),
        name: name.into(),
        status,
        current,
        latest,
        manager,
        executable: executable.map(|p| p.to_string_lossy().to_string()),
        other_sources,
        detail: None,
        action: (status != ToolStatus::Missing).then_some(action).flatten(),
    }
}

fn npm_report(
    executable: Option<PathBuf>,
    current: Option<String>,
    latest: Option<String>,
) -> ToolReport {
    let manager = executable
        .as_deref()
        .map(manager_for_executable)
        .unwrap_or(Manager::Unknown);
    report(
        "npm",
        "npm",
        current,
        latest,
        manager,
        executable,
        vec![],
        Some(CommandSpec::new(
            "npm",
            ["install", "--global", "npm@latest"],
        )),
    )
}

pub async fn check_all_with_context(
    config: &Config,
    refresh: bool,
    context: &ProviderContext<'_>,
) -> Result<Vec<ToolReport>> {
    let mut reports = Vec::new();
    let brew_executable = locate_executable("brew", context).await;
    let brew_available = brew_executable.is_some();
    let mut brew_items = HashMap::new();
    if brew_available {
        if refresh {
            output("brew", &["update"], context, "Refreshing Homebrew metadata")
                .await
                .context("Homebrew refresh failed")?;
        }
        let json = output(
            "brew",
            &["outdated", "--json=v2"],
            context,
            "Checking Homebrew packages",
        )
        .await?;
        let outdated: BrewOutdated =
            serde_json::from_str(&json).context("invalid Homebrew outdated response")?;
        for item in outdated.formulae.into_iter().chain(outdated.casks) {
            brew_items.insert(item.name.clone(), item);
        }
    }

    if config.enabled_tools.iter().any(|t| t == "homebrew") {
        if brew_available {
            for item in brew_items.values() {
                let current = item.installed_versions.last().cloned();
                reports.push(report(
                    &format!("brew:{}", item.name),
                    &item.name,
                    current,
                    item.current_version.clone(),
                    Manager::Homebrew,
                    brew_executable.clone(),
                    vec![],
                    CommandSpec::brew_upgrade(&item.name).ok(),
                ));
            }
            if brew_items.is_empty() {
                reports.push(report(
                    "homebrew",
                    "Homebrew packages",
                    Some("installed".into()),
                    Some("installed".into()),
                    Manager::Homebrew,
                    brew_executable.clone(),
                    vec![],
                    None,
                ));
            }
        } else {
            reports.push(ToolReport {
                id: "homebrew".into(),
                name: "Homebrew".into(),
                current: None,
                latest: None,
                status: ToolStatus::Unavailable,
                manager: Manager::Unknown,
                executable: None,
                other_sources: vec![],
                detail: Some("brew is not available on PATH".into()),
                action: None,
            });
        }
    }

    if config.enabled_tools.iter().any(|t| t == "rust") {
        let detected = detect_tool("rust", context).await;
        let executable = detected
            .as_ref()
            .map(|tool| PathBuf::from(&tool.executable));
        if detected.is_some() && locate_executable("rustup", context).await.is_some() {
            let current = detected.map(|tool| tool.version.display().to_string());
            let active = output(
                "rustup",
                &["show", "active-toolchain"],
                context,
                "Reading active Rust toolchain",
            )
            .await
            .unwrap_or_else(|_| "stable".into());
            let channel = active
                .split_whitespace()
                .next()
                .unwrap_or("stable")
                .split('-')
                .next()
                .unwrap_or("stable");
            let check = output("rustup", &["check"], context, "Checking Rust updates")
                .await
                .unwrap_or_default();
            let latest = check
                .lines()
                .find(|line| line.starts_with(channel))
                .and_then(version_number)
                .or_else(|| current.clone());
            let action = Some(CommandSpec::new("rustup", ["update", channel]));
            reports.push(report(
                "rust",
                "Rust",
                current,
                latest,
                Manager::Rustup,
                executable,
                vec![],
                action,
            ));
        } else {
            reports.push(report(
                "rust",
                "Rust",
                None,
                None,
                Manager::Unknown,
                executable,
                vec![],
                None,
            ));
        }
    }

    for (id, display, brew_name) in [("node", "Node.js", "node"), ("go", "Go", "go")] {
        if !config.enabled_tools.iter().any(|t| t == id) {
            continue;
        }
        let detected = detect_tool(id, context).await;
        let executable = detected
            .as_ref()
            .map(|tool| PathBuf::from(&tool.executable));
        let manager = executable
            .as_deref()
            .map(manager_for_executable)
            .unwrap_or(Manager::Unknown);
        let current = detected.map(|tool| tool.version.display().to_string());
        let latest = if manager == Manager::Homebrew {
            brew_items
                .get(brew_name)
                .and_then(|i| i.current_version.clone())
                .or_else(|| current.clone())
        } else if manager == Manager::Mise {
            output(
                "mise",
                &["latest", id],
                context,
                &format!("Checking latest {display} version"),
            )
            .await
            .ok()
            .and_then(|s| version_number(&s).or(Some(s)))
        } else {
            current.clone()
        };
        let action = match manager {
            Manager::Homebrew => CommandSpec::brew_upgrade(brew_name).ok(),
            Manager::Mise => Some(CommandSpec::new(
                "mise",
                ["use", "-g", &format!("{id}@latest")],
            )),
            _ => None,
        };
        reports.push(report(
            id,
            display,
            current,
            latest,
            manager,
            executable,
            other_source(id, manager, context).await,
            action,
        ));
    }

    if config.enabled_tools.iter().any(|t| t == "npm") {
        let detected = detect_tool("npm", context).await;
        let executable = detected
            .as_ref()
            .map(|tool| PathBuf::from(&tool.executable));
        let current = detected.map(|tool| tool.version.display().to_string());
        let latest = output(
            "npm",
            &["view", "npm", "version"],
            context,
            "Checking latest npm version",
        )
        .await
        .ok()
        .and_then(|s| version_number(&s).or(Some(s)));
        reports.push(npm_report(executable, current, latest));
    }

    if config.enabled_tools.iter().any(|t| t == "pnpm") {
        let detected = detect_tool("pnpm", context).await;
        let executable = detected
            .as_ref()
            .map(|tool| PathBuf::from(&tool.executable));
        let current = detected.map(|tool| tool.version.display().to_string());
        let latest = output(
            "npm",
            &["view", "pnpm", "version"],
            context,
            "Checking latest pnpm version",
        )
        .await
        .ok()
        .and_then(|s| version_number(&s).or(Some(s)));
        let (manager, action) = if let Some(path) = executable.as_deref() {
            let manager = manager_for_executable(path);
            let action = match manager {
                Manager::Homebrew => CommandSpec::brew_upgrade("pnpm").ok(),
                Manager::Mise => Some(CommandSpec::new("mise", ["use", "-g", "pnpm@latest"])),
                _ => Some(CommandSpec::new(
                    "npm",
                    ["install", "--global", "pnpm@latest"],
                )),
            };
            (manager, action)
        } else {
            (Manager::Unknown, None)
        };
        reports.push(report(
            "pnpm",
            "pnpm",
            current,
            latest,
            manager,
            executable,
            other_source("pnpm", manager, context).await,
            action,
        ));
    }

    let mut seen = HashSet::new();
    reports.retain(|item| seen.insert(item.id.clone()));
    reports.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(reports)
}

pub async fn check_all(config: &Config, refresh: bool, ui: &Ui) -> Result<Vec<ToolReport>> {
    let executor = SystemCommandExecutor {
        verbose: ui.mode() == crate::ui::FeedbackMode::Verbose,
    };
    let context = ProviderContext::new(&executor, ui, config.command_timeout_seconds);
    check_all_with_context(config, refresh, &context).await
}

pub async fn verify_with_context(
    report: &ToolReport,
    context: &ProviderContext<'_>,
) -> Result<Option<String>> {
    let (program, args): (&str, &[&str]) = match report.id.as_str() {
        "rust" => ("rustc", &["--version"]),
        "node" => ("node", &["--version"]),
        "npm" => ("npm", &["--version"]),
        "pnpm" => ("pnpm", &["--version"]),
        "go" => ("go", &["version"]),
        id if id.starts_with("brew:") => return Ok(Some("installed".into())),
        _ => return Ok(report.current.clone()),
    };
    let text = output(
        program,
        args,
        context,
        &format!("Verifying {}", report.name),
    )
    .await?;
    Ok(version_number(&text))
}

pub async fn verify(report: &ToolReport, config: &Config, ui: &Ui) -> Result<Option<String>> {
    let executor = SystemCommandExecutor {
        verbose: ui.mode() == crate::ui::FeedbackMode::Verbose,
    };
    let context = ProviderContext::new(&executor, ui, config.command_timeout_seconds);
    verify_with_context(report, &context).await
}

pub fn recovery_hint(report: &ToolReport) -> String {
    match report.manager {
        Manager::Homebrew => format!(
            "Run `brew doctor` and inspect `brew info {}`.",
            report.id.trim_start_matches("brew:")
        ),
        Manager::Rustup => "Run `rustup show` and `rustup check`.".into(),
        Manager::Mise => "Run `mise doctor` and `mise current`.".into(),
        Manager::Npm => "Run `npm doctor` and inspect your global prefix.".into(),
        Manager::Unknown => "Inspect PATH and reinstall the tool with its original manager.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_reports_never_offer_an_action() {
        let item = report(
            "pnpm",
            "pnpm",
            None,
            Some("11.0.0".into()),
            Manager::Homebrew,
            None,
            vec![],
            Some(CommandSpec::new("brew", ["install", "pnpm"])),
        );

        assert_eq!(item.status, ToolStatus::Missing);
        assert!(item.action.is_none());
    }

    #[test]
    fn npm_report_uses_the_executable_installation_manager() {
        let item = npm_report(
            Some(PathBuf::from(
                "/Users/alice/.local/share/mise/installs/node/26.5.0/bin/npm",
            )),
            Some("11.17.0".into()),
            Some("12.0.0".into()),
        );

        assert_eq!(item.manager, Manager::Mise);
        let action = item.action.unwrap();
        assert_eq!(action.program, "npm");
        assert_eq!(action.args, ["install", "--global", "npm@latest"]);
    }

    #[test]
    fn npm_report_supports_homebrew_and_missing_installations() {
        let homebrew = npm_report(
            Some(PathBuf::from("/opt/homebrew/bin/npm")),
            Some("11.17.0".into()),
            Some("12.0.0".into()),
        );
        let missing = npm_report(None, None, Some("12.0.0".into()));

        assert_eq!(homebrew.manager, Manager::Homebrew);
        assert_eq!(missing.manager, Manager::Unknown);
        assert!(missing.action.is_none());
    }
}

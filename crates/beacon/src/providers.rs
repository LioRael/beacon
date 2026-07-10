use crate::{
    AlternativeInstallation, CheckData, Diagnostics, InstallationReport, InventoryReport,
    ToolReport, ToolStatus, UpdateReport, command::CommandSpec, config::Config, ui::Ui,
    versions::version_number,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use serde::Deserialize;
use std::{cmp::Ordering, collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::OnceCell;

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
                    bail!("invalid provider id `{value}`");
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
            bail!("tool version cannot be empty");
        }
        if normalized
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            bail!("normalized tool version cannot be empty");
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
        let result = self.execute_silent(command).await;
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClaimEvidence {
    pub claim: String,
    pub id: String,
    pub confidence: ClaimConfidence,
    pub evidence: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedClaims {
    pub source: Option<SourceClaim>,
    pub updater: Option<UpdaterClaim>,
    pub evidence: Vec<ClaimEvidence>,
    pub conflicts: Vec<ClaimEvidence>,
}

pub fn resolve_claims(claims: impl IntoIterator<Item = ManagerClaims>) -> ResolvedClaims {
    let claims = claims.into_iter().collect::<Vec<_>>();
    let evidence = claims
        .iter()
        .flat_map(|claims| {
            claims
                .source
                .iter()
                .map(|claim| ClaimEvidence {
                    claim: "source".into(),
                    id: claim.source.to_string(),
                    confidence: claim.confidence,
                    evidence: claim.evidence.clone(),
                })
                .chain(claims.updater.iter().map(|claim| ClaimEvidence {
                    claim: "updater".into(),
                    id: claim.manager.to_string(),
                    confidence: claim.confidence,
                    evidence: claim.evidence.clone(),
                }))
        })
        .collect::<Vec<_>>();

    let sources = claims
        .iter()
        .filter_map(|claims| claims.source.clone())
        .collect::<Vec<_>>();
    let updaters = claims
        .iter()
        .filter_map(|claims| claims.updater.clone())
        .collect::<Vec<_>>();
    let source_max = sources.iter().map(|claim| claim.confidence).max();
    let updater_max = updaters.iter().map(|claim| claim.confidence).max();
    let source_top = sources
        .iter()
        .filter(|claim| Some(claim.confidence) == source_max)
        .cloned()
        .collect::<Vec<_>>();
    let updater_top = updaters
        .iter()
        .filter(|claim| Some(claim.confidence) == updater_max)
        .cloned()
        .collect::<Vec<_>>();
    let mut conflicts = Vec::new();
    if source_top.len() > 1 {
        conflicts.extend(
            evidence
                .iter()
                .filter(|item| item.claim == "source" && Some(item.confidence) == source_max)
                .cloned(),
        );
    }
    if updater_top.len() > 1 {
        conflicts.extend(
            evidence
                .iter()
                .filter(|item| item.claim == "updater" && Some(item.confidence) == updater_max)
                .cloned(),
        );
    }
    ResolvedClaims {
        source: (source_top.len() == 1).then(|| source_top[0].clone()),
        updater: (updater_top.len() == 1).then(|| updater_top[0].clone()),
        evidence,
        conflicts,
    }
}

pub fn verify_versions<F>(
    mode: TargetMode,
    old: &ToolVersion,
    expected: &ToolVersion,
    actual: &ToolVersion,
    mut compare: F,
) -> Result<()>
where
    F: FnMut(&ToolVersion, &ToolVersion) -> Result<Ordering>,
{
    match mode {
        TargetMode::Exact if compare(actual, expected)? != Ordering::Equal => {
            bail!(
                "exact verification expected {}, got {}",
                expected.display(),
                actual.display()
            )
        }
        TargetMode::Floating
            if compare(actual, old)? != Ordering::Greater
                || compare(actual, expected)? == Ordering::Less =>
        {
            bail!(
                "floating verification requires a newer version at least {}, got {}",
                expected.display(),
                actual.display()
            )
        }
        _ => {}
    }
    Ok(())
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
        ToolId::new(self.id).expect("valid built-in tool ID")
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    async fn detect(&self, context: &ProviderContext<'_>) -> Result<DetectedTool> {
        let location = context
            .execute_silent(&CommandSpec::new("/usr/bin/which", [self.executable]))
            .await
            .context("tool is not available on PATH")?;
        let executable = location
            .stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("tool is not available on PATH")?
            .to_string();
        let output = context
            .execute(
                &format!("Reading {} version", self.display_name),
                &CommandSpec::new(&executable, self.version_args.iter().copied()),
            )
            .await?;
        Ok(DetectedTool {
            id: self.id(),
            executable,
            version: self.parse_version(&output.stdout)?,
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
        Ok(semver::Version::parse(current.display())?
            .cmp(&semver::Version::parse(latest.display())?))
    }
}

macro_rules! adapter {
    ($name:ident, $id:literal, $display:literal, $exe:literal, [$($arg:literal),*]) => {
        static $name: BuiltinToolAdapter = BuiltinToolAdapter {
            id: $id,
            display_name: $display,
            executable: $exe,
            version_args: &[$($arg),*],
        };
    };
}

adapter!(RUST_ADAPTER, "rust", "Rust", "rustc", ["--version"]);
adapter!(NODE_ADAPTER, "node", "Node.js", "node", ["--version"]);
adapter!(NPM_ADAPTER, "npm", "npm", "npm", ["--version"]);
adapter!(PNPM_ADAPTER, "pnpm", "pnpm", "pnpm", ["--version"]);
adapter!(GO_ADAPTER, "go", "Go", "go", ["version"]);
adapter!(BUN_ADAPTER, "bun", "Bun", "bun", ["--version"]);
adapter!(DENO_ADAPTER, "deno", "Deno", "deno", ["--version"]);
adapter!(UV_ADAPTER, "uv", "uv", "uv", ["--version"]);

static TOOL_REGISTRY: [&'static dyn ToolAdapter; 8] = [
    &RUST_ADAPTER,
    &NODE_ADAPTER,
    &NPM_ADAPTER,
    &PNPM_ADAPTER,
    &GO_ADAPTER,
    &BUN_ADAPTER,
    &DENO_ADAPTER,
    &UV_ADAPTER,
];

pub fn tool_registry() -> &'static [&'static dyn ToolAdapter] {
    &TOOL_REGISTRY
}

fn tool_executable_name(id: &str) -> &str {
    if id == "rust" { "rustc" } else { id }
}

fn project_file_contents(name: &str) -> Option<String> {
    let current = std::env::current_dir().ok()?;
    current
        .ancestors()
        .map(|directory| directory.join(name))
        .find_map(|path| std::fs::read_to_string(path).ok())
}

fn project_mise_selects(tool: &str) -> bool {
    let mise_toml_selects = project_file_contents(".mise.toml")
        .and_then(|source| toml::from_str::<toml::Value>(&source).ok())
        .and_then(|value| value.get("tools").and_then(toml::Value::as_table).cloned())
        .is_some_and(|tools| tools.contains_key(tool));
    let tool_versions_selects = project_file_contents(".tool-versions").is_some_and(|source| {
        source.lines().any(|line| {
            line.split('#')
                .next()
                .and_then(|entry| entry.split_whitespace().next())
                == Some(tool)
        })
    });
    mise_toml_selects || tool_versions_selects
}

fn project_pins_pnpm() -> bool {
    project_file_contents("package.json")
        .and_then(|source| serde_json::from_str::<serde_json::Value>(&source).ok())
        .and_then(|value| value.get("packageManager")?.as_str().map(str::to_owned))
        .is_some_and(|manager| manager.starts_with("pnpm@"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ManagerKind {
    Homebrew,
    Mise,
    Rustup,
    Npm,
    Corepack,
    BunOfficial,
    DenoOfficial,
    UvStandalone,
}

struct BuiltinInstallManager {
    id: &'static str,
    kind: ManagerKind,
}

impl BuiltinInstallManager {
    fn receipt_matches(&self, tool: &DetectedTool, evidence: &ManagerEvidence) -> bool {
        if !evidence.kind.starts_with("receipt")
            || evidence.value.split_whitespace().next() != Some(tool.id.as_str())
        {
            return false;
        }
        let active = std::fs::canonicalize(&tool.executable)
            .unwrap_or_else(|_| PathBuf::from(&tool.executable));
        let has_linked_path = evidence.value.split_whitespace().skip(1).any(|token| {
            if !token.starts_with('/') && !token.starts_with('~') {
                return false;
            }
            let receipt_path =
                std::fs::canonicalize(token).unwrap_or_else(|_| PathBuf::from(token));
            active == receipt_path || active.starts_with(&receipt_path)
        });
        has_linked_path || self.path_matches(tool)
    }

    fn inventory_upgrade(
        &self,
        kind: &str,
        name: &str,
        latest: &ToolVersion,
    ) -> Result<UpgradeAction> {
        if self.kind != ManagerKind::Homebrew || !matches!(kind, "formula" | "cask") {
            bail!("unsupported inventory update target");
        }
        Ok(UpgradeAction {
            manager: self.id(),
            command: CommandSpec::brew_inventory_upgrade(kind, name)?,
            expected_version: latest.clone(),
            target_mode: TargetMode::Floating,
        })
    }

    fn path_matches(&self, tool: &DetectedTool) -> bool {
        let path = std::fs::canonicalize(&tool.executable)
            .unwrap_or_else(|_| PathBuf::from(&tool.executable))
            .to_string_lossy()
            .to_lowercase();
        match self.kind {
            ManagerKind::Homebrew => path.contains("/homebrew/"),
            ManagerKind::Mise => path.contains("/mise/") && path.contains("/installs/"),
            ManagerKind::Rustup => {
                tool.id.as_str() == "rust"
                    && (path.contains("/.cargo/") || path.contains("/rustup/"))
            }
            ManagerKind::Npm => {
                tool.id.as_str() == "npm"
                    || (tool.id.as_str() == "pnpm"
                        && path.contains("node_modules")
                        && !path.contains("node_modules/corepack"))
            }
            ManagerKind::Corepack => {
                tool.id.as_str() == "pnpm"
                    && (path.contains("corepack") || path.contains("/shims/"))
            }
            ManagerKind::BunOfficial => tool.id.as_str() == "bun" && path.contains("/.bun/"),
            ManagerKind::DenoOfficial => tool.id.as_str() == "deno" && path.contains("/.deno/"),
            ManagerKind::UvStandalone => {
                tool.id.as_str() == "uv" && path.contains("/.local/bin/uv")
            }
        }
    }

    fn confidence(&self, tool: &DetectedTool, snapshot: &ManagerSnapshot) -> ClaimConfidence {
        if snapshot
            .evidence
            .iter()
            .any(|evidence| self.receipt_matches(tool, evidence))
        {
            ClaimConfidence::Receipt
        } else if std::fs::canonicalize(&tool.executable).is_ok_and(|path| {
            let path = path.to_string_lossy().to_lowercase();
            match self.kind {
                ManagerKind::Homebrew => path.contains("/homebrew/"),
                ManagerKind::Mise => path.contains("/mise/") && path.contains("/installs/"),
                ManagerKind::Rustup => path.contains("/.cargo/") || path.contains("/rustup/"),
                ManagerKind::Npm => {
                    tool.id.as_str() == "pnpm"
                        && path.contains("node_modules")
                        && !path.contains("node_modules/corepack")
                }
                ManagerKind::Corepack => path.contains("corepack") || path.contains("/shims/"),
                ManagerKind::BunOfficial => path.contains("/.bun/"),
                ManagerKind::DenoOfficial => path.contains("/.deno/"),
                ManagerKind::UvStandalone => path.contains("/.local/bin/uv"),
            }
        }) {
            ClaimConfidence::CanonicalPath
        } else {
            ClaimConfidence::PathHeuristic
        }
    }

    fn supports_update(&self, tool: &DetectedTool) -> bool {
        match self.kind {
            ManagerKind::Homebrew | ManagerKind::Mise => tool.id.as_str() != "npm",
            ManagerKind::Rustup => tool.id.as_str() == "rust",
            ManagerKind::Npm => matches!(tool.id.as_str(), "npm" | "pnpm"),
            ManagerKind::Corepack => tool.id.as_str() == "pnpm",
            ManagerKind::BunOfficial => tool.id.as_str() == "bun",
            ManagerKind::DenoOfficial => tool.id.as_str() == "deno",
            ManagerKind::UvStandalone => tool.id.as_str() == "uv",
        }
    }

    fn claim_evidence(
        &self,
        tool: &DetectedTool,
        snapshot: &ManagerSnapshot,
        receipt: bool,
        role: &str,
    ) -> String {
        let basis = receipt
            .then(|| {
                snapshot
                    .evidence
                    .iter()
                    .find(|evidence| self.receipt_matches(tool, evidence))
                    .map(|evidence| format!("{}: {}", evidence.kind, evidence.value))
            })
            .flatten()
            .unwrap_or_else(|| format!("active executable: {}", tool.executable));
        let redacted = crate::redact::redact(&basis, std::env::var("HOME").ok().as_deref());
        format!("{} {role} evidence: {redacted}", self.id)
    }
}

#[async_trait]
impl InstallManager for BuiltinInstallManager {
    fn id(&self) -> ManagerId {
        ManagerId::new(self.id).expect("valid built-in manager ID")
    }

    async fn snapshot(
        &self,
        context: &ProviderContext<'_>,
        refresh: RefreshPolicy,
    ) -> Result<ManagerSnapshot> {
        if self.kind == ManagerKind::Homebrew {
            if refresh == RefreshPolicy::Refresh {
                context
                    .execute(
                        "Refreshing Homebrew metadata",
                        &CommandSpec::new("brew", ["update"]),
                    )
                    .await?;
            }
            let formulae = context
                .execute(
                    "Reading Homebrew formula receipts",
                    &CommandSpec::new("brew", ["list", "--formula", "--versions"]),
                )
                .await?;
            let casks = context
                .execute(
                    "Reading Homebrew cask receipts",
                    &CommandSpec::new("brew", ["list", "--cask", "--versions"]),
                )
                .await?;
            let prefix = context
                .execute(
                    "Reading Homebrew prefix",
                    &CommandSpec::new("brew", ["--prefix"]),
                )
                .await?
                .stdout
                .trim()
                .to_string();
            if prefix.is_empty() {
                bail!("Homebrew prefix is empty");
            }
            let evidence = formulae
                .stdout
                .lines()
                .map(|line| ManagerEvidence {
                    kind: "receipt:formula".into(),
                    value: format!(
                        "{line} {prefix}/bin/{name} {prefix}/opt/{name}",
                        name = line.split_whitespace().next().unwrap_or_default()
                    ),
                })
                .chain(casks.stdout.lines().map(|line| ManagerEvidence {
                    kind: "receipt:cask".into(),
                    value: format!(
                        "{line} {prefix}/Caskroom/{name}",
                        name = line.split_whitespace().next().unwrap_or_default()
                    ),
                }))
                .collect();
            return Ok(ManagerSnapshot {
                manager: self.id(),
                evidence,
            });
        }
        let command = match self.kind {
            ManagerKind::Homebrew => unreachable!("Homebrew snapshot handled above"),
            ManagerKind::Mise => CommandSpec::new("mise", ["ls", "--json"]),
            ManagerKind::Rustup => CommandSpec::new("rustup", ["show", "active-toolchain"]),
            ManagerKind::Npm => CommandSpec::new("npm", ["prefix", "--global"]),
            ManagerKind::Corepack => CommandSpec::new("corepack", ["--version"]),
            ManagerKind::BunOfficial => CommandSpec::new("bun", ["--version"]),
            ManagerKind::DenoOfficial => CommandSpec::new("deno", ["--version"]),
            ManagerKind::UvStandalone => CommandSpec::new("uv", ["--version"]),
        };
        let output = context
            .execute(&format!("Reading {} manager state", self.id), &command)
            .await?;
        let kind = if self.kind == ManagerKind::Rustup {
            "active-toolchain"
        } else {
            "manager-output"
        };
        let redacted =
            crate::redact::redact(output.stdout.trim(), std::env::var("HOME").ok().as_deref());
        let mut evidence = vec![ManagerEvidence {
            kind: kind.into(),
            value: redacted.clone(),
        }];
        match self.kind {
            ManagerKind::Homebrew => unreachable!("Homebrew snapshot handled above"),
            ManagerKind::Mise => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&redacted) {
                    if let Some(tools) = value.as_object() {
                        for (tool, entries) in tools {
                            let entries = entries.as_array();
                            let mut linked_receipt = false;
                            for install_path in entries
                                .into_iter()
                                .flatten()
                                .filter_map(|entry| entry.get("install_path"))
                                .filter_map(|value| value.as_str())
                            {
                                linked_receipt = true;
                                evidence.push(ManagerEvidence {
                                    kind: "receipt".into(),
                                    value: format!(
                                        "{tool} {}",
                                        PathBuf::from(install_path)
                                            .join("bin")
                                            .join(tool_executable_name(tool))
                                            .display()
                                    ),
                                });
                            }
                            if !linked_receipt {
                                evidence.push(ManagerEvidence {
                                    kind: "receipt".into(),
                                    value: tool.clone(),
                                });
                            }
                            if let Some(entry) = entries.and_then(|entries| entries.first()) {
                                if let Some(selector) = entry
                                    .get("requested_version")
                                    .or_else(|| entry.get("version"))
                                    .and_then(|value| value.as_str())
                                {
                                    evidence.push(ManagerEvidence {
                                        kind: format!("selector:{tool}"),
                                        value: selector.into(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            ManagerKind::Rustup => evidence.push(ManagerEvidence {
                kind: "receipt".into(),
                value: "rust".into(),
            }),
            _ => {}
        }
        match self.kind {
            ManagerKind::Mise => {
                for tool in tool_registry() {
                    let id = tool.id();
                    if project_mise_selects(id.as_str()) {
                        evidence.push(ManagerEvidence {
                            kind: format!("project-policy:{id}"),
                            value: "project mise selection".into(),
                        });
                    }
                }
            }
            ManagerKind::Npm | ManagerKind::Corepack if project_pins_pnpm() => {
                evidence.push(ManagerEvidence {
                    kind: "project-policy:pnpm".into(),
                    value: "project packageManager pin".into(),
                });
            }
            _ => {}
        }
        Ok(ManagerSnapshot {
            manager: self.id(),
            evidence,
        })
    }

    fn claim(&self, tool: &DetectedTool, snapshot: &ManagerSnapshot) -> ManagerClaims {
        let receipt = snapshot
            .evidence
            .iter()
            .any(|evidence| self.receipt_matches(tool, evidence));
        if self.kind == ManagerKind::Mise && tool.id.as_str() == "pnpm" && !receipt {
            return ManagerClaims::default();
        }
        if !self.path_matches(tool) && !receipt {
            return ManagerClaims::default();
        }
        let confidence = self.confidence(tool, snapshot);
        let source =
            (!(self.kind == ManagerKind::Npm && tool.id.as_str() == "npm")).then(|| SourceClaim {
                source: SourceId::new(if self.kind == ManagerKind::Npm {
                    "npm-global"
                } else {
                    self.id
                })
                .expect("valid source ID"),
                confidence,
                evidence: self.claim_evidence(tool, snapshot, receipt, "source"),
            });
        let project_managed = snapshot
            .evidence
            .iter()
            .any(|evidence| evidence.kind == format!("project-policy:{}", tool.id));
        let updater = (self.supports_update(tool) && !project_managed).then(|| UpdaterClaim {
            manager: self.id(),
            confidence,
            evidence: self.claim_evidence(tool, snapshot, receipt, "updater"),
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
            ManagerKind::Homebrew => {
                CommandSpec::new("brew", ["info", "--json=v2", tool.id.as_str()])
            }
            ManagerKind::Mise => {
                let selector = snapshot
                    .evidence
                    .iter()
                    .find(|item| item.kind == format!("selector:{}", tool.id))
                    .map(|item| item.value.as_str())
                    .unwrap_or("latest");
                CommandSpec::new("mise", ["latest", &format!("{}@{selector}", tool.id)])
            }
            ManagerKind::Rustup => CommandSpec::new("rustup", ["check"]),
            ManagerKind::Npm | ManagerKind::Corepack => {
                CommandSpec::new("npm", ["view", tool.id.as_str(), "version"])
            }
            ManagerKind::BunOfficial => CommandSpec::new(
                "curl",
                [
                    "--fail",
                    "--silent",
                    "--show-error",
                    "https://api.github.com/repos/oven-sh/bun/releases/latest",
                ],
            ),
            ManagerKind::DenoOfficial => CommandSpec::new("deno", ["upgrade", "--dry-run"]),
            ManagerKind::UvStandalone => CommandSpec::new("uv", ["self", "update", "--dry-run"]),
        };
        let output = context
            .execute(&format!("Checking latest {} version", tool.id), &command)
            .await?;
        let normalized = match self.kind {
            ManagerKind::Homebrew => {
                let value: serde_json::Value = serde_json::from_str(&output.stdout)?;
                value["formulae"]
                    .as_array()
                    .and_then(|items| items.first())
                    .and_then(|item| item["versions"]["stable"].as_str())
                    .or_else(|| {
                        value["casks"]
                            .as_array()
                            .and_then(|items| items.first())
                            .and_then(|item| item["version"].as_str())
                    })
                    .map(str::to_string)
                    .context("Homebrew response had no version")?
            }
            ManagerKind::Rustup => {
                let channel = snapshot
                    .evidence
                    .iter()
                    .find(|item| item.kind == "active-toolchain")
                    .and_then(|item| item.value.split_whitespace().next())
                    .context("rustup snapshot had no active channel")?;
                output
                    .stdout
                    .lines()
                    .find(|line| line.starts_with(channel))
                    .and_then(|line| {
                        line.split_whitespace()
                            .filter_map(version_number)
                            .next_back()
                    })
                    .context("rustup check had no active-channel version")?
            }
            ManagerKind::BunOfficial => {
                let value: serde_json::Value =
                    serde_json::from_str(&output.stdout).context("invalid Bun release response")?;
                value["tag_name"]
                    .as_str()
                    .and_then(|tag| version_number(tag.trim_start_matches("bun-")))
                    .context("Bun release response had no version")?
            }
            _ => version_number(&output.stdout).context("latest output had no version")?,
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
            ManagerKind::Homebrew => (
                CommandSpec::new(
                    "brew",
                    [
                        "upgrade",
                        {
                            let canonical = std::fs::canonicalize(&tool.executable)
                                .unwrap_or_else(|_| PathBuf::from(&tool.executable))
                                .to_string_lossy()
                                .to_lowercase();
                            let formula = snapshot.evidence.iter().any(|evidence| {
                                evidence.kind == "receipt:formula"
                                    && evidence.value.split_whitespace().next()
                                        == Some(tool.id.as_str())
                            });
                            let cask = snapshot.evidence.iter().any(|evidence| {
                                evidence.kind == "receipt:cask"
                                    && evidence.value.split_whitespace().next()
                                        == Some(tool.id.as_str())
                            });
                            let formula_exact = snapshot.evidence.iter().any(|evidence| {
                                evidence.kind == "receipt:formula"
                                    && self.receipt_matches(tool, evidence)
                            });
                            let cask_exact = snapshot.evidence.iter().any(|evidence| {
                                evidence.kind == "receipt:cask"
                                    && self.receipt_matches(tool, evidence)
                            });
                            if cask_exact || canonical.contains("/caskroom/") || (cask && !formula)
                            {
                                "--cask"
                            } else if formula_exact
                                || canonical.contains("/cellar/")
                                || canonical.contains("/opt/")
                                || (formula && !cask)
                            {
                                "--formula"
                            } else {
                                bail!("Homebrew formula/cask ownership is ambiguous");
                            }
                        },
                        tool.id.as_str(),
                    ],
                ),
                TargetMode::Floating,
            ),
            ManagerKind::Mise => {
                let selector = snapshot
                    .evidence
                    .iter()
                    .find(|item| item.kind == format!("selector:{}", tool.id))
                    .map(|item| item.value.as_str())
                    .unwrap_or("latest");
                (
                    CommandSpec::new("mise", ["use", "-g", &format!("{}@{selector}", tool.id)]),
                    TargetMode::Floating,
                )
            }
            ManagerKind::Rustup => {
                let channel = snapshot
                    .evidence
                    .iter()
                    .find(|item| item.kind == "active-toolchain")
                    .and_then(|item| item.value.split_whitespace().next())
                    .context("rustup snapshot had no active channel")?;
                (
                    CommandSpec::new("rustup", ["update", channel]),
                    TargetMode::Floating,
                )
            }
            ManagerKind::Npm => (
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
            ManagerKind::Corepack => (
                CommandSpec::new(
                    "corepack",
                    [
                        "prepare",
                        &format!("pnpm@{}", latest.display()),
                        "--activate",
                    ],
                ),
                TargetMode::Exact,
            ),
            ManagerKind::BunOfficial => {
                (CommandSpec::new("bun", ["upgrade"]), TargetMode::Floating)
            }
            ManagerKind::DenoOfficial => (
                CommandSpec::new("deno", ["upgrade", "--version", latest.display()]),
                TargetMode::Exact,
            ),
            ManagerKind::UvStandalone => (
                CommandSpec::new("uv", ["self", "update", latest.display()]),
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

macro_rules! manager {
    ($name:ident, $id:literal, $kind:ident) => {
        static $name: BuiltinInstallManager = BuiltinInstallManager {
            id: $id,
            kind: ManagerKind::$kind,
        };
    };
}

manager!(HOMEBREW_MANAGER, "homebrew", Homebrew);
manager!(MISE_MANAGER, "mise", Mise);
manager!(RUSTUP_MANAGER, "rustup", Rustup);
manager!(NPM_MANAGER, "npm", Npm);
manager!(COREPACK_MANAGER, "corepack", Corepack);
manager!(BUN_MANAGER, "bun-official", BunOfficial);
manager!(DENO_MANAGER, "deno-official", DenoOfficial);
manager!(UV_MANAGER, "uv-standalone", UvStandalone);

static INSTALL_MANAGER_REGISTRY: [&'static dyn InstallManager; 8] = [
    &HOMEBREW_MANAGER,
    &MISE_MANAGER,
    &RUSTUP_MANAGER,
    &NPM_MANAGER,
    &COREPACK_MANAGER,
    &BUN_MANAGER,
    &DENO_MANAGER,
    &UV_MANAGER,
];

pub fn install_manager_registry() -> &'static [&'static dyn InstallManager] {
    &INSTALL_MANAGER_REGISTRY
}

fn empty_snapshot(manager: &dyn InstallManager) -> ManagerSnapshot {
    ManagerSnapshot {
        manager: manager.id(),
        evidence: Vec::new(),
    }
}

type SnapshotCache = Arc<HashMap<String, OnceCell<Option<ManagerSnapshot>>>>;

fn manager_executable(manager: &dyn InstallManager) -> &'static str {
    match manager.id().as_str() {
        "homebrew" => "brew",
        "mise" => "mise",
        "rustup" => "rustup",
        "npm" => "npm",
        "corepack" => "corepack",
        "bun-official" => "bun",
        "deno-official" => "deno",
        "uv-standalone" => "uv",
        _ => unreachable!("all managers are built in"),
    }
}

async fn prefetch_snapshots(refresh: bool, context: &ProviderContext<'_>, cache: &SnapshotCache) {
    stream::iter(install_manager_registry().iter().copied().map(|manager| {
        let cache = cache.clone();
        async move {
            if context
                .execute_silent(&CommandSpec::new(
                    "/usr/bin/which",
                    [manager_executable(manager)],
                ))
                .await
                .is_err()
            {
                return;
            }
            let cell = cache
                .get(manager.id().as_str())
                .expect("manager cache entry exists");
            cell.get_or_init(|| async {
                manager
                    .snapshot(
                        context,
                        if refresh {
                            RefreshPolicy::Refresh
                        } else {
                            RefreshPolicy::Cached
                        },
                    )
                    .await
                    .ok()
            })
            .await;
        }
    }))
    .buffered(4)
    .collect::<Vec<_>>()
    .await;
}

async fn path_claims(
    tool: &DetectedTool,
    refresh: bool,
    context: &ProviderContext<'_>,
    cache: &SnapshotCache,
) -> Vec<ManagerClaims> {
    let mut claims = Vec::new();
    for manager in install_manager_registry() {
        let cell = cache
            .get(manager.id().as_str())
            .expect("manager cache entry exists");
        let preliminary_snapshot = cell
            .get()
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| empty_snapshot(*manager));
        let preliminary = manager.claim(tool, &preliminary_snapshot);
        if preliminary.source.is_none() && preliminary.updater.is_none() {
            continue;
        }
        let snapshot = cell
            .get_or_init(|| async {
                manager
                    .snapshot(
                        context,
                        if refresh {
                            RefreshPolicy::Refresh
                        } else {
                            RefreshPolicy::Cached
                        },
                    )
                    .await
                    .ok()
            })
            .await;
        claims.push(manager.claim(tool, snapshot.as_ref().unwrap_or(&empty_snapshot(*manager))));
    }
    let path = std::fs::canonicalize(&tool.executable)
        .unwrap_or_else(|_| PathBuf::from(&tool.executable))
        .to_string_lossy()
        .to_lowercase();
    let diagnostic_source = if tool.id.as_str() == "uv" && path.contains("pipx") {
        Some("pipx")
    } else if tool.id.as_str() == "uv" && (path.contains("site-packages") || path.contains(".venv"))
    {
        Some("pip")
    } else if tool.id.as_str() == "uv" && path.contains("/.cargo/") {
        Some("cargo")
    } else {
        None
    };
    if let Some(source) = diagnostic_source {
        claims.push(ManagerClaims {
            source: Some(SourceClaim {
                source: SourceId::new(source).expect("valid diagnostic source"),
                confidence: ClaimConfidence::PathHeuristic,
                evidence: "diagnostic-only uv installation source".into(),
            }),
            updater: None,
        });
    }
    claims
}

async fn alternate_installations(
    tool: &DetectedTool,
    cache: &SnapshotCache,
    context: &ProviderContext<'_>,
) -> Vec<AlternativeInstallation> {
    let mut alternatives = Vec::new();
    let active =
        std::fs::canonicalize(&tool.executable).unwrap_or_else(|_| PathBuf::from(&tool.executable));
    let executable_name = active
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(tool.id.as_str());
    for (manager, cell) in cache.iter() {
        let Some(snapshot) = cell.get().and_then(Option::as_ref) else {
            continue;
        };
        let mut versions = Vec::new();
        let mut paths = Vec::new();
        if manager == "homebrew" {
            for evidence in snapshot.evidence.iter().filter(|evidence| {
                evidence.kind.starts_with("receipt:")
                    && evidence.value.split_whitespace().next() == Some(tool.id.as_str())
            }) {
                let kind = evidence.kind.trim_start_matches("receipt:");
                let mut receipt_paths = Vec::new();
                if let Ok(output) = context
                    .execute_silent(&CommandSpec::new(
                        "brew",
                        ["list", &format!("--{kind}"), tool.id.as_str()],
                    ))
                    .await
                {
                    receipt_paths.extend(output.stdout.lines().filter_map(|path| {
                        let candidate = PathBuf::from(path);
                        (candidate.file_name().and_then(|name| name.to_str())
                            == Some(executable_name))
                        .then(|| path.to_string())
                    }));
                }
                receipt_paths.retain(|path| {
                    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)) != active
                });
                if !receipt_paths.is_empty() {
                    versions.extend(
                        evidence
                            .value
                            .split_whitespace()
                            .skip(1)
                            .take_while(|version| !version.starts_with('/'))
                            .filter_map(|version| {
                                ToolVersion::new(version, Some(version.into())).ok()
                            }),
                    );
                    paths.extend(receipt_paths);
                }
            }
        } else if manager == "mise" {
            if let Some(value) = snapshot
                .evidence
                .iter()
                .find(|evidence| evidence.kind == "manager-output")
                .and_then(|evidence| {
                    serde_json::from_str::<serde_json::Value>(&evidence.value).ok()
                })
            {
                for entry in value[tool.id.as_str()].as_array().into_iter().flatten() {
                    if let Some(version) = entry.get("version").and_then(|value| value.as_str()) {
                        if let Some(install_path) =
                            entry.get("install_path").and_then(|value| value.as_str())
                        {
                            let path = PathBuf::from(install_path)
                                .join("bin")
                                .join(executable_name);
                            let canonical =
                                std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                            if canonical != active {
                                if let Ok(version) = ToolVersion::new(version, Some(version.into()))
                                {
                                    versions.push(version);
                                }
                                paths.push(path.to_string_lossy().into_owned());
                            }
                        }
                    }
                }
            }
        }
        paths.sort();
        paths.dedup();
        versions.sort_by(|left, right| left.display().cmp(right.display()));
        versions.dedup();
        if !versions.is_empty() && !paths.is_empty() {
            alternatives.push(AlternativeInstallation {
                source: SourceId::new(manager).expect("manager IDs are valid source IDs"),
                versions,
                paths,
            });
        }
    }
    alternatives.sort_by(|left, right| left.source.as_str().cmp(right.source.as_str()));
    alternatives
}

async fn report_tool(
    adapter: &'static dyn ToolAdapter,
    refresh: bool,
    context: &ProviderContext<'_>,
    cache: SnapshotCache,
) -> ToolReport {
    let id = adapter.id().to_string();
    let name = adapter.display_name().to_string();
    let tool = match adapter.detect(context).await {
        Ok(tool) => tool,
        Err(error) if error.to_string().contains("not available on PATH") => {
            return ToolReport {
                id,
                name,
                status: ToolStatus::Missing,
                detail: Some("tool is not available on PATH".into()),
                installation: None,
                update: None,
                diagnostics: Diagnostics::default(),
            };
        }
        Err(error) => {
            return ToolReport {
                id,
                name,
                status: ToolStatus::Failed,
                detail: Some(error.to_string()),
                installation: None,
                update: None,
                diagnostics: Diagnostics::default(),
            };
        }
    };

    let claims = resolve_claims(path_claims(&tool, refresh, context, &cache).await);
    let diagnostics = if refresh {
        Diagnostics::default()
    } else {
        Diagnostics {
            evidence: claims.evidence.clone(),
            conflicts: claims.conflicts.clone(),
        }
    };
    let installation = InstallationReport {
        current: tool.version.clone(),
        executable: tool.executable.clone(),
        source: claims.source.as_ref().map(|claim| claim.source.clone()),
        alternatives: alternate_installations(&tool, &cache, context).await,
    };
    let Some(updater_claim) = claims.updater else {
        return ToolReport {
            id,
            name,
            status: ToolStatus::Unmanaged,
            detail: Some(
                if claims.conflicts.is_empty() {
                    "no safe update manager claimed the active installation"
                } else {
                    "manager claims conflict at equal confidence"
                }
                .into(),
            ),
            installation: Some(installation),
            update: None,
            diagnostics,
        };
    };
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id() == updater_claim.manager)
        .copied()
        .expect("claim manager is registered");
    let snapshot = cache
        .get(manager.id().as_str())
        .and_then(OnceCell::get)
        .and_then(Option::as_ref)
        .cloned()
        .unwrap_or_else(|| empty_snapshot(manager));
    let latest = match manager.latest(&tool, &snapshot, context).await {
        Ok(version) => version,
        Err(error) => {
            return ToolReport {
                id,
                name,
                status: ToolStatus::Failed,
                detail: Some(error.to_string()),
                installation: Some(installation),
                update: None,
                diagnostics,
            };
        }
    };
    let ordering = match adapter.compare(&tool.version, &latest) {
        Ok(ordering) => ordering,
        Err(error) => {
            return ToolReport {
                id,
                name,
                status: ToolStatus::Failed,
                detail: Some(error.to_string()),
                installation: Some(installation),
                update: None,
                diagnostics,
            };
        }
    };
    let action = match manager.upgrade(&tool, &latest, &snapshot) {
        Ok(action) => action,
        Err(error) => {
            return ToolReport {
                id,
                name,
                status: ToolStatus::Failed,
                detail: Some(error.to_string()),
                installation: Some(installation),
                update: None,
                diagnostics,
            };
        }
    };
    ToolReport {
        id,
        name,
        status: if ordering == Ordering::Less {
            ToolStatus::Outdated
        } else {
            ToolStatus::Current
        },
        detail: None,
        installation: Some(installation),
        update: Some(UpdateReport {
            manager: updater_claim.manager,
            latest,
            action,
        }),
        diagnostics,
    }
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

async fn homebrew_inventory(
    refresh: bool,
    context: &ProviderContext<'_>,
    cache: &SnapshotCache,
) -> Vec<InventoryReport> {
    if context
        .execute_silent(&CommandSpec::new("/usr/bin/which", ["brew"]))
        .await
        .is_err()
    {
        return Vec::new();
    }
    let manager = install_manager_registry()
        .iter()
        .find(|manager| manager.id().as_str() == "homebrew")
        .copied()
        .expect("homebrew manager is registered");
    let snapshot = cache
        .get("homebrew")
        .expect("homebrew cache entry exists")
        .get_or_init(|| async {
            manager
                .snapshot(
                    context,
                    if refresh {
                        RefreshPolicy::Refresh
                    } else {
                        RefreshPolicy::Cached
                    },
                )
                .await
                .ok()
        })
        .await;
    if snapshot.is_none() {
        return vec![InventoryReport {
            id: "homebrew".into(),
            name: "Homebrew".into(),
            kind: "inventory".into(),
            status: ToolStatus::Failed,
            current: None,
            latest: None,
            action: None,
            detail: Some("Homebrew refresh failed".into()),
        }];
    }
    let output = match context
        .execute(
            "Checking Homebrew inventory",
            &CommandSpec::new("brew", ["outdated", "--json=v2"]),
        )
        .await
    {
        Ok(output) => output,
        Err(error) => {
            return vec![InventoryReport {
                id: "homebrew".into(),
                name: "Homebrew".into(),
                kind: "inventory".into(),
                status: ToolStatus::Failed,
                current: None,
                latest: None,
                action: None,
                detail: Some(error.to_string()),
            }];
        }
    };
    let parsed: BrewOutdated = match serde_json::from_str(&output.stdout) {
        Ok(parsed) => parsed,
        Err(error) => {
            return vec![InventoryReport {
                id: "homebrew".into(),
                name: "Homebrew".into(),
                kind: "inventory".into(),
                status: ToolStatus::Failed,
                current: None,
                latest: None,
                action: None,
                detail: Some(error.to_string()),
            }];
        }
    };
    let mut reports = Vec::new();
    for (kind, item) in parsed
        .formulae
        .into_iter()
        .map(|item| ("formula", item))
        .chain(parsed.casks.into_iter().map(|item| ("cask", item)))
    {
        let current = item
            .installed_versions
            .last()
            .and_then(|version| ToolVersion::new(version, Some(version.clone())).ok());
        let latest = item
            .current_version
            .as_ref()
            .and_then(|version| ToolVersion::new(version, Some(version.clone())).ok());
        let action = latest.as_ref().and_then(|latest| {
            HOMEBREW_MANAGER
                .inventory_upgrade(kind, &item.name, latest)
                .ok()
        });
        reports.push(InventoryReport {
            id: format!("brew:{kind}:{}", item.name),
            name: item.name,
            kind: kind.into(),
            status: ToolStatus::Outdated,
            current,
            latest,
            action,
            detail: None,
        });
    }
    reports.sort_by(|left, right| left.id.cmp(&right.id));
    reports
}

pub async fn check_all_with_context(
    config: &Config,
    refresh: bool,
    context: &ProviderContext<'_>,
) -> Result<CheckData> {
    let enabled = tool_registry()
        .iter()
        .copied()
        .filter(|adapter| {
            config
                .enabled_tools
                .iter()
                .any(|id| id == adapter.id().as_str())
        })
        .collect::<Vec<_>>();
    let cache = Arc::new(
        install_manager_registry()
            .iter()
            .map(|manager| (manager.id().to_string(), OnceCell::new()))
            .collect::<HashMap<_, _>>(),
    );
    prefetch_snapshots(refresh, context, &cache).await;
    let inventories = if config.enabled_inventories.iter().any(|id| id == "homebrew") {
        homebrew_inventory(refresh, context, &cache).await
    } else {
        Vec::new()
    };
    let mut tools = stream::iter(
        enabled
            .into_iter()
            .map(|adapter| report_tool(adapter, refresh, context, cache.clone())),
    )
    .buffered(4)
    .collect::<Vec<_>>()
    .await;
    tools.sort_by_key(|report| {
        tool_registry()
            .iter()
            .position(|adapter| adapter.id().as_str() == report.id)
            .unwrap_or(usize::MAX)
    });
    Ok(CheckData { tools, inventories })
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

pub async fn check_all(config: &Config, refresh: bool, ui: &Ui) -> Result<CheckData> {
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
    let installation = report
        .installation
        .as_ref()
        .context("report has no installation")?;
    let adapter = tool_registry()
        .iter()
        .find(|adapter| adapter.id().as_str() == report.id)
        .copied()
        .context("tool adapter not found")?;
    let detected = adapter.detect(context).await?;
    if detected.executable != installation.executable {
        bail!("active executable changed during verification");
    }
    let actual = detected.version;
    if let Some(update) = &report.update {
        verify_versions(
            update.action.target_mode,
            &installation.current,
            &update.action.expected_version,
            &actual,
            |left, right| adapter.compare(left, right),
        )?;
    }
    Ok(Some(actual.display().to_string()))
}

pub async fn verify(report: &ToolReport, config: &Config, ui: &Ui) -> Result<Option<String>> {
    let executor = SystemCommandExecutor {
        verbose: ui.mode() == crate::ui::FeedbackMode::Verbose,
    };
    let context = ProviderContext::new(&executor, ui, config.command_timeout_seconds);
    verify_with_context(report, &context).await
}

pub fn recovery_hint(report: &ToolReport) -> String {
    match report.update.as_ref().map(|update| update.manager.as_str()) {
        Some("homebrew") => "Run `brew doctor` and inspect the qualified formula or cask.".into(),
        Some("rustup") => "Run `rustup show` and `rustup check`.".into(),
        Some("mise") => "Run `mise doctor` and `mise current`.".into(),
        Some("npm" | "corepack") => {
            "Run `npm doctor` and inspect Corepack/global prefix state.".into()
        }
        _ => "Inspect PATH and reinstall the tool with its original manager.".into(),
    }
}

use serde::{Deserialize, Serialize};

pub mod command {
    use anyhow::{Result, bail};
    use serde::{Deserialize, Serialize};
    use std::{collections::BTreeMap, path::PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CommandSpec {
        pub program: String,
        pub args: Vec<String>,
        #[serde(skip, default)]
        pub accepted_exit_codes: Vec<i32>,
        #[serde(skip, default)]
        pub current_dir: Option<PathBuf>,
        #[serde(skip, default)]
        pub environment: BTreeMap<String, String>,
        #[serde(skip, default)]
        pub removed_environment: Vec<String>,
    }

    impl CommandSpec {
        pub fn new(
            program: impl Into<String>,
            args: impl IntoIterator<Item = impl Into<String>>,
        ) -> Self {
            Self {
                program: program.into(),
                args: args.into_iter().map(Into::into).collect(),
                accepted_exit_codes: Vec::new(),
                current_dir: None,
                environment: BTreeMap::new(),
                removed_environment: Vec::new(),
            }
        }

        pub fn accepting_exit_code(mut self, code: i32) -> Self {
            self.accepted_exit_codes.push(code);
            self
        }

        pub fn in_directory(mut self, path: impl Into<PathBuf>) -> Self {
            self.current_dir = Some(path.into());
            self
        }

        pub fn with_environment(
            mut self,
            key: impl Into<String>,
            value: impl Into<String>,
        ) -> Self {
            self.environment.insert(key.into(), value.into());
            self
        }

        pub fn removing_environment(mut self, key: impl Into<String>) -> Self {
            self.removed_environment.push(key.into());
            self
        }

        pub fn brew_inventory_upgrade(kind: &str, target: &str) -> Result<Self> {
            if target.trim().is_empty() {
                bail!("Homebrew upgrade requires an explicit target");
            }
            if !matches!(kind, "formula" | "cask") {
                bail!("Homebrew upgrade requires a formula or cask target kind");
            }
            Ok(Self::new("brew", ["upgrade", &format!("--{kind}"), target]))
        }

        pub fn display(&self) -> String {
            std::iter::once(self.program.as_str())
                .chain(self.args.iter().map(String::as_str))
                .map(|part| {
                    if part.contains([' ', '\'', '"']) {
                        format!("{:?}", part)
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

pub mod envelope {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ErrorDetail {
        pub code: String,
        pub target: Option<String>,
        pub message: String,
    }

    impl ErrorDetail {
        pub fn new(
            code: impl Into<String>,
            target: Option<impl Into<String>>,
            message: impl Into<String>,
        ) -> Self {
            Self {
                code: code.into(),
                target: target.map(Into::into),
                message: message.into(),
            }
        }
    }

    #[derive(Debug, Serialize)]
    pub struct Envelope<T: Serialize> {
        pub schema_version: u8,
        pub status: &'static str,
        pub data: T,
        pub errors: Vec<ErrorDetail>,
    }

    impl<T: Serialize> Envelope<T> {
        pub fn ok(data: T) -> Self {
            Self {
                schema_version: 2,
                status: "ok",
                data,
                errors: vec![],
            }
        }
        pub fn partial(data: T, errors: Vec<ErrorDetail>) -> Self {
            Self {
                schema_version: 2,
                status: "partial",
                data,
                errors,
            }
        }

        pub fn error(data: T, errors: Vec<ErrorDetail>) -> Self {
            Self {
                schema_version: 2,
                status: "error",
                data,
                errors,
            }
        }
    }
}

pub mod redact {
    pub fn redact(input: &str, home: Option<&str>) -> String {
        let mut output = input.to_string();
        if let Some(home) = home {
            if !home.is_empty() {
                output = output.replace(home, "~");
            }
        }
        for marker in [
            "token=",
            "TOKEN=",
            "password=",
            "PASSWORD=",
            "cookie=",
            "COOKIE=",
        ] {
            let mut cursor = 0;
            while let Some(relative) = output[cursor..].find(marker) {
                let start = cursor + relative;
                let value_start = start + marker.len();
                let end = output[value_start..]
                    .find(char::is_whitespace)
                    .map(|i| value_start + i)
                    .unwrap_or(output.len());
                output.replace_range(value_start..end, "[REDACTED]");
                cursor = value_start + "[REDACTED]".len();
            }
        }
        for marker in ["Bearer ", "Basic "] {
            let mut cursor = 0;
            while let Some(relative) = output[cursor..].find(marker) {
                let start = cursor + relative;
                let value_start = start + marker.len();
                let end = output[value_start..]
                    .find(char::is_whitespace)
                    .map(|i| value_start + i)
                    .unwrap_or(output.len());
                output.replace_range(value_start..end, "[REDACTED]");
                cursor = value_start + "[REDACTED]".len();
            }
        }
        output
    }
}

pub mod versions {
    pub fn version_number(output: &str) -> Option<String> {
        output.split_whitespace().find_map(|word| {
            let trimmed = word
                .trim_start_matches('v')
                .strip_prefix("go")
                .unwrap_or(word.trim_start_matches('v'))
                .trim_matches(|c: char| {
                    !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
                });
            (trimmed.chars().next()?.is_ascii_digit() && trimmed.contains('.'))
                .then(|| trimmed.to_string())
        })
    }
}

pub mod orchestrator {
    use anyhow::Result;
    use std::future::Future;

    pub async fn run_until_failure<I, T, F, Fut>(items: I, mut run: F) -> Result<()>
    where
        I: IntoIterator<Item = T>,
        F: FnMut(T) -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        for item in items {
            run(item).await?;
        }
        Ok(())
    }
}

pub mod upgrade {
    use crate::{ToolReport, ToolStatus};
    use anyhow::{Result, bail};

    pub fn upgrade_candidates(reports: &[ToolReport]) -> Vec<ToolReport> {
        reports
            .iter()
            .filter(|report| report.status == ToolStatus::Outdated && report.update.is_some())
            .cloned()
            .collect()
    }

    pub fn resolve_targets(reports: &[ToolReport], targets: &[String]) -> Result<Vec<ToolReport>> {
        let actionable = upgrade_candidates(reports);
        let selected: Vec<_> = actionable
            .into_iter()
            .filter(|report| {
                targets
                    .iter()
                    .any(|target| target == &report.id || target == &report.name)
            })
            .collect();
        for target in targets {
            if reports.iter().any(|report| {
                (report.id == *target || report.name == *target)
                    && report.status == ToolStatus::Missing
            }) {
                bail!("target `{target}` is not installed; upgrade does not install missing tools");
            }
            if !selected
                .iter()
                .any(|report| report.id == *target || report.name == *target)
            {
                bail!("target `{target}` is not actionable or was not found");
            }
        }
        Ok(selected)
    }
}

pub mod ui {
    use crate::{ToolStatus, command::CommandSpec, runner::CommandOutput};
    use anyhow::Result;
    use console::Style;
    use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
    use std::{io::IsTerminal, sync::Mutex, time::Duration};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FeedbackMode {
        Spinner,
        Plain,
        Silent,
        Verbose,
    }

    impl FeedbackMode {
        pub fn select(is_tty: bool, json: bool, verbose: bool) -> Self {
            if json {
                Self::Silent
            } else if verbose {
                Self::Verbose
            } else if is_tty {
                Self::Spinner
            } else {
                Self::Plain
            }
        }
    }

    pub fn spinner_template(colors: bool) -> &'static str {
        if colors {
            "{spinner:.cyan} {msg} {elapsed:.dim}"
        } else {
            "{spinner} {msg} {elapsed}"
        }
    }

    pub fn status_text(status: ToolStatus, colors: bool, width: usize) -> String {
        let text = format!("{:<width$}", format!("{status:?}").to_lowercase());
        let style = match status {
            ToolStatus::Current => Style::new().green(),
            ToolStatus::Outdated => Style::new().yellow(),
            ToolStatus::Missing | ToolStatus::Unmanaged => Style::new().yellow(),
            ToolStatus::Failed => Style::new().red(),
        };
        if colors {
            style.force_styling(true).apply_to(text).to_string()
        } else {
            text
        }
    }

    pub struct Ui {
        mode: FeedbackMode,
        colors: bool,
        progress: Mutex<ProgressState>,
    }

    #[derive(Default)]
    struct ProgressState {
        active: usize,
        bar: Option<ProgressBar>,
    }

    impl Ui {
        pub fn new(json: bool, verbose: bool, no_color: bool) -> Self {
            let is_tty = std::io::stderr().is_terminal();
            let colors = is_tty && !no_color && std::env::var_os("NO_COLOR").is_none();
            Self {
                mode: FeedbackMode::select(is_tty, json, verbose),
                colors,
                progress: Mutex::new(ProgressState::default()),
            }
        }

        pub fn mode(&self) -> FeedbackMode {
            self.mode
        }
        pub fn colors(&self) -> bool {
            self.colors
        }

        pub fn paint(&self, text: impl std::fmt::Display, style: Style) -> String {
            if self.colors {
                style.force_styling(true).apply_to(text).to_string()
            } else {
                text.to_string()
            }
        }

        pub(crate) fn start_progress(&self, label: &str) {
            match self.mode {
                FeedbackMode::Spinner => {
                    let mut progress = self.progress.lock().expect("progress mutex poisoned");
                    progress.active += 1;
                    if let Some(bar) = &progress.bar {
                        bar.set_message(label.to_string());
                        return;
                    }
                    let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
                    if let Ok(style) = ProgressStyle::with_template(spinner_template(self.colors)) {
                        bar.set_style(
                            style.tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
                        );
                    }
                    bar.set_message(label.to_string());
                    bar.enable_steady_tick(Duration::from_millis(80));
                    progress.bar = Some(bar);
                }
                FeedbackMode::Plain | FeedbackMode::Verbose => eprintln!("… {label}"),
                FeedbackMode::Silent => {}
            }
        }

        pub(crate) fn finish_progress(&self) {
            let mut progress = self.progress.lock().expect("progress mutex poisoned");
            progress.active = progress.active.saturating_sub(1);
            if progress.active == 0 {
                if let Some(bar) = progress.bar.take() {
                    bar.finish_and_clear();
                }
            }
        }

        pub async fn run_command(
            &self,
            label: &str,
            spec: &CommandSpec,
            seconds: u64,
        ) -> Result<CommandOutput> {
            self.start_progress(label);
            let result =
                crate::runner::run_with_output(spec, seconds, self.mode == FeedbackMode::Verbose)
                    .await;
            self.finish_progress();
            result
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Current,
    Outdated,
    Missing,
    Unmanaged,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlternativeInstallation {
    pub source: providers::SourceId,
    pub versions: Vec<providers::ToolVersion>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationReport {
    pub current: providers::ToolVersion,
    pub executable: String,
    pub source: Option<providers::SourceId>,
    pub alternatives: Vec<AlternativeInstallation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateReport {
    pub manager: providers::ManagerId,
    pub latest: providers::ToolVersion,
    pub action: providers::UpgradeAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub evidence: Vec<providers::ClaimEvidence>,
    pub conflicts: Vec<providers::ClaimEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolReport {
    pub id: String,
    pub name: String,
    pub status: ToolStatus,
    pub detail: Option<String>,
    pub installation: Option<InstallationReport>,
    pub update: Option<UpdateReport>,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryReport {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub status: ToolStatus,
    pub current: Option<providers::ToolVersion>,
    pub latest: Option<providers::ToolVersion>,
    pub action: Option<providers::UpgradeAction>,
    pub detail: Option<String>,
    #[serde(default)]
    pub scope: ResourceScope,
    pub installation_source: Option<String>,
    pub source_locator: Option<String>,
    pub update_manager: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<InventoryChange>,
    #[serde(skip, default)]
    pub runtime: InventoryRuntime,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResourceScope {
    #[default]
    System,
    Global,
    Project,
}

impl ResourceScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryChange {
    pub path: String,
    pub kind: InventoryChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InventoryChangeKind {
    Added,
    Modified,
    Removed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InventoryRuntime {
    pub canonical_path: Option<std::path::PathBuf>,
    pub receipt_path: Option<std::path::PathBuf>,
    pub project_root: Option<std::path::PathBuf>,
    pub manager_path: Option<std::path::PathBuf>,
    pub manager_version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckData {
    pub tools: Vec<ToolReport>,
    pub inventories: Vec<InventoryReport>,
}

pub mod config {
    use anyhow::{Context, Result, bail};
    use directories::ProjectDirs;
    use serde::{Deserialize, Serialize};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn missing_schema_version() -> u8 {
        1
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Config {
        /// Missing keys deserialize as 1 so unmigrated v1 files are not mistaken for v2.
        #[serde(default = "missing_schema_version")]
        pub schema_version: u8,
        pub enabled_tools: Vec<String>,
        pub disabled_tools: Vec<String>,
        pub enabled_inventories: Vec<String>,
        pub disabled_inventories: Vec<String>,
        pub tool_catalog_version: u32,
        pub inventory_catalog_version: u32,
        pub history_limit: usize,
        pub command_timeout_seconds: u64,
    }

    pub const TOOL_CATALOG_VERSION: u32 = 1;
    pub const INVENTORY_CATALOG_VERSION: u32 = 1;

    pub fn tool_catalog_entry_version(id: &str) -> u32 {
        match id {
            "rust" | "node" | "npm" | "pnpm" | "go" | "bun" | "deno" | "uv" => 1,
            _ => TOOL_CATALOG_VERSION,
        }
    }

    pub fn inventory_catalog_entry_version(id: &str) -> u32 {
        match id {
            "homebrew" => 0,
            "skills" => 1,
            _ => INVENTORY_CATALOG_VERSION,
        }
    }

    impl Default for Config {
        fn default() -> Self {
            Self {
                schema_version: 4,
                enabled_tools: Vec::new(),
                disabled_tools: Vec::new(),
                enabled_inventories: Vec::new(),
                disabled_inventories: Vec::new(),
                tool_catalog_version: 0,
                inventory_catalog_version: 0,
                history_limit: 500,
                command_timeout_seconds: 120,
            }
        }
    }

    pub fn app_dir() -> Result<PathBuf> {
        let home = ProjectDirs::from("dev", "Beacon", "Beacon")
            .context("cannot determine application directory")?
            .data_dir()
            .ancestors()
            .nth(1)
            .context("cannot determine Application Support directory")?
            .to_path_buf();
        Ok(home.join("Beacon"))
    }
    pub fn path() -> Result<PathBuf> {
        Ok(app_dir()?.join("config.toml"))
    }
    pub fn load_from(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let config: Config =
            toml::from_str(&fs::read_to_string(path)?).context("invalid Beacon config")?;
        match config.schema_version {
            4 => Ok(config),
            version if version > 4 => bail!("unsupported Beacon config schema version {version}"),
            version => bail!("Beacon config schema version {version} requires migration"),
        }
    }
    pub fn load() -> Result<Config> {
        load_from(&path()?)
    }
    pub fn save_to(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if path.exists() {
            use toml_edit::{Array, DocumentMut, value};

            let mut document = fs::read_to_string(path)?
                .parse::<DocumentMut>()
                .context("invalid Beacon config")?;
            let array = |items: &[String]| {
                let mut array = Array::new();
                for item in items {
                    array.push(item.as_str());
                }
                array
            };
            document["schema_version"] = value(config.schema_version as i64);
            document["enabled_tools"] = value(array(&config.enabled_tools));
            document["disabled_tools"] = value(array(&config.disabled_tools));
            document["enabled_inventories"] = value(array(&config.enabled_inventories));
            document["disabled_inventories"] = value(array(&config.disabled_inventories));
            document["tool_catalog_version"] = value(config.tool_catalog_version as i64);
            document["inventory_catalog_version"] = value(config.inventory_catalog_version as i64);
            document["history_limit"] = value(config.history_limit as i64);
            document["command_timeout_seconds"] = value(config.command_timeout_seconds as i64);
            atomic_write(path, document.to_string().as_bytes())?;
        } else {
            atomic_write(path, toml::to_string_pretty(config)?.as_bytes())?;
        }
        Ok(())
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("config path has no file name")?;
        let temporary = path.with_file_name(format!("{file_name}.tmp"));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    fn migrate_v1(path: &Path, source: &str) -> Result<()> {
        use toml_edit::{Array, DocumentMut, value};

        let mut document = source
            .parse::<DocumentMut>()
            .context("invalid Beacon config")?;
        let version = document
            .get("schema_version")
            .and_then(|item| item.as_integer())
            .unwrap_or(1);
        if version > 4 {
            bail!("unsupported Beacon config schema version {version}");
        }
        if version >= 2 {
            return Ok(());
        }

        let backup = path.with_file_name("config.toml.v1.bak");
        if !backup.exists() {
            atomic_write(&backup, source.as_bytes())?;
        }
        let mut tools = Array::new();
        let mut homebrew_was_enabled = false;
        if let Some(existing) = document
            .get("enabled_tools")
            .and_then(|item| item.as_array())
        {
            for tool in existing.iter().filter_map(|item| item.as_str()) {
                if tool == "homebrew" {
                    homebrew_was_enabled = true;
                } else {
                    tools.push(tool);
                }
            }
        } else {
            homebrew_was_enabled = true;
            for tool in ["rust", "node", "npm", "pnpm", "go", "bun", "deno", "uv"] {
                tools.push(tool);
            }
        }
        let mut inventories = Array::new();
        if homebrew_was_enabled {
            inventories.push("homebrew");
        }
        document["schema_version"] = value(2);
        document["enabled_tools"] = value(tools);
        document["enabled_inventories"] = value(inventories);
        document.remove("preferred_install_manager");
        atomic_write(path, document.to_string().as_bytes())?;
        Ok(())
    }

    fn migrate_v2(path: &Path, source: &str) -> Result<()> {
        use toml_edit::{Array, DocumentMut, value};

        let mut document = source
            .parse::<DocumentMut>()
            .context("invalid Beacon config")?;
        let version = document
            .get("schema_version")
            .and_then(|item| item.as_integer())
            .unwrap_or(1);
        if version > 4 {
            bail!("unsupported Beacon config schema version {version}");
        }
        if version != 2 {
            return Ok(());
        }

        let backup = path.with_file_name("config.toml.v2.bak");
        if !backup.exists() {
            atomic_write(&backup, source.as_bytes())?;
        }
        let homebrew_enabled = document
            .get("enabled_inventories")
            .and_then(|item| item.as_array())
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("homebrew")));
        let mut disabled_inventories = Array::new();
        if !homebrew_enabled {
            disabled_inventories.push("homebrew");
        }
        document["schema_version"] = value(3);
        document["disabled_tools"] = value(Array::new());
        document["disabled_inventories"] = value(disabled_inventories);
        document["tool_catalog_version"] = value(0);
        atomic_write(path, document.to_string().as_bytes())?;
        Ok(())
    }

    fn migrate_v3(path: &Path, source: &str) -> Result<()> {
        use toml_edit::{DocumentMut, value};

        let mut document = source
            .parse::<DocumentMut>()
            .context("invalid Beacon config")?;
        let version = document
            .get("schema_version")
            .and_then(|item| item.as_integer())
            .unwrap_or(1);
        if version > 4 {
            bail!("unsupported Beacon config schema version {version}");
        }
        if version != 3 {
            return Ok(());
        }

        let backup = path.with_file_name("config.toml.v3.bak");
        if !backup.exists() {
            atomic_write(&backup, source.as_bytes())?;
        }
        document["schema_version"] = value(4);
        document["inventory_catalog_version"] = value(0);
        atomic_write(path, document.to_string().as_bytes())?;
        Ok(())
    }

    pub fn initialize_catalog(
        config: &mut Config,
        available_tools: &[String],
        available_inventories: &[String],
    ) -> bool {
        let mut changed = false;
        if config.tool_catalog_version < TOOL_CATALOG_VERSION {
            if config.tool_catalog_version == 0 {
                config.enabled_tools = available_tools
                    .iter()
                    .filter(|id| !config.disabled_tools.contains(id))
                    .cloned()
                    .collect();
            } else {
                for id in available_tools {
                    let introduced_in = tool_catalog_entry_version(id);
                    if introduced_in > config.tool_catalog_version
                        && !config.disabled_tools.contains(id)
                        && !config.enabled_tools.contains(id)
                    {
                        config.enabled_tools.push(id.clone());
                    }
                }
            }
            config.tool_catalog_version = TOOL_CATALOG_VERSION;
            changed = true;
        }

        if config.inventory_catalog_version < INVENTORY_CATALOG_VERSION {
            if config.inventory_catalog_version == 0
                && config.enabled_inventories.is_empty()
                && config.disabled_inventories.is_empty()
            {
                config.enabled_inventories = available_inventories.to_vec();
            } else {
                for id in available_inventories {
                    let introduced_in = inventory_catalog_entry_version(id);
                    if introduced_in > config.inventory_catalog_version
                        && !config.disabled_inventories.contains(id)
                        && !config.enabled_inventories.contains(id)
                    {
                        config.enabled_inventories.push(id.clone());
                    }
                }
            }
            config.inventory_catalog_version = INVENTORY_CATALOG_VERSION;
            changed = true;
        }
        changed
    }
    pub fn ensure() -> Result<(Config, PathBuf)> {
        let path = path()?;
        ensure_at(&path)
    }

    pub fn ensure_at(path: &Path) -> Result<(Config, PathBuf)> {
        if path.exists() {
            let source = fs::read_to_string(path)?;
            migrate_v1(path, &source)?;
            let source = fs::read_to_string(path)?;
            migrate_v2(path, &source)?;
            let source = fs::read_to_string(path)?;
            migrate_v3(path, &source)?;
        }
        let config = load_from(path)?;
        if !path.exists() {
            save_to(&config, path)?;
        }
        Ok((config, path.to_path_buf()))
    }
}

pub mod runner {
    use crate::command::CommandSpec;
    use anyhow::{Context, Result, bail};
    use std::{
        io::{Read, Seek, Write},
        process::Stdio,
        time::Duration,
    };
    use tokio::{
        io::{AsyncRead, AsyncReadExt},
        process::Command,
        time::timeout,
    };

    #[derive(Debug)]
    pub struct CommandOutput {
        pub stdout: String,
        pub stderr: String,
    }

    fn prepare_command(spec: &CommandSpec) -> Command {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = &spec.current_dir {
            command.current_dir(path);
        }
        command.envs(&spec.environment);
        for key in &spec.removed_environment {
            command.env_remove(key);
        }
        command
    }

    fn command_output(
        spec: &CommandSpec,
        status: std::process::ExitStatus,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        home: Option<&str>,
    ) -> Result<CommandOutput> {
        let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        if !status.success()
            && !status
                .code()
                .is_some_and(|code| spec.accepted_exit_codes.contains(&code))
        {
            let message = format!("{} failed ({}): {}", spec.display(), status, stderr);
            bail!("{}", crate::redact::redact(&message, home));
        }
        Ok(CommandOutput { stdout, stderr })
    }

    pub async fn run(spec: &CommandSpec, seconds: u64) -> Result<CommandOutput> {
        run_with_output(spec, seconds, false).await
    }

    /// Captures machine output through a regular file so package CLIs that force an early
    /// process exit cannot lose buffered JSON at the stdout pipe boundary.
    pub async fn run_machine_output(spec: &CommandSpec, seconds: u64) -> Result<CommandOutput> {
        let mut stdout_file = tempfile::tempfile()?;
        let mut command = prepare_command(spec);
        command.stdout(Stdio::from(stdout_file.try_clone()?));
        let mut child = command.spawn()?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture command stderr")?;
        let home = std::env::var("HOME").ok();
        let (status, stderr) = timeout(Duration::from_secs(seconds), async {
            tokio::try_join!(
                async { Ok::<_, anyhow::Error>(child.wait().await?) },
                capture(stderr, false, home.as_deref())
            )
        })
        .await
        .with_context(|| format!("command timed out: {}", spec.display()))??;
        stdout_file.rewind()?;
        let mut stdout = Vec::new();
        stdout_file.read_to_end(&mut stdout)?;
        command_output(spec, status, stdout, stderr, home.as_deref())
    }

    pub fn sanitize_verbose_line(line: &str, home: Option<&str>) -> String {
        crate::redact::redact(line, home)
    }

    struct StreamingRedactor {
        pending: Vec<u8>,
        home: Option<Vec<u8>>,
        in_secret: bool,
    }

    impl StreamingRedactor {
        fn new(home: Option<&str>) -> Self {
            Self {
                pending: Vec::new(),
                home: home
                    .filter(|value| !value.is_empty())
                    .map(|value| value.as_bytes().to_vec()),
                in_secret: false,
            }
        }

        fn patterns(&self) -> Vec<(Vec<u8>, bool)> {
            let mut patterns = vec![
                (b"token=".to_vec(), true),
                (b"TOKEN=".to_vec(), true),
                (b"password=".to_vec(), true),
                (b"PASSWORD=".to_vec(), true),
                (b"cookie=".to_vec(), true),
                (b"COOKIE=".to_vec(), true),
                (b"Bearer ".to_vec(), true),
                (b"Basic ".to_vec(), true),
            ];
            if let Some(home) = &self.home {
                patterns.push((home.clone(), false));
            }
            patterns
        }

        fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
            self.pending.extend_from_slice(chunk);
            let mut output = Vec::new();
            loop {
                if self.in_secret {
                    if let Some(end) = self
                        .pending
                        .iter()
                        .position(|byte| byte.is_ascii_whitespace())
                    {
                        let delimiter = self.pending[end];
                        self.pending.drain(..=end);
                        output.push(delimiter);
                        self.in_secret = false;
                        continue;
                    }
                    self.pending.clear();
                    break;
                }

                let patterns = self.patterns();
                let found = patterns
                    .iter()
                    .filter_map(|(pattern, secret)| {
                        self.pending
                            .windows(pattern.len())
                            .position(|window| window == *pattern)
                            .map(|index| (index, pattern.clone(), *secret))
                    })
                    .min_by_key(|(index, _, _)| *index);
                if let Some((index, pattern, secret)) = found {
                    output.extend(self.pending.drain(..index));
                    self.pending.drain(..pattern.len());
                    if secret {
                        output.extend_from_slice(&pattern);
                        output.extend_from_slice(b"[REDACTED]");
                        self.in_secret = true;
                    } else {
                        output.push(b'~');
                    }
                    continue;
                }

                let keep = patterns
                    .iter()
                    .map(|(pattern, _)| pattern.len().saturating_sub(1))
                    .max()
                    .unwrap_or(0);
                if self.pending.len() > keep {
                    let emit = self.pending.len() - keep;
                    output.extend(self.pending.drain(..emit));
                }
                break;
            }
            output
        }

        fn finish(mut self) -> Vec<u8> {
            if self.in_secret {
                Vec::new()
            } else {
                std::mem::take(&mut self.pending)
            }
        }

        #[cfg(test)]
        fn buffered_len(&self) -> usize {
            self.pending.len()
        }
    }

    async fn capture(
        mut stream: impl AsyncRead + Unpin,
        verbose: bool,
        home: Option<&str>,
    ) -> Result<Vec<u8>> {
        let mut captured = Vec::new();
        let mut redactor = StreamingRedactor::new(home);
        let mut chunk = [0_u8; 4096];
        loop {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            captured.extend_from_slice(&chunk[..read]);
            if verbose {
                std::io::stderr().write_all(&redactor.push(&chunk[..read]))?;
                std::io::stderr().flush()?;
            }
        }
        if verbose {
            std::io::stderr().write_all(&redactor.finish())?;
            std::io::stderr().flush()?;
        }
        Ok(captured)
    }

    pub async fn run_with_output(
        spec: &CommandSpec,
        seconds: u64,
        verbose: bool,
    ) -> Result<CommandOutput> {
        let mut command = prepare_command(spec);
        command.stdout(Stdio::piped());
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .context("failed to capture command stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture command stderr")?;
        let home = std::env::var("HOME").ok();
        let (status, stdout, stderr) = timeout(Duration::from_secs(seconds), async {
            tokio::try_join!(
                async { Ok::<_, anyhow::Error>(child.wait().await?) },
                capture(stdout, verbose, home.as_deref()),
                capture(stderr, verbose, home.as_deref())
            )
        })
        .await
        .with_context(|| format!("command timed out: {}", spec.display()))??;
        command_output(spec, status, stdout, stderr, home.as_deref())
    }

    #[cfg(test)]
    mod tests {
        use super::{StreamingRedactor, run_with_output};
        use crate::command::CommandSpec;

        #[test]
        fn streaming_redactor_handles_secrets_split_across_chunks() {
            let mut redactor = StreamingRedactor::new(Some("/Users/alice"));
            let mut output = Vec::new();
            output.extend(redactor.push(b"token=sec"));
            output.extend(redactor.push(b"ret next /Users/"));
            output.extend(redactor.push(b"alice/project"));
            output.extend(redactor.finish());
            let output = String::from_utf8(output).unwrap();

            assert_eq!(output, "token=[REDACTED] next ~/project");
        }

        #[test]
        fn streaming_redactor_emits_bounded_partial_lines() {
            let mut redactor = StreamingRedactor::new(None);
            let output = redactor.push(b"downloading a long status without a newline");

            assert!(!output.is_empty());
            assert!(redactor.buffered_len() < 16);
        }

        #[tokio::test]
        async fn nonzero_exit_errors_are_redacted() {
            let home = std::env::var("HOME").unwrap();
            let command = CommandSpec::new(
                "sh",
                [
                    "-c",
                    "printf 'token=secret %s/private' \"$HOME\" >&2; exit 7",
                ],
            );

            let error = run_with_output(&command, 5, false)
                .await
                .unwrap_err()
                .to_string();

            assert!(!error.contains("secret"));
            assert!(!error.contains(&home));
            assert!(error.contains("[REDACTED]"));
        }
    }
}

pub mod store {
    use crate::CheckData;
    use anyhow::{Result, bail};
    use chrono::Utc;
    use rusqlite::{Connection, Transaction, params};
    use serde::Serialize;
    use std::path::Path;

    fn history_v2_ddl(table: &str) -> String {
        format!(
            "CREATE TABLE {table} (
                    id INTEGER PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    tool TEXT NOT NULL,
                    old_version TEXT,
                    new_version TEXT,
                    installation_source TEXT NOT NULL DEFAULT 'unknown',
                    update_manager TEXT NOT NULL DEFAULT 'unknown',
                    status TEXT NOT NULL,
                    summary TEXT NOT NULL
                )"
        )
    }

    fn history_v3_ddl(table: &str) -> String {
        format!(
            "CREATE TABLE {table} (
                    id INTEGER PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    tool TEXT NOT NULL,
                    old_version TEXT,
                    new_version TEXT,
                    installation_source TEXT NOT NULL DEFAULT 'unknown',
                    update_manager TEXT NOT NULL DEFAULT 'unknown',
                    resource_scope TEXT NOT NULL DEFAULT 'system',
                    scope_locator TEXT,
                    status TEXT NOT NULL,
                    summary TEXT NOT NULL
                )"
        )
    }

    fn snapshots_v2_ddl(table: &str) -> String {
        format!(
            "CREATE TABLE {table} (
                    id INTEGER PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    payload_schema_version INTEGER NOT NULL DEFAULT 2
                )"
        )
    }

    #[derive(Debug, Serialize)]
    pub struct HistoryEntry {
        pub id: i64,
        pub created_at: String,
        pub operation: String,
        pub tool: String,
        pub old_version: Option<String>,
        pub new_version: Option<String>,
        pub installation_source: String,
        pub update_manager: String,
        pub resource_scope: String,
        pub scope_locator: Option<String>,
        pub status: String,
        pub summary: String,
    }

    #[derive(Debug, Serialize)]
    pub struct SnapshotEntry {
        pub id: i64,
        pub created_at: String,
        pub payload: String,
        pub payload_schema_version: u8,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SkillBaseline {
        pub receipt_fingerprint: String,
        pub content_revision: String,
    }

    pub struct Store {
        connection: Connection,
    }
    impl Store {
        pub fn open(path: &Path) -> Result<Self> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut store = Self {
                connection: Connection::open(path)?,
            };
            store.migrate()?;
            Ok(store)
        }

        fn migrate(&mut self) -> Result<()> {
            let current_version: i64 =
                self.connection
                    .pragma_query_value(None, "user_version", |row| row.get(0))?;
            if current_version > 3 {
                bail!("unsupported Beacon database schema version {current_version}");
            }
            self.connection.execute_batch("PRAGMA journal_mode=WAL;")?;
            // Always enforce the v2 physical schema. A prior ALTER-only path could
            // stamp user_version=2 while leaving history.manager NOT NULL, which
            // breaks subsequent inserts; heal that intermediate shape on open.
            let transaction = self.connection.transaction()?;
            migrate_history_to_v2(&transaction)?;
            migrate_history_to_v3(&transaction)?;
            migrate_snapshots_to_v2(&transaction)?;
            migrate_skill_baselines(&transaction)?;
            transaction.pragma_update(None, "user_version", 3)?;
            transaction.commit()?;
            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn record(
            &self,
            operation: &str,
            tool: &str,
            old: Option<&str>,
            new: Option<&str>,
            installation_source: &str,
            update_manager: &str,
            status: &str,
            summary: &str,
        ) -> Result<()> {
            self.record_scoped(
                operation,
                tool,
                old,
                new,
                installation_source,
                update_manager,
                "system",
                None,
                status,
                summary,
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn record_scoped(
            &self,
            operation: &str,
            tool: &str,
            old: Option<&str>,
            new: Option<&str>,
            installation_source: &str,
            update_manager: &str,
            resource_scope: &str,
            scope_locator: Option<&str>,
            status: &str,
            summary: &str,
        ) -> Result<()> {
            self.connection.execute(
                "INSERT INTO history(created_at,operation,tool,old_version,new_version,installation_source,update_manager,resource_scope,scope_locator,status,summary) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    Utc::now().to_rfc3339(),
                    operation,
                    tool,
                    old,
                    new,
                    installation_source,
                    update_manager,
                    resource_scope,
                    scope_locator,
                    status,
                    summary
                ],
            )?;
            Ok(())
        }

        pub fn snapshot(&self, reports: &CheckData) -> Result<()> {
            self.connection.execute(
                "INSERT INTO snapshots(created_at,payload,payload_schema_version) VALUES(?,?,2)",
                params![Utc::now().to_rfc3339(), serde_json::to_string(reports)?],
            )?;
            Ok(())
        }

        pub fn skill_baseline(
            &self,
            resource_scope: &str,
            scope_locator: &str,
            skill_name: &str,
        ) -> Result<Option<SkillBaseline>> {
            let mut statement = self.connection.prepare(
                "SELECT receipt_fingerprint, content_revision
                 FROM skill_baselines
                 WHERE resource_scope = ? AND scope_locator = ? AND skill_name = ?",
            )?;
            let mut rows = statement.query(params![resource_scope, scope_locator, skill_name])?;
            Ok(rows
                .next()?
                .map(|row| {
                    Ok::<_, rusqlite::Error>(SkillBaseline {
                        receipt_fingerprint: row.get(0)?,
                        content_revision: row.get(1)?,
                    })
                })
                .transpose()?)
        }

        pub fn upsert_skill_baseline(
            &self,
            resource_scope: &str,
            scope_locator: &str,
            skill_name: &str,
            receipt_fingerprint: &str,
            content_revision: &str,
        ) -> Result<()> {
            self.connection.execute(
                "INSERT INTO skill_baselines(
                    resource_scope, scope_locator, skill_name, receipt_fingerprint,
                    content_revision, observed_at
                 ) VALUES(?,?,?,?,?,?)
                 ON CONFLICT(resource_scope, scope_locator, skill_name) DO UPDATE SET
                    receipt_fingerprint = excluded.receipt_fingerprint,
                    content_revision = excluded.content_revision,
                    observed_at = excluded.observed_at",
                params![
                    resource_scope,
                    scope_locator,
                    skill_name,
                    receipt_fingerprint,
                    content_revision,
                    Utc::now().to_rfc3339()
                ],
            )?;
            Ok(())
        }

        pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
            let mut statement = self.connection.prepare(
                "SELECT id,created_at,operation,tool,old_version,new_version,installation_source,update_manager,resource_scope,scope_locator,status,summary FROM history ORDER BY id DESC LIMIT ?",
            )?;
            Ok(statement
                .query_map([limit as i64], |row| {
                    Ok(HistoryEntry {
                        id: row.get(0)?,
                        created_at: row.get(1)?,
                        operation: row.get(2)?,
                        tool: row.get(3)?,
                        old_version: row.get(4)?,
                        new_version: row.get(5)?,
                        installation_source: row.get(6)?,
                        update_manager: row.get(7)?,
                        resource_scope: row.get(8)?,
                        scope_locator: row.get(9)?,
                        status: row.get(10)?,
                        summary: row.get(11)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        }

        pub fn snapshots(&self, limit: usize) -> Result<Vec<SnapshotEntry>> {
            let mut statement = self.connection.prepare(
                "SELECT id,created_at,payload,payload_schema_version FROM snapshots ORDER BY id DESC LIMIT ?",
            )?;
            Ok(statement
                .query_map([limit as i64], |row| {
                    Ok(SnapshotEntry {
                        id: row.get(0)?,
                        created_at: row.get(1)?,
                        payload: row.get(2)?,
                        payload_schema_version: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        }

        pub fn prune(&self, limit: usize) -> Result<()> {
            self.connection.execute(
                "DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY id DESC LIMIT ?)",
                [limit as i64],
            )?;
            Ok(())
        }

        pub fn schema_version(&self) -> Result<u8> {
            Ok(self
                .connection
                .pragma_query_value(None, "user_version", |row| row.get(0))?)
        }
    }

    fn migrate_history_to_v2(transaction: &Transaction<'_>) -> Result<()> {
        if !table_exists(transaction, "history")? {
            transaction.execute_batch(&history_v2_ddl("history"))?;
            return Ok(());
        }

        let columns = table_columns(transaction, "history")?;
        require_history_columns(&columns)?;
        let has_manager = columns.iter().any(|column| column == "manager");
        let has_source = columns.iter().any(|column| column == "installation_source");
        let has_updater = columns.iter().any(|column| column == "update_manager");
        if !has_manager && has_source && has_updater {
            return Ok(());
        }

        transaction.execute_batch("DROP TABLE IF EXISTS history_v2_migration;")?;
        transaction.execute_batch(&history_v2_ddl("history_v2_migration"))?;

        // require_history_columns guarantees manager and/or installation_source exist.
        let source_expr = if has_source && has_manager {
            "CASE WHEN installation_source = 'unknown' THEN manager ELSE installation_source END"
        } else if has_source {
            "installation_source"
        } else {
            "manager"
        };
        let updater_expr = if has_updater {
            "update_manager"
        } else {
            "'unknown'"
        };

        transaction.execute(
            &format!(
                "INSERT INTO history_v2_migration(
                    id, created_at, operation, tool, old_version, new_version,
                    installation_source, update_manager, status, summary
                )
                SELECT
                    id, created_at, operation, tool, old_version, new_version,
                    {source_expr}, {updater_expr}, status, summary
                FROM history"
            ),
            [],
        )?;
        transaction.execute_batch(
            "DROP TABLE history;
             ALTER TABLE history_v2_migration RENAME TO history;",
        )?;
        Ok(())
    }

    fn migrate_history_to_v3(transaction: &Transaction<'_>) -> Result<()> {
        let columns = table_columns(transaction, "history")?;
        require_history_columns(&columns)?;
        let has_scope = columns.iter().any(|column| column == "resource_scope");
        let has_locator = columns.iter().any(|column| column == "scope_locator");
        if has_scope && has_locator {
            return Ok(());
        }
        if has_scope || has_locator {
            bail!("cannot migrate history table: incomplete scoped history columns");
        }

        transaction.execute_batch("DROP TABLE IF EXISTS history_v3_migration;")?;
        transaction.execute_batch(&history_v3_ddl("history_v3_migration"))?;
        transaction.execute_batch(
            "INSERT INTO history_v3_migration(
                id, created_at, operation, tool, old_version, new_version,
                installation_source, update_manager, resource_scope, scope_locator,
                status, summary
            )
            SELECT
                id, created_at, operation, tool, old_version, new_version,
                installation_source, update_manager, 'system', NULL,
                status, summary
            FROM history;
            DROP TABLE history;
            ALTER TABLE history_v3_migration RENAME TO history;",
        )?;
        Ok(())
    }

    fn migrate_snapshots_to_v2(transaction: &Transaction<'_>) -> Result<()> {
        if !table_exists(transaction, "snapshots")? {
            transaction.execute_batch(&snapshots_v2_ddl("snapshots"))?;
            return Ok(());
        }

        let columns = table_columns(transaction, "snapshots")?;
        require_snapshot_columns(&columns)?;
        if columns
            .iter()
            .any(|column| column == "payload_schema_version")
        {
            return Ok(());
        }

        transaction.execute_batch("DROP TABLE IF EXISTS snapshots_v2_migration;")?;
        transaction.execute_batch(&snapshots_v2_ddl("snapshots_v2_migration"))?;
        transaction.execute(
            "INSERT INTO snapshots_v2_migration(id, created_at, payload, payload_schema_version)
             SELECT id, created_at, payload, 1 FROM snapshots",
            [],
        )?;
        transaction.execute_batch(
            "DROP TABLE snapshots;
             ALTER TABLE snapshots_v2_migration RENAME TO snapshots;",
        )?;
        Ok(())
    }

    fn migrate_skill_baselines(transaction: &Transaction<'_>) -> Result<()> {
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS skill_baselines (
                resource_scope TEXT NOT NULL,
                scope_locator TEXT NOT NULL DEFAULT '',
                skill_name TEXT NOT NULL,
                receipt_fingerprint TEXT NOT NULL,
                content_revision TEXT NOT NULL,
                observed_at TEXT NOT NULL,
                PRIMARY KEY(resource_scope, scope_locator, skill_name)
            );",
        )?;
        Ok(())
    }

    fn require_history_columns(columns: &[String]) -> Result<()> {
        for required in [
            "id",
            "created_at",
            "operation",
            "tool",
            "old_version",
            "new_version",
            "status",
            "summary",
        ] {
            if !columns.iter().any(|column| column == required) {
                bail!("cannot migrate history table: missing column {required}");
            }
        }
        if !columns.iter().any(|column| column == "manager")
            && !columns.iter().any(|column| column == "installation_source")
        {
            bail!("cannot migrate history table: missing manager ownership columns");
        }
        Ok(())
    }

    fn require_snapshot_columns(columns: &[String]) -> Result<()> {
        for required in ["id", "created_at", "payload"] {
            if !columns.iter().any(|column| column == required) {
                bail!("cannot migrate snapshots table: missing column {required}");
            }
        }
        Ok(())
    }

    fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
            [table],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        Ok(statement
            .query_map([], |row| row.get(1))?
            .collect::<std::result::Result<Vec<String>, _>>()?)
    }
}

pub mod agent_skills;
pub mod providers;

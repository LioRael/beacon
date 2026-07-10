use serde::{Deserialize, Serialize};

pub mod command {
    use anyhow::{Result, bail};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CommandSpec {
        pub program: String,
        pub args: Vec<String>,
    }

    impl CommandSpec {
        pub fn new(
            program: impl Into<String>,
            args: impl IntoIterator<Item = impl Into<String>>,
        ) -> Self {
            Self {
                program: program.into(),
                args: args.into_iter().map(Into::into).collect(),
            }
        }

        pub fn brew_upgrade(target: &str) -> Result<Self> {
            if target.trim().is_empty() {
                bail!("Homebrew upgrade requires an explicit target");
            }
            Ok(Self::new("brew", ["upgrade", target]))
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
            if verbose {
                Self::Verbose
            } else if json {
                Self::Silent
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
                FeedbackMode::Plain => eprintln!("… {label}"),
                FeedbackMode::Silent | FeedbackMode::Verbose => {}
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

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Config {
        pub schema_version: u8,
        pub enabled_tools: Vec<String>,
        pub enabled_inventories: Vec<String>,
        pub history_limit: usize,
        pub command_timeout_seconds: u64,
    }

    impl Default for Config {
        fn default() -> Self {
            Self {
                schema_version: 2,
                enabled_tools: vec![
                    "rust".into(),
                    "node".into(),
                    "npm".into(),
                    "pnpm".into(),
                    "go".into(),
                    "bun".into(),
                    "deno".into(),
                    "uv".into(),
                ],
                enabled_inventories: vec!["homebrew".into()],
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
        if config.schema_version > 2 {
            bail!(
                "unsupported Beacon config schema version {}",
                config.schema_version
            );
        }
        Ok(config)
    }
    pub fn load() -> Result<Config> {
        load_from(&path()?)
    }
    pub fn save_to(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        atomic_write(path, toml::to_string_pretty(config)?.as_bytes())?;
        Ok(())
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
        let temporary = path.with_extension("toml.tmp");
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
        if version > 2 {
            bail!("unsupported Beacon config schema version {version}");
        }
        if version == 2 {
            return Ok(());
        }

        let backup = path.with_file_name("config.toml.v1.bak");
        if !backup.exists() {
            fs::write(&backup, source)?;
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
            for tool in Config::default().enabled_tools {
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
    pub fn ensure() -> Result<(Config, PathBuf)> {
        let path = path()?;
        ensure_at(&path)
    }

    pub fn ensure_at(path: &Path) -> Result<(Config, PathBuf)> {
        if path.exists() {
            let source = fs::read_to_string(path)?;
            migrate_v1(path, &source)?;
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
    use std::{io::Write, process::Stdio, time::Duration};
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

    pub async fn run(spec: &CommandSpec, seconds: u64) -> Result<CommandOutput> {
        run_with_output(spec, seconds, false).await
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
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
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
        let stdout = String::from_utf8_lossy(&stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
        if !status.success() {
            let message = format!("{} failed ({}): {}", spec.display(), status, stderr);
            bail!("{}", crate::redact::redact(&message, home.as_deref()));
        }
        Ok(CommandOutput { stdout, stderr })
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
    use anyhow::Result;
    use chrono::Utc;
    use rusqlite::{Connection, params};
    use serde::Serialize;
    use std::path::Path;

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
        pub status: String,
        pub summary: String,
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
            self.connection.execute_batch("PRAGMA journal_mode=WAL;")?;
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(
                "CREATE TABLE IF NOT EXISTS history (
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
                );
                CREATE TABLE IF NOT EXISTS snapshots (
                    id INTEGER PRIMARY KEY,
                    created_at TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    payload_schema_version INTEGER NOT NULL DEFAULT 2
                );",
            )?;
            let history_columns = table_columns(&transaction, "history")?;
            if !history_columns
                .iter()
                .any(|column| column == "installation_source")
            {
                transaction.execute("ALTER TABLE history ADD COLUMN installation_source TEXT NOT NULL DEFAULT 'unknown'", [])?;
            }
            if !history_columns
                .iter()
                .any(|column| column == "update_manager")
            {
                transaction.execute(
                    "ALTER TABLE history ADD COLUMN update_manager TEXT NOT NULL DEFAULT 'unknown'",
                    [],
                )?;
            }
            if history_columns.iter().any(|column| column == "manager") {
                transaction.execute("UPDATE history SET installation_source = manager WHERE installation_source = 'unknown'", [])?;
            }
            let snapshot_columns = table_columns(&transaction, "snapshots")?;
            if !snapshot_columns
                .iter()
                .any(|column| column == "payload_schema_version")
            {
                transaction.execute("ALTER TABLE snapshots ADD COLUMN payload_schema_version INTEGER NOT NULL DEFAULT 1", [])?;
            }
            transaction.pragma_update(None, "user_version", 2)?;
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
            self.connection.execute("INSERT INTO history(created_at,operation,tool,old_version,new_version,installation_source,update_manager,status,summary) VALUES(?,?,?,?,?,?,?,?,?)", params![Utc::now().to_rfc3339(), operation, tool, old, new, installation_source, update_manager, status, summary])?;
            Ok(())
        }
        pub fn snapshot(&self, reports: &CheckData) -> Result<()> {
            self.connection.execute(
                "INSERT INTO snapshots(created_at,payload,payload_schema_version) VALUES(?,?,2)",
                params![Utc::now().to_rfc3339(), serde_json::to_string(reports)?],
            )?;
            Ok(())
        }
        pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
            let mut statement = self.connection.prepare("SELECT id,created_at,operation,tool,old_version,new_version,installation_source,update_manager,status,summary FROM history ORDER BY id DESC LIMIT ?")?;
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
                        status: row.get(8)?,
                        summary: row.get(9)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        }
        pub fn prune(&self, limit: usize) -> Result<()> {
            self.connection.execute("DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY id DESC LIMIT ?)", [limit as i64])?;
            Ok(())
        }

        pub fn schema_version(&self) -> Result<u8> {
            Ok(self
                .connection
                .pragma_query_value(None, "user_version", |row| row.get(0))?)
        }
    }

    fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        Ok(statement
            .query_map([], |row| row.get(1))?
            .collect::<std::result::Result<Vec<String>, _>>()?)
    }
}

pub mod providers;

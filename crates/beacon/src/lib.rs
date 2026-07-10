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
    use serde::Serialize;

    #[derive(Debug, Serialize)]
    pub struct Envelope<T: Serialize> {
        pub schema_version: u8,
        pub status: &'static str,
        pub data: T,
        pub errors: Vec<String>,
    }

    impl<T: Serialize> Envelope<T> {
        pub fn ok(data: T) -> Self {
            Self {
                schema_version: 1,
                status: "ok",
                data,
                errors: vec![],
            }
        }
        pub fn error(data: T, errors: Vec<String>) -> Self {
            Self {
                schema_version: 1,
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
    use crate::Manager;
    use std::path::Path;

    pub fn classify_manager(path: &str) -> Manager {
        if path.contains("/mise/") {
            Manager::Mise
        } else if path.contains("/homebrew/") {
            Manager::Homebrew
        } else if path.contains("/.cargo/") {
            Manager::Rustup
        } else {
            Manager::Unknown
        }
    }

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

    pub fn manager_for_executable(path: &Path) -> Manager {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        classify_manager(&resolved.to_string_lossy())
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
            .filter(|report| report.status == ToolStatus::Outdated && report.action.is_some())
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
            ToolStatus::Missing | ToolStatus::Unavailable => Style::new().yellow(),
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
        progress: Mutex<Option<ProgressBar>>,
    }

    impl Ui {
        pub fn new(json: bool, verbose: bool, no_color: bool) -> Self {
            let is_tty = std::io::stderr().is_terminal();
            let colors = is_tty && !no_color && std::env::var_os("NO_COLOR").is_none();
            Self {
                mode: FeedbackMode::select(is_tty, json, verbose),
                colors,
                progress: Mutex::new(None),
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
                    let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
                    if let Ok(style) = ProgressStyle::with_template(spinner_template(self.colors)) {
                        bar.set_style(
                            style.tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
                        );
                    }
                    bar.set_message(label.to_string());
                    bar.enable_steady_tick(Duration::from_millis(80));
                    *self.progress.lock().expect("progress mutex poisoned") = Some(bar);
                }
                FeedbackMode::Plain => eprintln!("… {label}"),
                FeedbackMode::Silent | FeedbackMode::Verbose => {}
            }
        }

        pub(crate) fn finish_progress(&self) {
            if let Some(bar) = self
                .progress
                .lock()
                .expect("progress mutex poisoned")
                .take()
            {
                bar.finish_and_clear();
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
pub enum Manager {
    Homebrew,
    Rustup,
    Mise,
    Npm,
    Unknown,
}

impl std::fmt::Display for Manager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("{self:?}").to_lowercase())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Current,
    Outdated,
    Missing,
    Unavailable,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReport {
    pub id: String,
    pub name: String,
    pub current: Option<String>,
    pub latest: Option<String>,
    pub status: ToolStatus,
    pub manager: Manager,
    pub executable: Option<String>,
    pub other_sources: Vec<String>,
    pub detail: Option<String>,
    pub action: Option<command::CommandSpec>,
}

pub mod config {
    use anyhow::{Context, Result};
    use directories::ProjectDirs;
    use serde::{Deserialize, Serialize};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(default)]
    pub struct Config {
        pub enabled_tools: Vec<String>,
        pub history_limit: usize,
        pub command_timeout_seconds: u64,
        pub preferred_install_manager: String,
    }

    impl Default for Config {
        fn default() -> Self {
            Self {
                enabled_tools: vec![
                    "homebrew".into(),
                    "rust".into(),
                    "node".into(),
                    "npm".into(),
                    "pnpm".into(),
                    "go".into(),
                ],
                history_limit: 500,
                command_timeout_seconds: 120,
                preferred_install_manager: "homebrew".into(),
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
        toml::from_str(&fs::read_to_string(path)?).context("invalid Beacon config")
    }
    pub fn load() -> Result<Config> {
        load_from(&path()?)
    }
    pub fn save_to(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(config)?)?;
        Ok(())
    }
    pub fn ensure() -> Result<(Config, PathBuf)> {
        let path = path()?;
        let config = load_from(&path)?;
        if !path.exists() {
            save_to(&config, &path)?;
        }
        Ok((config, path))
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
    use crate::{Manager, ToolReport};
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
        pub manager: String,
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
            let store = Self {
                connection: Connection::open(path)?,
            };
            store.migrate()?;
            Ok(store)
        }
        fn migrate(&self) -> Result<()> {
            self.connection.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, operation TEXT NOT NULL, tool TEXT NOT NULL, old_version TEXT, new_version TEXT, manager TEXT NOT NULL, status TEXT NOT NULL, summary TEXT NOT NULL); CREATE TABLE IF NOT EXISTS snapshots (id INTEGER PRIMARY KEY, created_at TEXT NOT NULL, payload TEXT NOT NULL);")?;
            Ok(())
        }
        #[allow(clippy::too_many_arguments)]
        pub fn record(
            &self,
            operation: &str,
            tool: &str,
            old: Option<&str>,
            new: Option<&str>,
            manager: Manager,
            status: &str,
            summary: &str,
        ) -> Result<()> {
            self.connection.execute("INSERT INTO history(created_at,operation,tool,old_version,new_version,manager,status,summary) VALUES(?,?,?,?,?,?,?,?)", params![Utc::now().to_rfc3339(), operation, tool, old, new, manager.to_string(), status, summary])?;
            Ok(())
        }
        pub fn snapshot(&self, reports: &[ToolReport]) -> Result<()> {
            self.connection.execute(
                "INSERT INTO snapshots(created_at,payload) VALUES(?,?)",
                params![Utc::now().to_rfc3339(), serde_json::to_string(reports)?],
            )?;
            Ok(())
        }
        pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
            let mut statement = self.connection.prepare("SELECT id,created_at,operation,tool,old_version,new_version,manager,status,summary FROM history ORDER BY id DESC LIMIT ?")?;
            Ok(statement
                .query_map([limit as i64], |row| {
                    Ok(HistoryEntry {
                        id: row.get(0)?,
                        created_at: row.get(1)?,
                        operation: row.get(2)?,
                        tool: row.get(3)?,
                        old_version: row.get(4)?,
                        new_version: row.get(5)?,
                        manager: row.get(6)?,
                        status: row.get(7)?,
                        summary: row.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?)
        }
        pub fn prune(&self, limit: usize) -> Result<()> {
            self.connection.execute("DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY id DESC LIMIT ?)", [limit as i64])?;
            Ok(())
        }
    }
}

pub mod providers;

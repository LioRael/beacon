use anyhow::{Context, Result, bail};
use beacon::{
    CheckData, InventoryReport, ToolReport, ToolStatus,
    config::{self, Config},
    envelope::{Envelope, ErrorDetail},
    providers,
    redact::redact,
    store::Store,
    ui::{Ui, status_text},
};
use clap::{Args, Parser, Subcommand};
use console::Style;
use dialoguer::{Confirm, MultiSelect};
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::{IsTerminal, Write},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    name = "beacon",
    version,
    about = "A safe development toolchain update manager"
)]
struct Cli {
    #[arg(long, global = true)]
    no_color: bool,
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Check(OutputArgs),
    Upgrade(UpgradeArgs),
    Doctor(TargetArgs),
    History(HistoryArgs),
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

impl Commands {
    fn json(&self) -> bool {
        match self {
            Self::Check(args) => args.json,
            Self::Upgrade(args) => args.json,
            Self::Doctor(args) => args.json,
            Self::History(args) => args.json,
            Self::Config { command } => command.json(),
        }
    }
}

#[derive(Args)]
struct OutputArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct TargetArgs {
    targets: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct UpgradeArgs {
    targets: Vec<String>,
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct HistoryArgs {
    #[arg(long, default_value_t = 20)]
    limit: usize,
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum ConfigCommand {
    Show {
        #[arg(long)]
        json: bool,
    },
    Path,
    Set {
        key: String,
        value: String,
    },
    Tools(ConfigToolsArgs),
    Inventories(ConfigInventoriesArgs),
}

impl ConfigCommand {
    fn json(&self) -> bool {
        match self {
            Self::Show { json } => *json,
            Self::Tools(args) => args.json,
            Self::Inventories(args) => args.json,
            _ => false,
        }
    }
}

#[derive(Args)]
struct ConfigToolsArgs {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<ToolConfigCommand>,
}

#[derive(Subcommand)]
enum ToolConfigCommand {
    Edit,
    Enable { tools: Vec<String> },
    Disable { tools: Vec<String> },
    Sync,
    Reset,
}

#[derive(Args)]
struct ConfigInventoriesArgs {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<InventoryConfigCommand>,
}

#[derive(Subcommand)]
enum InventoryConfigCommand {
    Edit,
    Enable { inventories: Vec<String> },
    Disable { inventories: Vec<String> },
    Reset,
}

#[derive(Debug, Clone)]
struct PlannedUpgrade {
    id: String,
    name: String,
    current: Option<String>,
    source: String,
    updater: String,
    action: providers::UpgradeAction,
    tool: Option<ToolReport>,
}

#[derive(Serialize)]
struct UpgradeResult {
    tool: String,
    old_version: Option<String>,
    new_version: Option<String>,
    installation_source: String,
    update_manager: String,
    status: String,
    action: providers::UpgradeAction,
}

#[derive(Serialize)]
struct ConfigChange {
    config_path: String,
    before: Vec<String>,
    after: Vec<String>,
    disabled: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>,
    unchanged: Vec<String>,
    skipped: Vec<ConfigSkipped>,
}

#[derive(Serialize)]
struct ConfigSkipped {
    id: String,
    reason: String,
}

#[derive(Serialize)]
struct ConfigItemStatus {
    id: String,
    name: String,
    enabled: bool,
    installed: bool,
    detail: Option<String>,
}

#[derive(Default)]
struct UpgradeBatch {
    results: Vec<UpgradeResult>,
    errors: Vec<ErrorDetail>,
}

impl UpgradeBatch {
    fn exit_code(&self) -> i32 {
        upgrade_exit_code(self.results.len(), self.errors.len())
    }
}

fn upgrade_exit_code(result_count: usize, error_count: usize) -> i32 {
    match (result_count == 0, error_count == 0) {
        (_, true) => 0,
        (true, false) => 1,
        (false, false) => 2,
    }
}

fn paths() -> Result<(PathBuf, PathBuf)> {
    let app = config::app_dir()?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok((
        app.join("beacon.db"),
        PathBuf::from(home).join("Library/Logs/Beacon/beacon.log"),
    ))
}

fn print_envelope<T: Serialize>(envelope: &Envelope<T>) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(envelope)?);
    Ok(())
}

fn report_errors(data: &CheckData) -> Vec<ErrorDetail> {
    data.tools
        .iter()
        .filter(|report| report.status == ToolStatus::Failed)
        .map(|report| {
            ErrorDetail::new(
                "tool_failed",
                Some(format!("tool:{}", report.id)),
                report.detail.as_deref().unwrap_or("tool check failed"),
            )
        })
        .chain(
            data.inventories
                .iter()
                .filter(|report| report.status == ToolStatus::Failed)
                .map(|report| {
                    ErrorDetail::new(
                        "inventory_failed",
                        Some(format!("inventory:{}", report.id)),
                        report.detail.as_deref().unwrap_or("inventory check failed"),
                    )
                }),
        )
        .collect()
}

fn print_reports(data: &CheckData, ui: &Ui) {
    println!(
        "{} {} {} {} {}",
        ui.paint(format!("{:<18}", "TOOL"), Style::new().cyan().bold()),
        ui.paint(format!("{:<11}", "STATUS"), Style::new().cyan().bold()),
        ui.paint(format!("{:<14}", "CURRENT"), Style::new().cyan().bold()),
        ui.paint(format!("{:<14}", "LATEST"), Style::new().cyan().bold()),
        ui.paint("SOURCE → UPDATER", Style::new().cyan().bold()),
    );
    for item in &data.tools {
        let current = item
            .installation
            .as_ref()
            .map(|value| value.current.display())
            .unwrap_or("—");
        let latest = item
            .update
            .as_ref()
            .map(|value| value.latest.display())
            .unwrap_or("—");
        let source = item
            .installation
            .as_ref()
            .and_then(|value| value.source.as_ref())
            .map(ToString::to_string)
            .unwrap_or_else(|| "unknown".into());
        let updater = item
            .update
            .as_ref()
            .map(|value| value.manager.to_string())
            .unwrap_or_else(|| "unmanaged".into());
        println!(
            "{} {} {:<14} {:<14} {} → {}",
            ui.paint(format!("{:<18}", item.name), Style::new().cyan()),
            status_text(item.status, ui.colors(), 11),
            current,
            latest,
            source,
            updater,
        );
        if let Some(installation) = &item.installation {
            println!(
                "  {}",
                ui.paint(
                    format!("path: {}", installation.executable),
                    Style::new().dim()
                )
            );
            for alternative in &installation.alternatives {
                println!(
                    "  {}",
                    ui.paint(
                        format!(
                            "also detected: {} {} ({})",
                            alternative.source,
                            alternative
                                .versions
                                .iter()
                                .map(|version| version.display())
                                .collect::<Vec<_>>()
                                .join(", "),
                            alternative.paths.join(", ")
                        ),
                        Style::new().dim()
                    )
                );
            }
        }
        if let Some(detail) = &item.detail {
            println!("  {detail}");
        }
        for conflict in &item.diagnostics.conflicts {
            println!(
                "  conflict: {} {} ({:?}): {}",
                conflict.claim, conflict.id, conflict.confidence, conflict.evidence
            );
        }
    }
    if !data.inventories.is_empty() {
        println!("\nHOMEBREW INVENTORY");
        for item in &data.inventories {
            println!(
                "{:<32} {:<11} {}",
                item.id,
                format!("{:?}", item.status).to_lowercase(),
                item.latest
                    .as_ref()
                    .map(|version| version.display())
                    .unwrap_or("—")
            );
        }
    }
}

fn tool_plan(report: &ToolReport) -> Option<PlannedUpgrade> {
    let installation = report.installation.as_ref()?;
    let update = report.update.as_ref()?;
    (report.status == ToolStatus::Outdated).then(|| PlannedUpgrade {
        id: report.id.clone(),
        name: report.name.clone(),
        current: Some(installation.current.display().to_string()),
        source: installation
            .source
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "unknown".into()),
        updater: update.manager.to_string(),
        action: update.action.clone(),
        tool: Some(report.clone()),
    })
}

fn inventory_plan(report: &InventoryReport) -> Option<PlannedUpgrade> {
    let action = report.action.as_ref()?;
    (report.status == ToolStatus::Outdated).then(|| PlannedUpgrade {
        id: report.id.clone(),
        name: report.name.clone(),
        current: report
            .current
            .as_ref()
            .map(|version| version.display().to_string()),
        source: "homebrew".into(),
        updater: "homebrew".into(),
        action: action.clone(),
        tool: None,
    })
}

fn all_plans(data: &CheckData) -> Vec<PlannedUpgrade> {
    data.tools
        .iter()
        .filter_map(tool_plan)
        .chain(data.inventories.iter().filter_map(inventory_plan))
        .collect()
}

fn matches_target(plan: &PlannedUpgrade, target: &str, data: &CheckData) -> Result<bool> {
    if plan.id == target || plan.name == target {
        return Ok(true);
    }
    if let Some(name) = target
        .strip_prefix("brew:")
        .filter(|_| !target.starts_with("brew:formula:") && !target.starts_with("brew:cask:"))
    {
        let matches = data
            .inventories
            .iter()
            .filter(|item| item.name == name)
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            bail!(
                "legacy target `{target}` is ambiguous; use brew:formula:{name} or brew:cask:{name}"
            );
        }
        return Ok(matches.first().is_some_and(|item| item.id == plan.id));
    }
    Ok(false)
}

fn select_targets(data: &CheckData, args: &UpgradeArgs, ui: &Ui) -> Result<Vec<PlannedUpgrade>> {
    if (args.json || !std::io::stdin().is_terminal()) && !args.yes {
        bail!("non-interactive upgrade requires explicit targets and --yes");
    }
    let actionable = all_plans(data);
    if !args.targets.is_empty() {
        let mut selected = Vec::new();
        for target in &args.targets {
            let mut matches = Vec::new();
            for plan in &actionable {
                if matches_target(plan, target, data)? {
                    matches.push(plan.clone());
                }
            }
            if matches.is_empty() {
                if data.tools.iter().any(|report| {
                    (report.id == *target || report.name == *target)
                        && report.status == ToolStatus::Missing
                }) {
                    bail!(
                        "target `{target}` is not installed; upgrade does not install missing tools"
                    );
                }
                bail!("target `{target}` is not actionable or was not found");
            }
            selected.extend(matches);
        }
        selected.sort_by(|left, right| left.id.cmp(&right.id));
        selected.dedup_by(|left, right| left.id == right.id);
        return Ok(selected);
    }
    if args.yes || args.json || !std::io::stdin().is_terminal() {
        bail!("non-interactive upgrade requires explicit targets and --yes");
    }
    let labels = actionable
        .iter()
        .map(|plan| {
            format!(
                "{}: {} → {} ({}; {})",
                ui.paint(&plan.id, Style::new().cyan()),
                plan.current.as_deref().unwrap_or("unknown"),
                ui.paint(
                    plan.action.expected_version.display(),
                    Style::new().yellow()
                ),
                ui.paint(plan.action.command.display(), Style::new().dim()),
                plan.action.target_mode
            )
        })
        .collect::<Vec<_>>();
    let chosen = MultiSelect::new()
        .with_prompt("Select updates")
        .items(&labels)
        .interact()?;
    Ok(chosen
        .into_iter()
        .map(|index| actionable[index].clone())
        .collect())
}

fn append_log(path: &PathBuf, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    writeln!(
        OpenOptions::new().create(true).append(true).open(path)?,
        "{line}"
    )?;
    Ok(())
}

fn set_config(config: &mut Config, key: &str, value: &str) -> Result<()> {
    match key {
        "history_limit" => {
            config.history_limit = value
                .parse()
                .context("history_limit must be a positive integer")?
        }
        "command_timeout_seconds" => {
            config.command_timeout_seconds = value
                .parse()
                .context("command_timeout_seconds must be a positive integer")?
        }
        "enabled_tools" | "disabled_tools" => {
            bail!("tool lists are managed with `beacon config tools`")
        }
        "enabled_inventories" | "disabled_inventories" => {
            bail!("inventory lists are managed with `beacon config inventories`")
        }
        "schema_version" => bail!("schema_version cannot be changed manually"),
        _ => bail!("unknown config key `{key}`"),
    }
    if config.history_limit == 0 || config.command_timeout_seconds == 0 {
        bail!("numeric settings must be greater than zero");
    }
    Ok(())
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_byte) in left.bytes().enumerate() {
        let mut current = vec![left_index + 1];
        for (right_index, right_byte) in right.bytes().enumerate() {
            current.push(std::cmp::min(
                std::cmp::min(current[right_index] + 1, previous[right_index + 1] + 1),
                previous[right_index] + usize::from(left_byte != right_byte),
            ));
        }
        previous = current;
    }
    previous[right.len()]
}

fn normalize_ids(values: &[String], supported: &[String], kind: &str) -> Result<Vec<String>> {
    if values.is_empty() {
        bail!("at least one {kind} is required");
    }
    let mut normalized = Vec::new();
    for value in values {
        let candidate = value.to_ascii_lowercase();
        if !supported.contains(&candidate) {
            let suggestion = supported
                .iter()
                .min_by_key(|supported| edit_distance(&candidate, supported));
            let hint = suggestion
                .filter(|suggestion| edit_distance(&candidate, suggestion) <= 2)
                .map(|suggestion| format!("; did you mean `{suggestion}`?"))
                .unwrap_or_default();
            bail!(
                "unknown {kind} `{value}`{hint} Supported {kind}s: {}",
                supported.join(", ")
            );
        }
        if !normalized.contains(&candidate) {
            normalized.push(candidate);
        }
    }
    Ok(normalized)
}

fn ordered(values: impl IntoIterator<Item = String>, supported: &[String]) -> Vec<String> {
    let values = values.into_iter().collect::<Vec<_>>();
    supported
        .iter()
        .filter(|id| values.contains(id))
        .cloned()
        .collect()
}

fn config_change(
    path: &std::path::Path,
    before: &[String],
    enabled: &[String],
    disabled: &[String],
    touched: &[String],
    skipped: Vec<ConfigSkipped>,
) -> ConfigChange {
    ConfigChange {
        config_path: path.display().to_string(),
        before: before.to_vec(),
        after: enabled.to_vec(),
        disabled: disabled.to_vec(),
        added: enabled
            .iter()
            .filter(|id| !before.contains(id))
            .cloned()
            .collect(),
        removed: before
            .iter()
            .filter(|id| !enabled.contains(id))
            .cloned()
            .collect(),
        unchanged: touched
            .iter()
            .filter(|id| before.contains(id) == enabled.contains(id))
            .cloned()
            .collect(),
        skipped,
    }
}

fn print_config_change(change: &ConfigChange, json: bool) -> Result<()> {
    if json {
        print_envelope(&Envelope::ok(change))?;
    } else {
        if !change.added.is_empty() {
            println!("Enabled: {}", change.added.join(", "));
        }
        if !change.removed.is_empty() {
            println!("Disabled: {}", change.removed.join(", "));
        }
        if change.added.is_empty() && change.removed.is_empty() {
            println!("No configuration changes.");
        }
        for skipped in &change.skipped {
            println!("Skipped {}: {}", skipped.id, skipped.reason);
        }
        println!("Config: {}", change.config_path);
    }
    Ok(())
}

fn assert_preflight(planned: &PlannedUpgrade, current: &ToolReport) -> Result<()> {
    let original = planned
        .tool
        .as_ref()
        .context("tool preflight has no original report")?;
    let old_installation = original
        .installation
        .as_ref()
        .context("original installation missing")?;
    let new_installation = current
        .installation
        .as_ref()
        .context("active installation disappeared")?;
    let old_update = original
        .update
        .as_ref()
        .context("original updater missing")?;
    let new_update = current
        .update
        .as_ref()
        .context("active updater disappeared")?;
    if old_installation.executable != new_installation.executable
        || old_installation.source != new_installation.source
        || old_installation.current != new_installation.current
        || old_update.manager != new_update.manager
    {
        bail!("PATH, source, updater, or version drifted after confirmation");
    }
    Ok(())
}

async fn verify_inventory(plan: &PlannedUpgrade, ui: &Ui, timeout_seconds: u64) -> Result<String> {
    let output = ui
        .run_command(
            &format!("Verifying {}", plan.name),
            &beacon::command::CommandSpec::new("brew", ["info", "--json=v2", plan.name.as_str()]),
            timeout_seconds,
        )
        .await?;
    let value: serde_json::Value = serde_json::from_str(&output.stdout)?;
    let actual = if plan.id.starts_with("brew:formula:") {
        value["formulae"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item["installed"].as_array())
            .and_then(|items| items.last())
            .and_then(|item| item["version"].as_str())
    } else {
        value["casks"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item["installed"].as_array())
            .and_then(|items| items.last())
            .and_then(|item| item.as_str())
    }
    .context("Homebrew verification response had no installed version")?;
    let expected = plan.action.expected_version.display();
    let changed = plan.current.as_deref() != Some(actual);
    let meets_expected = match (
        semver::Version::parse(actual),
        semver::Version::parse(expected),
    ) {
        (Ok(actual), Ok(expected)) => actual >= expected,
        _ => actual == expected,
    };
    if !changed || !meets_expected {
        bail!("Homebrew verification expected a newer version at least {expected}, got {actual}");
    }
    Ok(actual.into())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = cli.command.json();
    match run(cli).await {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(error) => {
            let home = std::env::var("HOME").ok();
            let message = redact(&format!("{error:#}"), home.as_deref());
            if json {
                let envelope = Envelope::error(
                    serde_json::Value::Null,
                    vec![ErrorDetail::new("fatal_error", None::<String>, message)],
                );
                if print_envelope(&envelope).is_err() {
                    eprintln!("error: failed to serialize fatal error");
                }
            } else {
                eprintln!("error: {message}");
            }
            std::process::exit(1);
        }
    }
}

async fn run(cli: Cli) -> Result<i32> {
    let config_was_missing = !config::path()?.exists();
    let human_output = !cli.command.json();
    let (mut config, config_path) = config::ensure()?;
    if config.tool_catalog_version < config::TOOL_CATALOG_VERSION {
        let ui = Ui::new(true, cli.verbose, cli.no_color);
        let pending_tools = providers::tool_registry()
            .iter()
            .map(|adapter| adapter.id().to_string())
            .filter(|id| {
                config.tool_catalog_version == 0
                    || config::tool_catalog_entry_version(id) > config.tool_catalog_version
            })
            .collect::<Vec<_>>();
        let probes = providers::probe_tools_for(&config, &ui, &pending_tools).await;
        let available_tools = probes
            .iter()
            .filter(|probe| probe.available)
            .map(|probe| probe.id.clone())
            .collect::<Vec<_>>();
        let available_inventories = if config.tool_catalog_version == 0 {
            providers::available_inventories(&config, &ui).await
        } else {
            Vec::new()
        };
        if config::initialize_catalog(&mut config, &available_tools, &available_inventories) {
            config::save_to(&config, &config_path)?;
            if config_was_missing && human_output {
                eprintln!(
                    "Initialized Beacon config at {}. Run `beacon config tools edit` to change monitored tools.",
                    config_path.display()
                );
            }
        }
    }
    let (db_path, log_path) = paths()?;
    let store = Store::open(&db_path)?;
    match cli.command {
        Commands::Check(args) => {
            let ui = Ui::new(args.json, cli.verbose, cli.no_color);
            let data = providers::check_all(&config, true, &ui).await?;
            let errors = report_errors(&data);
            let partial = !errors.is_empty();
            store.snapshot(&data)?;
            store.record(
                "check",
                "all",
                None,
                None,
                "unknown",
                "unknown",
                if errors.is_empty() {
                    "success"
                } else {
                    "partial"
                },
                &format!(
                    "{} tools, {} inventories",
                    data.tools.len(),
                    data.inventories.len()
                ),
            )?;
            store.prune(config.history_limit)?;
            if args.json {
                if errors.is_empty() {
                    print_envelope(&Envelope::ok(data))?;
                } else {
                    print_envelope(&Envelope::partial(data, errors))?;
                }
            } else {
                print_reports(&data, &ui);
            }
            return Ok(if partial { 2 } else { 0 });
        }
        Commands::Doctor(args) => {
            let ui = Ui::new(args.json, cli.verbose, cli.no_color);
            let mut data = providers::check_all(&config, false, &ui).await?;
            if !args.targets.is_empty() {
                data.tools.retain(|report| {
                    args.targets
                        .iter()
                        .any(|target| target == &report.id || target == &report.name)
                });
            }
            let errors = report_errors(&data);
            let partial = !errors.is_empty();
            store.record(
                "doctor",
                "all",
                None,
                None,
                "unknown",
                "unknown",
                if errors.is_empty() {
                    "success"
                } else {
                    "partial"
                },
                &format!("{} results", data.tools.len()),
            )?;
            if args.json {
                if errors.is_empty() {
                    print_envelope(&Envelope::ok(data))?;
                } else {
                    print_envelope(&Envelope::partial(data, errors))?;
                }
            } else {
                print_reports(&data, &ui);
            }
            return Ok(if partial { 2 } else { 0 });
        }
        Commands::Upgrade(args) => {
            let ui = Ui::new(args.json, cli.verbose, cli.no_color);
            let data = providers::check_all(&config, true, &ui).await?;
            let selected = select_targets(&data, &args, &ui)?;
            let home = std::env::var("HOME").ok();
            let mut batch = UpgradeBatch::default();
            for (index, plan) in selected.iter().enumerate() {
                if !args.yes
                    && !Confirm::new()
                        .with_prompt(format!(
                            "Run `{}` → {} ({})?",
                            plan.action.command.display(),
                            plan.action.expected_version.display(),
                            plan.action.target_mode
                        ))
                        .default(false)
                        .interact()?
                {
                    continue;
                }
                let attempt: Result<(beacon::runner::CommandOutput, Option<String>)> = async {
                    let fresh = providers::check_all(&config, true, &ui).await?;
                    if plan.tool.is_some() {
                        let current = fresh
                            .tools
                            .iter()
                            .find(|report| report.id == plan.id)
                            .context("tool disappeared during preflight")?;
                        assert_preflight(plan, current)?;
                    } else {
                        let current = fresh
                            .inventories
                            .iter()
                            .find(|report| report.id == plan.id)
                            .context("inventory target disappeared during preflight")?;
                        if current.current.as_ref().map(|version| version.display())
                            != plan.current.as_deref()
                            || current.action.as_ref().map(|action| &action.manager)
                                != Some(&plan.action.manager)
                        {
                            bail!(
                                "inventory source, updater, or version drifted after confirmation"
                            );
                        }
                    }
                    let label =
                        format!("[{}/{}] Upgrading {}", index + 1, selected.len(), plan.name);
                    let output = ui
                        .run_command(&label, &plan.action.command, config.command_timeout_seconds)
                        .await?;
                    let new_version = if let Some(report) = &plan.tool {
                        let new = providers::verify(report, &config, &ui).await?;
                        let post = providers::check_all(&config, false, &ui).await?;
                        let post_report = post
                            .tools
                            .iter()
                            .find(|item| item.id == plan.id)
                            .context("tool disappeared after upgrade")?;
                        let old_installation = report.installation.as_ref().unwrap();
                        let post_installation = post_report
                            .installation
                            .as_ref()
                            .context("installation missing after upgrade")?;
                        let old_updater = report.update.as_ref().unwrap();
                        let post_updater = post_report
                            .update
                            .as_ref()
                            .context("updater missing after upgrade")?;
                        if old_installation.executable != post_installation.executable
                            || old_installation.source != post_installation.source
                            || old_updater.manager != post_updater.manager
                        {
                            bail!("source or updater changed after upgrading {}", plan.id);
                        }
                        new
                    } else {
                        Some(verify_inventory(plan, &ui, config.command_timeout_seconds).await?)
                    };
                    Ok((output, new_version))
                }
                .await;
                let (output, new_version) = match attempt {
                    Ok(result) => result,
                    Err(error) => {
                        let summary = redact(&error.to_string(), home.as_deref());
                        let recovery = plan
                            .tool
                            .as_ref()
                            .map(providers::recovery_hint)
                            .unwrap_or_else(|| "Run `brew doctor`.".into());
                        let detail = format!("upgrade failed: {summary}. {recovery}");
                        let target = if plan.tool.is_some() {
                            format!("tool:{}", plan.id)
                        } else {
                            format!("inventory:{}", plan.id)
                        };
                        store.record(
                            "upgrade",
                            &plan.id,
                            plan.current.as_deref(),
                            None,
                            &plan.source,
                            &plan.updater,
                            "failed",
                            &detail,
                        )?;
                        append_log(
                            &log_path,
                            &format!(
                                "{} {} failed: {}",
                                chrono::Utc::now().to_rfc3339(),
                                plan.id,
                                detail
                            ),
                        )?;
                        batch
                            .errors
                            .push(ErrorDetail::new("upgrade_failed", Some(target), detail));
                        break;
                    }
                };
                let summary = redact(
                    &format!("{} {}", output.stdout, output.stderr),
                    home.as_deref(),
                );
                store.record(
                    "upgrade",
                    &plan.id,
                    plan.current.as_deref(),
                    new_version.as_deref(),
                    &plan.source,
                    &plan.updater,
                    "success",
                    &summary,
                )?;
                append_log(
                    &log_path,
                    &format!(
                        "{} {} success {}",
                        chrono::Utc::now().to_rfc3339(),
                        plan.id,
                        summary
                    ),
                )?;
                batch.results.push(UpgradeResult {
                    tool: plan.id.clone(),
                    old_version: plan.current.clone(),
                    new_version,
                    installation_source: plan.source.clone(),
                    update_manager: plan.updater.clone(),
                    status: "success".into(),
                    action: plan.action.clone(),
                });
            }
            store.prune(config.history_limit)?;
            let exit_code = batch.exit_code();
            if args.json {
                match exit_code {
                    0 => print_envelope(&Envelope::ok(batch.results))?,
                    1 => print_envelope(&Envelope::error(batch.results, batch.errors))?,
                    2 => print_envelope(&Envelope::partial(batch.results, batch.errors))?,
                    _ => unreachable!(),
                }
            } else if batch.results.is_empty() && batch.errors.is_empty() {
                println!("No updates selected.");
            } else {
                for item in batch.results {
                    println!(
                        "{} {}: {} → {}",
                        ui.paint("✓", Style::new().green()),
                        ui.paint(&item.tool, Style::new().cyan()),
                        item.old_version.as_deref().unwrap_or("unknown"),
                        item.new_version.as_deref().unwrap_or("unknown")
                    );
                }
                if let Some(error) = batch.errors.first() {
                    eprintln!(
                        "error: {}: {}",
                        error.target.as_deref().unwrap_or("upgrade"),
                        error.message
                    );
                }
            }
            return Ok(exit_code);
        }
        Commands::History(args) => {
            let entries = store.history(args.limit)?;
            if args.json {
                print_envelope(&Envelope::ok(entries))?;
            } else {
                for entry in entries {
                    println!(
                        "{} {:<8} {:<18} {:<8} {}",
                        entry.created_at, entry.operation, entry.tool, entry.status, entry.summary
                    );
                }
            }
        }
        Commands::Config { command } => match command {
            ConfigCommand::Show { json } => {
                if json {
                    print_envelope(&Envelope::ok(config))?;
                } else {
                    print!("{}", toml::to_string_pretty(&config)?);
                }
            }
            ConfigCommand::Path => println!("{}", config_path.display()),
            ConfigCommand::Set { key, value } => {
                set_config(&mut config, &key, &value)?;
                config::save_to(&config, &config_path)?;
                println!("Updated {key}.");
            }
            ConfigCommand::Tools(args) => {
                let supported = providers::tool_registry()
                    .iter()
                    .map(|adapter| adapter.id().to_string())
                    .collect::<Vec<_>>();
                match args.command {
                    None => {
                        let ui = Ui::new(args.json, cli.verbose, cli.no_color);
                        let probes = providers::probe_tools(&config, &ui).await;
                        let items = probes
                            .into_iter()
                            .map(|probe| ConfigItemStatus {
                                enabled: config.enabled_tools.contains(&probe.id),
                                installed: probe.available,
                                id: probe.id,
                                name: probe.name,
                                detail: probe.detail,
                            })
                            .collect::<Vec<_>>();
                        if args.json {
                            print_envelope(&Envelope::ok(items))?;
                        } else {
                            println!("{:<10} {:<18} {:<10} INSTALLED", "ID", "TOOL", "ENABLED");
                            for item in items {
                                println!(
                                    "{:<10} {:<18} {:<10} {}",
                                    item.id,
                                    item.name,
                                    if item.enabled { "yes" } else { "no" },
                                    if item.installed { "yes" } else { "no" }
                                );
                            }
                        }
                    }
                    Some(ToolConfigCommand::Enable { tools }) => {
                        let tools = normalize_ids(&tools, &supported, "tool")?;
                        let before = config.enabled_tools.clone();
                        config.disabled_tools.retain(|id| !tools.contains(id));
                        config.enabled_tools = ordered(
                            config
                                .enabled_tools
                                .into_iter()
                                .chain(tools.iter().cloned()),
                            &supported,
                        );
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_tools,
                            &config.disabled_tools,
                            &tools,
                            Vec::new(),
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(ToolConfigCommand::Disable { tools }) => {
                        let tools = normalize_ids(&tools, &supported, "tool")?;
                        let before = config.enabled_tools.clone();
                        config.enabled_tools.retain(|id| !tools.contains(id));
                        config.disabled_tools = ordered(
                            config
                                .disabled_tools
                                .into_iter()
                                .chain(tools.iter().cloned()),
                            &supported,
                        );
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_tools,
                            &config.disabled_tools,
                            &tools,
                            Vec::new(),
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(ToolConfigCommand::Sync) => {
                        let ui = Ui::new(args.json, cli.verbose, cli.no_color);
                        let probes = providers::probe_tools(&config, &ui).await;
                        let before = config.enabled_tools.clone();
                        let mut candidates = config.enabled_tools.clone();
                        let mut skipped = Vec::new();
                        for probe in probes {
                            if !probe.available {
                                skipped.push(ConfigSkipped {
                                    id: probe.id,
                                    reason: probe.detail.unwrap_or_else(|| "not available".into()),
                                });
                            } else if config.disabled_tools.contains(&probe.id) {
                                skipped.push(ConfigSkipped {
                                    id: probe.id,
                                    reason: "explicitly disabled".into(),
                                });
                            } else {
                                candidates.push(probe.id);
                            }
                        }
                        config.enabled_tools = ordered(candidates, &supported);
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_tools,
                            &config.disabled_tools,
                            &supported,
                            skipped,
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(ToolConfigCommand::Reset) => {
                        let ui = Ui::new(args.json, cli.verbose, cli.no_color);
                        let probes = providers::probe_tools(&config, &ui).await;
                        let before = config.enabled_tools.clone();
                        config.enabled_tools = probes
                            .iter()
                            .filter(|probe| probe.available)
                            .map(|probe| probe.id.clone())
                            .collect();
                        config.disabled_tools.clear();
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_tools,
                            &config.disabled_tools,
                            &supported,
                            Vec::new(),
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(ToolConfigCommand::Edit) => {
                        if args.json
                            || !std::io::stdin().is_terminal()
                            || !std::io::stdout().is_terminal()
                        {
                            bail!("`beacon config tools edit` requires an interactive terminal");
                        }
                        let labels = providers::tool_registry()
                            .iter()
                            .map(|adapter| format!("{} ({})", adapter.display_name(), adapter.id()))
                            .collect::<Vec<_>>();
                        let defaults = supported
                            .iter()
                            .map(|id| config.enabled_tools.contains(id))
                            .collect::<Vec<_>>();
                        let selected = MultiSelect::new()
                            .with_prompt("Select tools to monitor")
                            .items(&labels)
                            .defaults(&defaults)
                            .interact()?;
                        let before = config.enabled_tools.clone();
                        config.enabled_tools = selected
                            .iter()
                            .map(|index| supported[*index].clone())
                            .collect();
                        config.disabled_tools = supported
                            .iter()
                            .filter(|id| !config.enabled_tools.contains(id))
                            .cloned()
                            .collect();
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_tools,
                            &config.disabled_tools,
                            &supported,
                            Vec::new(),
                        );
                        print_config_change(&change, false)?;
                    }
                }
            }
            ConfigCommand::Inventories(args) => {
                let supported = vec!["homebrew".to_string()];
                match args.command {
                    None => {
                        let ui = Ui::new(args.json, cli.verbose, cli.no_color);
                        let available = providers::available_inventories(&config, &ui).await;
                        let items = vec![ConfigItemStatus {
                            id: "homebrew".into(),
                            name: "Homebrew".into(),
                            enabled: config.enabled_inventories.iter().any(|id| id == "homebrew"),
                            installed: available.iter().any(|id| id == "homebrew"),
                            detail: None,
                        }];
                        if args.json {
                            print_envelope(&Envelope::ok(items))?;
                        } else {
                            println!(
                                "{:<12} {:<18} {:<10} INSTALLED",
                                "ID", "INVENTORY", "ENABLED"
                            );
                            for item in items {
                                println!(
                                    "{:<12} {:<18} {:<10} {}",
                                    item.id,
                                    item.name,
                                    if item.enabled { "yes" } else { "no" },
                                    if item.installed { "yes" } else { "no" }
                                );
                            }
                        }
                    }
                    Some(InventoryConfigCommand::Enable { inventories }) => {
                        let inventories = normalize_ids(&inventories, &supported, "inventory")?;
                        let before = config.enabled_inventories.clone();
                        config
                            .disabled_inventories
                            .retain(|id| !inventories.contains(id));
                        config.enabled_inventories = ordered(
                            config
                                .enabled_inventories
                                .into_iter()
                                .chain(inventories.iter().cloned()),
                            &supported,
                        );
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_inventories,
                            &config.disabled_inventories,
                            &inventories,
                            Vec::new(),
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(InventoryConfigCommand::Disable { inventories }) => {
                        let inventories = normalize_ids(&inventories, &supported, "inventory")?;
                        let before = config.enabled_inventories.clone();
                        config
                            .enabled_inventories
                            .retain(|id| !inventories.contains(id));
                        config.disabled_inventories = ordered(
                            config
                                .disabled_inventories
                                .into_iter()
                                .chain(inventories.iter().cloned()),
                            &supported,
                        );
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_inventories,
                            &config.disabled_inventories,
                            &inventories,
                            Vec::new(),
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(InventoryConfigCommand::Reset) => {
                        let before = config.enabled_inventories.clone();
                        config.enabled_inventories = supported.clone();
                        config.disabled_inventories.clear();
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_inventories,
                            &config.disabled_inventories,
                            &supported,
                            Vec::new(),
                        );
                        print_config_change(&change, args.json)?;
                    }
                    Some(InventoryConfigCommand::Edit) => {
                        if args.json
                            || !std::io::stdin().is_terminal()
                            || !std::io::stdout().is_terminal()
                        {
                            bail!(
                                "`beacon config inventories edit` requires an interactive terminal"
                            );
                        }
                        let defaults =
                            vec![config.enabled_inventories.iter().any(|id| id == "homebrew")];
                        let selected = MultiSelect::new()
                            .with_prompt("Select inventories")
                            .items(&["Homebrew (homebrew)"])
                            .defaults(&defaults)
                            .interact()?;
                        let before = config.enabled_inventories.clone();
                        config.enabled_inventories =
                            selected.iter().map(|_| "homebrew".into()).collect();
                        config.disabled_inventories = if selected.is_empty() {
                            vec!["homebrew".into()]
                        } else {
                            Vec::new()
                        };
                        config::save_to(&config, &config_path)?;
                        let change = config_change(
                            &config_path,
                            &before,
                            &config.enabled_inventories,
                            &config.disabled_inventories,
                            &supported,
                            Vec::new(),
                        );
                        print_config_change(&change, false)?;
                    }
                }
            }
        },
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::upgrade_exit_code;

    #[test]
    fn upgrade_exit_codes_distinguish_success_fatal_and_partial_results() {
        assert_eq!(upgrade_exit_code(2, 0), 0);
        assert_eq!(upgrade_exit_code(0, 1), 1);
        assert_eq!(upgrade_exit_code(1, 1), 2);
    }
}

use anyhow::{Context, Result, bail};
use beacon::{
    ToolReport, ToolStatus,
    config::{self, Config},
    envelope::Envelope,
    providers,
    redact::redact,
    runner,
    store::Store,
};
use clap::{Args, Parser, Subcommand};
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
}

#[derive(Serialize)]
struct UpgradeResult {
    tool: String,
    old_version: Option<String>,
    new_version: Option<String>,
    status: String,
    command: String,
}

fn paths() -> Result<(PathBuf, PathBuf)> {
    let app = config::app_dir()?;
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok((
        app.join("beacon.db"),
        PathBuf::from(home).join("Library/Logs/Beacon/beacon.log"),
    ))
}

fn print_json<T: Serialize>(data: T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&Envelope::ok(data))?);
    Ok(())
}

fn print_reports(reports: &[ToolReport]) {
    if reports.is_empty() {
        println!("All managed tools are current.");
        return;
    }
    println!(
        "{:<24} {:<12} {:<14} {:<14} SOURCE",
        "TOOL", "STATUS", "CURRENT", "LATEST"
    );
    for item in reports {
        println!(
            "{:<24} {:<12} {:<14} {:<14} {}",
            item.name,
            format!("{:?}", item.status).to_lowercase(),
            item.current.as_deref().unwrap_or("—"),
            item.latest.as_deref().unwrap_or("—"),
            item.manager
        );
        if let Some(path) = &item.executable {
            println!("  path: {path}");
        }
        for source in &item.other_sources {
            println!("  also detected: {source}");
        }
        if let Some(detail) = &item.detail {
            println!("  {detail}");
        }
    }
}

fn select_targets(reports: &[ToolReport], args: &UpgradeArgs) -> Result<Vec<ToolReport>> {
    let actionable: Vec<_> = reports
        .iter()
        .filter(|r| {
            matches!(r.status, ToolStatus::Outdated | ToolStatus::Missing) && r.action.is_some()
        })
        .cloned()
        .collect();
    if !args.targets.is_empty() {
        let selected: Vec<_> = actionable
            .into_iter()
            .filter(|r| {
                args.targets
                    .iter()
                    .any(|target| target == &r.id || target == &r.name)
            })
            .collect();
        for target in &args.targets {
            if !selected
                .iter()
                .any(|r| &r.id == target || &r.name == target)
            {
                bail!("target `{target}` is not actionable or was not found");
            }
        }
        return Ok(selected);
    }
    if args.yes || args.json || !std::io::stdin().is_terminal() {
        bail!("non-interactive upgrade requires explicit targets and --yes");
    }
    if actionable.is_empty() {
        return Ok(vec![]);
    }
    let labels: Vec<_> = actionable
        .iter()
        .map(|r| {
            format!(
                "{}: {} → {} ({})",
                r.id,
                r.current.as_deref().unwrap_or("not installed"),
                r.latest.as_deref().unwrap_or("latest"),
                r.action.as_ref().unwrap().display()
            )
        })
        .collect();
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
        "preferred_install_manager" => {
            if !matches!(value, "homebrew" | "npm" | "mise") {
                bail!("preferred_install_manager must be homebrew, npm, or mise");
            }
            config.preferred_install_manager = value.into();
        }
        "enabled_tools" => {
            config.enabled_tools = value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        }
        _ => bail!("unknown config key `{key}`"),
    }
    if config.history_limit == 0 || config.command_timeout_seconds == 0 {
        bail!("numeric settings must be greater than zero");
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let (mut config, config_path) = config::ensure()?;
    let (db_path, log_path) = paths()?;
    let store = Store::open(&db_path)?;
    match cli.command {
        Commands::Check(args) => {
            let reports = providers::check_all(&config, true).await?;
            store.snapshot(&reports)?;
            store.record(
                "check",
                "all",
                None,
                None,
                beacon::Manager::Unknown,
                "success",
                &format!("{} results", reports.len()),
            )?;
            store.prune(config.history_limit)?;
            if args.json {
                print_json(reports)?;
            } else {
                print_reports(&reports);
            }
        }
        Commands::Doctor(args) => {
            let mut reports = providers::check_all(&config, false).await?;
            if !args.targets.is_empty() {
                reports.retain(|r| args.targets.iter().any(|t| t == &r.id || t == &r.name));
            }
            for report in &mut reports {
                if report.executable.is_none() && report.status != ToolStatus::Missing {
                    report.status = ToolStatus::Unavailable;
                }
            }
            store.record(
                "doctor",
                "all",
                None,
                None,
                beacon::Manager::Unknown,
                "success",
                &format!("{} results", reports.len()),
            )?;
            if args.json {
                print_json(reports)?;
            } else {
                print_reports(&reports);
            }
        }
        Commands::Upgrade(args) => {
            let reports = providers::check_all(&config, true).await?;
            let selected = select_targets(&reports, &args)?;
            let home = std::env::var("HOME").ok();
            let mut results = Vec::new();
            for report in selected {
                let command = report
                    .action
                    .as_ref()
                    .context("selected tool has no update action")?;
                if !args.yes
                    && !Confirm::new()
                        .with_prompt(format!("Run `{}`?", command.display()))
                        .default(false)
                        .interact()?
                {
                    continue;
                }
                let old = report.current.clone();
                match runner::run(command, config.command_timeout_seconds).await {
                    Ok(output) => match providers::verify(&report, &config).await {
                        Ok(new) => {
                            let summary = redact(
                                &format!("{} {}", output.stdout, output.stderr),
                                home.as_deref(),
                            );
                            store.record(
                                if old.is_some() { "upgrade" } else { "install" },
                                &report.id,
                                old.as_deref(),
                                new.as_deref(),
                                report.manager,
                                "success",
                                &summary,
                            )?;
                            append_log(
                                &log_path,
                                &format!(
                                    "{} {} success {}",
                                    chrono::Utc::now().to_rfc3339(),
                                    report.id,
                                    summary
                                ),
                            )?;
                            results.push(UpgradeResult {
                                tool: report.id,
                                old_version: old,
                                new_version: new,
                                status: "success".into(),
                                command: command.display(),
                            });
                        }
                        Err(error) => {
                            let summary = redact(&error.to_string(), home.as_deref());
                            store.record(
                                "verify",
                                &report.id,
                                old.as_deref(),
                                None,
                                report.manager,
                                "failed",
                                &summary,
                            )?;
                            append_log(
                                &log_path,
                                &format!(
                                    "{} {} verification failed: {}",
                                    chrono::Utc::now().to_rfc3339(),
                                    report.id,
                                    summary
                                ),
                            )?;
                            bail!(
                                "verification failed for {}. {}",
                                report.id,
                                providers::recovery_hint(&report)
                            );
                        }
                    },
                    Err(error) => {
                        let summary = redact(&error.to_string(), home.as_deref());
                        store.record(
                            "upgrade",
                            &report.id,
                            old.as_deref(),
                            None,
                            report.manager,
                            "failed",
                            &summary,
                        )?;
                        append_log(
                            &log_path,
                            &format!(
                                "{} {} failed: {}",
                                chrono::Utc::now().to_rfc3339(),
                                report.id,
                                summary
                            ),
                        )?;
                        bail!(
                            "upgrade failed for {}. {}",
                            report.id,
                            providers::recovery_hint(&report)
                        );
                    }
                }
            }
            store.prune(config.history_limit)?;
            if args.json {
                print_json(results)?;
            } else if results.is_empty() {
                println!("No updates selected.");
            } else {
                for item in results {
                    println!(
                        "✓ {}: {} → {}",
                        item.tool,
                        item.old_version.as_deref().unwrap_or("not installed"),
                        item.new_version.as_deref().unwrap_or("installed")
                    );
                }
            }
        }
        Commands::History(args) => {
            let entries = store.history(args.limit)?;
            if args.json {
                print_json(entries)?;
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
                    print_json(config)?;
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
        },
    }
    Ok(())
}

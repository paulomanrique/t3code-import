pub mod catalog;
pub mod model;
pub mod scan;
pub mod t3db;
#[cfg(feature = "gui")]
pub mod trash;

use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use rayon::prelude::*;

use model::{Provider, SessionSummary};
use scan::Roots;

#[derive(Parser)]
#[command(
    version,
    about = "Import Claude, Codex, Grok and OpenCode sessions into T3 Code"
)]
struct Cli {
    /// T3 Code home (defaults to $T3CODE_HOME, then ~/.t3).
    #[arg(long, global = true)]
    t3_home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List every session found.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Check that T3 is closed and its database is in a shape this build can write.
    Check,
    /// Import sessions given as <provider>:<session-id>.
    Import {
        sessions: Vec<String>,
        /// Keep only the newest N messages per thread (default: all).
        #[arg(long)]
        max_messages: Option<usize>,
        #[arg(long)]
        dry_run: bool,
        /// Close T3 first if it's running, and start its background service
        /// again when the import is done.
        #[arg(long)]
        close_t3: bool,
    },
}

pub fn run_cli(
    gui: Option<fn(Roots, Option<PathBuf>) -> anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let cli = Cli::parse();
    let roots = Roots::detect();
    match cli.command {
        None => match gui {
            Some(run) => run(roots, cli.t3_home),
            None => {
                bail!("Choose a command: list, check, or import")
            }
        },
        Some(Command::List { json }) => {
            let sessions = scan_all(&roots);
            if json {
                println!("{}", serde_json::to_string_pretty(&sessions)?);
            } else {
                for s in &sessions {
                    println!(
                        "{:<8} {}  {}  {}  {}",
                        s.provider.label(),
                        s.updated_at.format("%Y-%m-%d"),
                        s.session_id,
                        s.cwd.display(),
                        s.title
                    );
                }
            }
            Ok(())
        }
        Some(Command::Check) => {
            let home = t3db::T3Home::detect(cli.t3_home)?;
            let report = t3db::check(&home)?;
            print_guard(&home, &report);
            if !report.ok() {
                std::process::exit(2);
            }
            Ok(())
        }
        Some(Command::Import {
            sessions,
            max_messages,
            dry_run,
            close_t3,
        }) => {
            let home = t3db::T3Home::detect(cli.t3_home)?;
            let snapshot = t3db::snapshot(&home)?;
            let all = scan_all(&roots);
            let mut requests = Vec::new();
            for spec in &sessions {
                let (provider, id) = spec
                    .split_once(':')
                    .context("expected <provider>:<session-id>")?;
                let provider = parse_provider(provider)?;
                let summary = all
                    .iter()
                    .find(|s| s.provider == provider && s.session_id == id)
                    .with_context(|| format!("session not found: {spec}"))?;
                if snapshot.disabled_providers.contains(&provider) {
                    eprintln!(
                        "warning: {} is turned off in T3; {spec} imports, but can only be continued after enabling it",
                        provider.label()
                    );
                }
                requests.push(request_for(&roots, &snapshot, summary)?);
            }
            let options = t3db::ImportOptions {
                max_messages,
                dry_run,
            };
            // A dry run writes nothing, so it never needs T3 closed.
            let mut restart = if close_t3 && !dry_run {
                let running = t3db::control::find(&home)?;
                for r in &running {
                    eprintln!("Closing the {}: {}", r.name(), r.plan());
                }
                t3db::control::close(&home, &running)?
            } else {
                t3db::control::Restart::default()
            };
            let result = t3db::import(&home, requests, &options, &mut |p| eprintln!("{p:?}"));
            for line in restart.run() {
                match line {
                    Ok(done) => eprintln!("{done}"),
                    Err(failed) => eprintln!("warning: {failed}"),
                }
            }
            if let Err(error) = &result {
                if let Some(failure) = error.downcast_ref::<t3db::writer::ImportFailure>() {
                    for path in &failure.projects_created {
                        eprintln!("project created: {}", path.display());
                    }
                    if let Some(path) = &failure.backup {
                        eprintln!("backup: {}", path.display());
                    }
                }
            }
            print_report(&result?);
            Ok(())
        }
    }
}

pub fn scan_all(roots: &Roots) -> Vec<SessionSummary> {
    let mut all: Vec<SessionSummary> = scan::scanners(roots)
        .par_iter()
        .flat_map(|s| match s.scan() {
            Ok(found) => found,
            Err(e) => {
                eprintln!("{}: {e:#}", s.provider().label());
                Vec::new()
            }
        })
        .collect();
    all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    all
}

pub fn request_for(
    roots: &Roots,
    snapshot: &t3db::Snapshot,
    summary: &SessionSummary,
) -> anyhow::Result<t3db::ImportRequest> {
    let instance = snapshot
        .provider_instances
        .get(&summary.provider)
        .and_then(|v| v.first())
        .with_context(|| {
            format!(
                "no enabled {} provider in T3 settings",
                summary.provider.label()
            )
        })?
        .clone();
    let transcript = scan::scanner_for(roots, summary.provider).load(summary)?;
    Ok(t3db::ImportRequest {
        transcript,
        provider_instance_id: instance,
        placement: t3db::resolve_placement(&summary.cwd),
    })
}

fn parse_provider(s: &str) -> anyhow::Result<Provider> {
    Provider::ALL
        .into_iter()
        .find(|p| p.label().eq_ignore_ascii_case(s) || p.driver().eq_ignore_ascii_case(s))
        .map_or_else(|| bail!("unknown provider: {s}"), Ok)
}

pub fn print_guard(home: &t3db::T3Home, report: &t3db::GuardReport) {
    if !report.t3_running.is_empty() {
        match t3db::control::find(home) {
            Ok(running) if !running.is_empty() => {
                for r in &running {
                    println!("T3 is running: the {}", r.name());
                }
            }
            _ => report
                .t3_running
                .iter()
                .for_each(|p| println!("T3 is running: {p}")),
        }
        println!(
            "Close it, or let `t3-import import --close-t3 …` close it and start it again afterwards."
        );
    }
    for f in &report.findings {
        println!(
            "schema mismatch at {}: expected {}, found {}",
            f.location, f.expected, f.actual
        );
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
    if let Some(url) = &report.issue_url {
        println!("\nThis T3 Code version changed its database. Please report it:\n{url}");
    }
    if report.ok() {
        println!("ok");
    }
}

fn print_report(report: &t3db::ImportReport) {
    for p in &report.projects_created {
        println!("project created: {}", p.display());
    }
    for id in &report.imported {
        println!("imported: {id}");
    }
    for (id, why) in &report.skipped {
        println!("skipped: {id} ({why})");
    }
    for w in &report.warnings {
        println!("warning: {w}");
    }
    if let Some(b) = &report.backup {
        println!("backup: {}", b.display());
    }
}

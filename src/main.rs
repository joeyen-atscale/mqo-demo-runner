use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mqo_demo_runner::{
    flagship_scenario, probe_steps, render_markdown, run_scenario, Scenario, RunOptions,
};

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    /// Emit raw `transcript.json`
    Json,
    /// Render a human-readable Markdown demo script
    Md,
}

#[derive(Parser)]
#[command(
    name = "mqo-demo-runner",
    about = "Ordered orchestrator for the AtScale MQO five-pillar story",
    long_about = "Calls each pillar CLI in declared order, threads output forward, \
                  and emits a single transcript.json with one entry per pillar plus \
                  a final signed answer.\n\nUse --mock to run without any sibling binaries installed.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute an ordered pipeline of pillar steps from a scenario file.
    ///
    /// Each step is a subprocess invocation; its stdout JSON is captured,
    /// validated, and passed forward. Emits transcript.json plus optional
    /// Markdown rendering. A failing step short-circuits with blocked_at
    /// rather than fabricating downstream results.
    Run {
        /// Path to a scenario JSON file.
        ///
        /// Defaults to the bundled flagship scenario when omitted.
        #[arg(long, value_name = "SCENARIO")]
        scenario: Option<PathBuf>,

        /// Use canned mock responses instead of real subprocess calls.
        ///
        /// Runs the full pipeline without network or sibling binaries — safe for CI.
        #[arg(long)]
        mock: bool,

        /// Output format.
        #[arg(long, value_enum, default_value = "json")]
        format: OutputFormat,

        /// Write transcript to this file instead of stdout.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },

    /// List the canonical pillar order and which binary each step needs.
    ///
    /// Shows which binaries are available on PATH before a run.
    Steps {
        /// Scenario file to inspect.
        ///
        /// Defaults to the bundled flagship scenario.
        #[arg(long, value_name = "SCENARIO")]
        scenario: Option<PathBuf>,

        /// Emit JSON instead of human-readable table.
        #[arg(long)]
        json: bool,
    },

    /// Run as an MCP subprocess server over stdin/stdout.
    ///
    /// Reads newline-delimited JSON tool calls; supports the `run` tool:
    ///   {"tool": "run", "scenario_path": "<path>", "mock": false}
    Serve,
}

fn main() -> Result<()> {
    sigpipe::reset();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            scenario,
            mock,
            format,
            out,
        } => {
            let sc = load_scenario(scenario.as_deref())?;
            let opts = RunOptions { mock };
            let transcript = run_scenario(&sc, &opts);

            let content = match format {
                OutputFormat::Json => serde_json::to_string_pretty(&transcript)
                    .context("serialize transcript")?,
                OutputFormat::Md => render_markdown(&transcript),
            };

            match out {
                Some(path) => {
                    std::fs::write(&path, &content)
                        .with_context(|| format!("write transcript to {}", path.display()))?;
                    eprintln!("transcript written to {}", path.display());
                }
                None => println!("{}", content),
            }

            // Exit 1 if pipeline was blocked.
            if transcript.blocked_at.is_some() {
                std::process::exit(1);
            }
        }

        Commands::Steps { scenario, json } => {
            let sc = load_scenario(scenario.as_deref())?;
            let infos = probe_steps(&sc);

            if json {
                println!("{}", serde_json::to_string_pretty(&infos)?);
            } else {
                println!("Pillar steps for scenario: {}", sc.scenario);
                println!();
                for (i, info) in infos.iter().enumerate() {
                    let avail = if info.available { "✓" } else { "✗ MISSING" };
                    println!("  {:2}. {:30} {:25} [{}]", i + 1, info.pillar, info.tool, avail);
                }
                let missing = infos.iter().filter(|i| !i.available).count();
                if missing > 0 {
                    println!();
                    println!(
                        "  {} step(s) missing — use --mock to run without them",
                        missing
                    );
                }
            }
        }

        Commands::Serve => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            mqo_demo_runner::serve_mcp(&cwd)?;
        }
    }

    Ok(())
}

/// Load a `Scenario` from a path, or return the in-memory flagship if no path given.
fn load_scenario(path: Option<&Path>) -> Result<Scenario> {
    match path {
        Some(p) => Scenario::load(p),
        None => Ok(flagship_scenario()),
    }
}

#[cfg(test)]
mod tests {
    use mqo_demo_runner::{flagship_scenario, run_scenario, RunOptions};

    /// Integration-style: the steps subcommand logic (probe_steps) works on the flagship.
    #[test]
    fn steps_lists_all_flagship_pillars() {
        let scenario = flagship_scenario();
        let infos = mqo_demo_runner::probe_steps(&scenario);
        // All 8 pillars from the PRD must be present.
        let pillars: Vec<&str> = infos.iter().map(|i| i.pillar.as_str()).collect();
        assert!(
            pillars.contains(&"catalog-embed"),
            "missing catalog-embed pillar"
        );
        assert!(
            pillars.contains(&"binding-confidence"),
            "missing binding-confidence pillar"
        );
        assert!(pillars.contains(&"clarify"), "missing clarify pillar");
        assert!(
            pillars.contains(&"time-intelligence"),
            "missing time-intelligence pillar"
        );
        assert!(
            pillars.contains(&"engine-parity"),
            "missing engine-parity pillar"
        );
        assert!(
            pillars.contains(&"sensitivity-scan"),
            "missing sensitivity-scan pillar"
        );
        assert!(
            pillars.contains(&"ousia-grounding"),
            "missing ousia-grounding pillar"
        );
        assert!(
            pillars.contains(&"rosetta-credential"),
            "missing rosetta-credential pillar"
        );
    }

    /// Mock run emits transcript with 8 entries.
    #[test]
    fn mock_run_emits_8_entries() {
        let scenario = flagship_scenario();
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        assert_eq!(
            transcript.entries.len(),
            8,
            "flagship scenario must produce 8 transcript entries"
        );
    }
}

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use mqo_demo_runner::{
    default_demo_script, flagship_scenario, probe_steps, render_markdown, run_demo, run_scenario,
    DemoResult, Scenario, RunOptions,
};

/// Output format for the `run` subcommand (scenario-mode).
#[derive(Debug, Clone, ValueEnum)]
enum ScenarioFormat {
    /// Emit raw `transcript.json`
    Json,
    /// Render a human-readable Markdown demo script
    Md,
}

/// Output format for the simple `run --model` / `list` commands.
#[derive(Debug, Clone, ValueEnum)]
enum SimpleFormat {
    /// Human-readable text
    Text,
    /// Machine-readable JSON (`{"steps_run":…,"steps_passed":…,…}`)
    Json,
}

#[derive(Parser)]
#[command(
    name = "mqo-demo-runner",
    about = "Ordered orchestrator for the AtScale MQO ethical-AI toolchain",
    long_about = "Two usage modes:\n\
                  \n\
                  1. Simple demo (run / list):\n\
                     mqo-demo-runner run [--model <json>] [--dry-run] [--format text|json]\n\
                     mqo-demo-runner list\n\
                  \n\
                  2. Scenario pipeline (steps / serve):\n\
                     mqo-demo-runner steps [--scenario <file>]\n\
                     mqo-demo-runner serve\n\
                  \n\
                  Use --mock on `steps`/`run-scenario` to run without sibling binaries.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the five-step ethical-AI toolchain demo.
    ///
    /// By default runs the built-in demo against `--model`.  Use `--dry-run`
    /// to print the commands that would be executed without running them.
    /// Add `--scenario` to use the full eight-pillar JSON scenario pipeline
    /// instead of the simple five-step demo.
    Run {
        /// Model path / identifier passed to each demo step.
        ///
        /// Defaults to `"internet_sales"` when omitted.
        #[arg(long, value_name = "MODEL", default_value = "internet_sales")]
        model: String,

        /// Print the commands that would be run without executing them.
        #[arg(long)]
        dry_run: bool,

        /// Output format for the simple demo result.
        #[arg(long, value_enum, default_value = "text")]
        format: SimpleFormat,

        /// Path to a JSON scenario file (eight-pillar pipeline mode).
        ///
        /// When provided, all other flags are ignored and the scenario
        /// pipeline runs instead of the simple five-step demo.
        #[arg(long, value_name = "SCENARIO")]
        scenario: Option<PathBuf>,

        /// Use canned mock responses (scenario pipeline mode only).
        #[arg(long)]
        mock: bool,

        /// Scenario-mode output format.
        #[arg(long, value_enum, default_value = "json", value_name = "SCENARIO_FMT")]
        scenario_format: ScenarioFormat,

        /// Write output to this file instead of stdout.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },

    /// List the five default demo steps and their commands.
    ///
    /// No steps are executed; this is a dry-preview of what `run` would do.
    List {
        /// Model path / identifier (used to fill step commands).
        #[arg(long, value_name = "MODEL", default_value = "internet_sales")]
        model: String,
    },

    /// List the canonical pillar order and which binary each step needs.
    ///
    /// Shows which binaries are available on PATH before a scenario run.
    Steps {
        /// Scenario file to inspect (defaults to the bundled flagship).
        #[arg(long, value_name = "SCENARIO")]
        scenario: Option<PathBuf>,

        /// Emit JSON instead of a human-readable table.
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
            model,
            dry_run,
            format,
            scenario,
            mock,
            scenario_format,
            out,
        } => {
            if let Some(scenario_path) = scenario {
                // Eight-pillar scenario pipeline mode.
                let sc = load_scenario(Some(scenario_path.as_path()))?;
                let opts = RunOptions { mock };
                let transcript = run_scenario(&sc, &opts);

                let content = match scenario_format {
                    ScenarioFormat::Json => serde_json::to_string_pretty(&transcript)
                        .context("serialize transcript")?,
                    ScenarioFormat::Md => render_markdown(&transcript),
                };
                write_or_print(out.as_deref(), &content)?;

                if transcript.blocked_at.is_some() {
                    std::process::exit(1);
                }
            } else {
                // Simple five-step demo mode.
                let script = default_demo_script(&model);
                let result = run_demo(&script, dry_run);
                let content = format_demo_result(&result, &script.title, &format);
                write_or_print(out.as_deref(), &content)?;
            }
        }

        Commands::List { model } => {
            let script = default_demo_script(&model);
            println!("Demo: {}", script.title);
            println!();
            for (i, step) in script.steps.iter().enumerate() {
                println!("  {:2}. {} — {}", i + 1, step.name, step.description);
                println!("       {}", step.command.join(" "));
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
                    println!(
                        "  {:2}. {:30} {:25} [{}]",
                        i + 1,
                        info.pillar,
                        info.tool,
                        avail
                    );
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

fn format_demo_result(result: &DemoResult, title: &str, format: &SimpleFormat) -> String {
    match format {
        SimpleFormat::Json => {
            let entries: Vec<serde_json::Value> = result
                .results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "step": r.step,
                        "ok": r.ok,
                        "skipped": r.skipped,
                        "output": r.output,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&serde_json::json!({
                "title": title,
                "steps_run": result.steps_run,
                "steps_passed": result.steps_passed,
                "results": entries,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        }
        SimpleFormat::Text => {
            let mut out = String::new();
            out.push_str(&format!("Demo: {}\n\n", title));
            for r in &result.results {
                let status = if r.skipped {
                    "SKIP"
                } else if r.ok {
                    "OK"
                } else {
                    "FAIL"
                };
                out.push_str(&format!("  [{}] {}\n", status, r.step));
                if !r.output.trim().is_empty() {
                    out.push_str(&format!("       {}\n", r.output.trim()));
                }
            }
            out.push_str(&format!(
                "\n{}/{} steps passed\n",
                result.steps_passed, result.steps_run
            ));
            out
        }
    }
}

/// Load a `Scenario` from a path, or return the in-memory flagship if no path given.
fn load_scenario(path: Option<&Path>) -> Result<Scenario> {
    match path {
        Some(p) => Scenario::load(p),
        None => Ok(flagship_scenario()),
    }
}

fn write_or_print(path: Option<&Path>, content: &str) -> Result<()> {
    match path {
        Some(p) => {
            std::fs::write(p, content)
                .with_context(|| format!("write output to {}", p.display()))?;
            eprintln!("output written to {}", p.display());
        }
        None => println!("{}", content),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use mqo_demo_runner::{
        default_demo_script, flagship_scenario, probe_steps, run_demo, run_scenario, RunOptions,
    };

    /// Integration-style: the steps subcommand logic (probe_steps) works on the flagship.
    #[test]
    fn steps_lists_all_flagship_pillars() {
        let scenario = flagship_scenario();
        let infos = probe_steps(&scenario);
        let pillars: Vec<&str> = infos.iter().map(|i| i.pillar.as_str()).collect();
        assert!(pillars.contains(&"catalog-embed"), "missing catalog-embed");
        assert!(
            pillars.contains(&"binding-confidence"),
            "missing binding-confidence"
        );
        assert!(pillars.contains(&"clarify"), "missing clarify");
        assert!(
            pillars.contains(&"time-intelligence"),
            "missing time-intelligence"
        );
        assert!(pillars.contains(&"engine-parity"), "missing engine-parity");
        assert!(
            pillars.contains(&"sensitivity-scan"),
            "missing sensitivity-scan"
        );
        assert!(
            pillars.contains(&"ousia-grounding"),
            "missing ousia-grounding"
        );
        assert!(
            pillars.contains(&"rosetta-credential"),
            "missing rosetta-credential"
        );
    }

    /// Mock run emits transcript with 8 entries.
    #[test]
    fn mock_run_emits_8_entries() {
        let scenario = flagship_scenario();
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        assert_eq!(transcript.entries.len(), 8);
    }

    /// default_demo_script returns 5 steps.
    #[test]
    fn demo_script_has_5_steps() {
        let script = default_demo_script("test_model.json");
        assert_eq!(script.steps.len(), 5, "must return exactly 5 steps");
    }

    /// dry-run produces 5 results all with ok=true and output containing "[dry-run]".
    #[test]
    fn dry_run_all_ok_with_marker() {
        let script = default_demo_script("test_model.json");
        let result = run_demo(&script, true);
        assert_eq!(result.steps_run, 5);
        assert_eq!(result.steps_passed, 5);
        for r in &result.results {
            assert!(r.ok, "dry-run step {} must be ok", r.step);
            assert!(
                r.output.contains("[dry-run]"),
                "dry-run output for {} must contain [dry-run], got: {}",
                r.step,
                r.output
            );
        }
    }

    /// Missing binary → skipped=true, ok=true, output contains "SKIP".
    #[test]
    fn missing_binary_produces_skip() {
        use mqo_demo_runner::{DemoScript, DemoStep};
        let script = DemoScript {
            title: "test".into(),
            steps: vec![DemoStep {
                name: "Fake Step".into(),
                description: "Fake".into(),
                command: vec!["__nonexistent_xyzzy_binary_99999__".into(), "arg".into()],
            }],
        };
        let result = run_demo(&script, false);
        assert_eq!(result.steps_run, 1);
        let r = &result.results[0];
        assert!(r.skipped, "missing binary must be skipped");
        assert!(r.ok, "skipped step must be ok=true");
        assert!(
            r.output.contains("SKIP"),
            "output must contain SKIP, got: {}",
            r.output
        );
    }

    /// list exits 0 — tested by calling the listing logic (probe default script).
    #[test]
    fn list_logic_works() {
        let script = default_demo_script("model.json");
        // Just ensuring we can iterate the steps without panic.
        assert_eq!(script.steps.len(), 5);
        for step in &script.steps {
            assert!(!step.name.is_empty());
            assert!(!step.command.is_empty());
        }
    }

    /// json output has "steps_run" and "steps_passed".
    #[test]
    fn json_output_has_required_fields() {
        let script = default_demo_script("model.json");
        let result = run_demo(&script, true);
        // Simulate what format_demo_result does for JSON.
        let entries: Vec<serde_json::Value> = result
            .results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "step": r.step,
                    "ok": r.ok,
                    "skipped": r.skipped,
                    "output": r.output,
                })
            })
            .collect();
        let json_val = serde_json::json!({
            "title": script.title,
            "steps_run": result.steps_run,
            "steps_passed": result.steps_passed,
            "results": entries,
        });
        assert!(json_val.get("steps_run").is_some(), "missing steps_run");
        assert!(json_val.get("steps_passed").is_some(), "missing steps_passed");
    }
}

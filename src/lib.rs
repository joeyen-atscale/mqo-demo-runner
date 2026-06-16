//! mqo-demo-runner — ordered orchestrator for the AtScale MQO five-pillar story.
//!
//! Calls each pillar CLI in declared order, threads output forward, and emits
//! a single `transcript.json` with one entry per pillar plus a final signed answer.

use std::{
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ────────────────────────────────────── Scenario types ──────────────────────

/// One step in a scenario — a subprocess invocation description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioStep {
    /// Human-readable pillar name (e.g. `"catalog-embed"`).
    pub pillar: String,
    /// Binary to invoke (e.g. `"mqo-catalog-embed"`).
    pub tool: String,
    /// Arguments passed to the binary.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional canned response used in `--mock` mode.
    #[serde(default)]
    pub mock_response: Option<serde_json::Value>,
}

/// A scenario file (`--scenario <path>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub scenario: String,
    pub description: String,
    pub question: String,
    #[serde(default)]
    pub model: Option<String>,
    pub steps: Vec<ScenarioStep>,
}

impl Scenario {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read scenario file {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("invalid JSON in scenario file {}", path.display()))
    }
}

// ────────────────────────────────────── Transcript types ────────────────────

/// Verdict from one pillar step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Skipped,
    Blocked,
    Failed,
}

/// Per-step entry in the transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub pillar: String,
    pub tool: String,
    /// The exact subprocess invocation as a space-joined command line.
    pub invocation: String,
    pub verdict: Verdict,
    pub output: serde_json::Value,
    /// Wall-clock milliseconds for this step (excluded from determinism checks).
    pub ms: u64,
}

/// Full transcript produced by a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub scenario: String,
    pub question: String,
    pub entries: Vec<TranscriptEntry>,
    /// Which pillar caused a short-circuit, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_at: Option<String>,
    /// Final signed answer text, present only when all pillars succeeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_answer: Option<String>,
}

// ────────────────────────────────────── Step info (for `steps` subcommand) ──

/// Information about a single step (used by `steps` subcommand).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInfo {
    pub pillar: String,
    pub tool: String,
    pub available: bool,
}

/// Check which binaries in a scenario are available on PATH.
pub fn probe_steps(scenario: &Scenario) -> Vec<StepInfo> {
    scenario
        .steps
        .iter()
        .map(|s| StepInfo {
            pillar: s.pillar.clone(),
            tool: s.tool.clone(),
            available: binary_on_path(&s.tool),
        })
        .collect()
}

fn binary_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let full = dir.join(name);
                full.is_file()
            })
        })
        .unwrap_or(false)
}

// ────────────────────────────────────── Runner ──────────────────────────────

/// Options controlling how a scenario is executed.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Use canned `mock_response` instead of executing real subprocesses.
    pub mock: bool,
}

/// Execute all steps in `scenario` and return a `Transcript`.
///
/// - If `opts.mock` is true, each step reads `step.mock_response` instead of
///   spawning a subprocess.
/// - A step whose binary is not on PATH produces `Verdict::Blocked` and sets
///   `blocked_at`; downstream steps become `Verdict::Skipped`.
/// - A step that returns non-zero exit or invalid output produces
///   `Verdict::Blocked` and short-circuits the same way.
/// - The exact subprocess invocation is recorded in `invocation` for every step.
pub fn run_scenario(scenario: &Scenario, opts: &RunOptions) -> Transcript {
    let mut entries: Vec<TranscriptEntry> = Vec::new();
    let mut blocked_at: Option<String> = None;

    for step in &scenario.steps {
        let invocation = build_invocation(&step.tool, &step.args);

        if blocked_at.is_some() {
            // A previous step blocked — mark downstream as skipped.
            entries.push(TranscriptEntry {
                pillar: step.pillar.clone(),
                tool: step.tool.clone(),
                invocation,
                verdict: Verdict::Skipped,
                output: serde_json::Value::Null,
                ms: 0,
            });
            continue;
        }

        let t0 = Instant::now();
        let entry = if opts.mock {
            run_step_mock(step, invocation)
        } else {
            run_step_real(step, invocation)
        };
        let ms = t0.elapsed().as_millis() as u64;

        let is_blocked = entry.verdict == Verdict::Blocked || entry.verdict == Verdict::Failed;
        let pillar = entry.pillar.clone();
        entries.push(TranscriptEntry { ms, ..entry });

        if is_blocked {
            blocked_at = Some(pillar);
        }
    }

    let all_ok = blocked_at.is_none();
    let final_answer = if all_ok {
        Some(build_final_answer(scenario, &entries))
    } else {
        None
    };

    Transcript {
        scenario: scenario.scenario.clone(),
        question: scenario.question.clone(),
        entries,
        blocked_at,
        final_answer,
    }
}

fn build_invocation(tool: &str, args: &[String]) -> String {
    if args.is_empty() {
        tool.to_string()
    } else {
        format!("{} {}", tool, args.join(" "))
    }
}

fn run_step_mock(step: &ScenarioStep, invocation: String) -> TranscriptEntry {
    let output = step
        .mock_response
        .clone()
        .unwrap_or(serde_json::json!({"status": "ok", "mock": true}));

    let verdict = match output.get("status").and_then(|v| v.as_str()) {
        Some("ok") | None => Verdict::Ok,
        Some("blocked") => Verdict::Blocked,
        _ => Verdict::Failed,
    };

    TranscriptEntry {
        pillar: step.pillar.clone(),
        tool: step.tool.clone(),
        invocation,
        verdict,
        output,
        ms: 0,
    }
}

fn run_step_real(step: &ScenarioStep, invocation: String) -> TranscriptEntry {
    // Check binary availability first.
    if !binary_on_path(&step.tool) {
        return TranscriptEntry {
            pillar: step.pillar.clone(),
            tool: step.tool.clone(),
            invocation,
            verdict: Verdict::Blocked,
            output: serde_json::json!({
                "error": "binary not on PATH",
                "tool": step.tool,
            }),
            ms: 0,
        };
    }

    // Spawn the subprocess.
    let result = Command::new(&step.tool)
        .args(&step.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|child| child.wait_with_output());

    match result {
        Err(e) => TranscriptEntry {
            pillar: step.pillar.clone(),
            tool: step.tool.clone(),
            invocation,
            verdict: Verdict::Failed,
            output: serde_json::json!({"error": e.to_string()}),
            ms: 0,
        },
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let exit_code = out.status.code().unwrap_or(-1);

            // Try to parse stdout as JSON; fall back to a string envelope.
            let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(
                |_| serde_json::json!({"stdout": stdout.trim(), "stderr": stderr.trim()}),
            );

            let verdict = if exit_code == 0 {
                Verdict::Ok
            } else {
                Verdict::Blocked
            };

            TranscriptEntry {
                pillar: step.pillar.clone(),
                tool: step.tool.clone(),
                invocation,
                verdict,
                output: parsed,
                ms: 0,
            }
        }
    }
}

fn build_final_answer(scenario: &Scenario, _entries: &[TranscriptEntry]) -> String {
    format!(
        "All pillars fired in order for scenario '{}'. \
         Question: {} — pipeline completed; see transcript for per-pillar verdicts.",
        scenario.scenario, scenario.question
    )
}

// ────────────────────────────────────── Markdown rendering ──────────────────

/// Render `transcript` as a human-readable Markdown demo script.
pub fn render_markdown(transcript: &Transcript) -> String {
    let mut out = String::new();

    out.push_str(&format!("# Demo Transcript: {}\n\n", transcript.scenario));
    out.push_str(&format!("**Question:** {}\n\n", transcript.question));

    for (i, entry) in transcript.entries.iter().enumerate() {
        out.push_str(&format!(
            "## Step {}: {} ({})\n\n",
            i + 1,
            entry.pillar,
            entry.tool
        ));
        out.push_str(&format!("**Invocation:** `{}`\n\n", entry.invocation));
        out.push_str(&format!("**Verdict:** {:?}\n\n", entry.verdict));

        let output_str = serde_json::to_string_pretty(&entry.output)
            .unwrap_or_else(|_| "{}".to_string());
        out.push_str("**Output:**\n\n```json\n");
        out.push_str(&output_str);
        out.push_str("\n```\n\n");
    }

    if let Some(ref blocked) = transcript.blocked_at {
        out.push_str(&format!(
            "---\n\n**Pipeline blocked at pillar:** `{}`\n\n",
            blocked
        ));
        out.push_str("Downstream steps are marked `skipped`.\n");
    } else if let Some(ref answer) = transcript.final_answer {
        out.push_str("---\n\n**Final Answer:**\n\n");
        out.push_str(answer);
        out.push('\n');
    }

    out
}

// ────────────────────────────────────── Serve (MCP subprocess mode) ─────────

/// Run the MCP server loop over stdin/stdout.
///
/// Reads newline-delimited JSON tool calls and writes JSON responses.
/// Supports one tool: `run` with `{scenario_path: string, mock: bool}`.
pub fn serve_mcp(project_dir: &Path) -> Result<()> {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut stdout_lock = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line.context("reading stdin")?;
        if line.trim().is_empty() {
            continue;
        }

        let response = handle_mcp_call(&line, project_dir);
        let response_str = serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::json!({"error": e.to_string()}).to_string()
        });
        writeln!(stdout_lock, "{}", response_str).context("writing stdout")?;
        stdout_lock.flush().context("flushing stdout")?;
    }
    Ok(())
}

fn handle_mcp_call(line: &str, _project_dir: &Path) -> serde_json::Value {
    let call: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return serde_json::json!({
                "error": format!("invalid JSON: {}", e),
                "input": line,
            });
        }
    };

    let tool = call
        .get("tool")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match tool {
        "run" => {
            let scenario_path = match call
                .get("scenario_path")
                .and_then(|v| v.as_str())
            {
                Some(p) => p.to_string(),
                None => {
                    return serde_json::json!({
                        "error": "missing scenario_path",
                    });
                }
            };
            let mock = call
                .get("mock")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let path = Path::new(&scenario_path);
            match Scenario::load(path) {
                Err(e) => serde_json::json!({"error": e.to_string()}),
                Ok(scenario) => {
                    let opts = RunOptions { mock };
                    let transcript = run_scenario(&scenario, &opts);
                    serde_json::to_value(&transcript)
                        .unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}))
                }
            }
        }
        other => {
            serde_json::json!({
                "error": format!("unknown tool: {}", other),
                "available_tools": ["run"],
            })
        }
    }
}

// ────────────────────────────────────── Flagship scenario ───────────────────

/// Build the canonical flagship scenario in memory (for testing without fixture files).
pub fn flagship_scenario() -> Scenario {
    Scenario {
        scenario: "gross-margin-by-region-yoy".to_string(),
        description: "Gross margin by region year-over-year: EMEA vs AMER".to_string(),
        question: "What is the gross margin by region, year over year, for EMEA vs AMER?"
            .to_string(),
        model: Some(
            "internet_sales_catalog_BigQuery.internet_sales_schema.internet_sales".to_string(),
        ),
        steps: vec![
            ScenarioStep {
                pillar: "catalog-embed".to_string(),
                tool: "mqo-catalog-embed".to_string(),
                args: vec![
                    "embed".to_string(),
                    "--model".to_string(),
                    "internet_sales_catalog_BigQuery.internet_sales_schema.internet_sales"
                        .to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "catalog-embed",
                    "embedding_count": 42,
                    "top_match": "Gross_Profit_Margin"
                })),
            },
            ScenarioStep {
                pillar: "binding-confidence".to_string(),
                tool: "mqo-binding-confidence".to_string(),
                args: vec![
                    "score".to_string(),
                    "--query".to_string(),
                    "gross margin by region year over year".to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "binding-confidence",
                    "confidence": 0.91,
                    "verdict": "high"
                })),
            },
            ScenarioStep {
                pillar: "clarify".to_string(),
                tool: "mqo-clarify".to_string(),
                args: vec![
                    "check".to_string(),
                    "--query".to_string(),
                    "gross margin by region year over year EMEA vs AMER".to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "clarify",
                    "ambiguous": false,
                    "clarification_needed": null
                })),
            },
            ScenarioStep {
                pillar: "time-intelligence".to_string(),
                tool: "mqo-time-intelligence".to_string(),
                args: vec![
                    "resolve".to_string(),
                    "--expr".to_string(),
                    "year over year".to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "time-intelligence",
                    "period": "YOY",
                    "grain": "year",
                    "comparison": "current_year vs prior_year"
                })),
            },
            ScenarioStep {
                pillar: "engine-parity".to_string(),
                tool: "mqo-engine-parity".to_string(),
                args: vec![
                    "check".to_string(),
                    "--model".to_string(),
                    "internet_sales_catalog_BigQuery.internet_sales_schema.internet_sales"
                        .to_string(),
                    "--engines".to_string(),
                    "BigQuery,Snowflake".to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "engine-parity",
                    "engines_checked": ["BigQuery", "Snowflake"],
                    "parity": true,
                    "delta_pct": 0.0
                })),
            },
            ScenarioStep {
                pillar: "sensitivity-scan".to_string(),
                tool: "mqo-sensitivity-scan".to_string(),
                args: vec![
                    "scan".to_string(),
                    "--model".to_string(),
                    "internet_sales_catalog_BigQuery.internet_sales_schema.internet_sales"
                        .to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "sensitivity-scan",
                    "pii_fields_found": 0,
                    "safe": true
                })),
            },
            ScenarioStep {
                pillar: "ousia-grounding".to_string(),
                tool: "ousia-atscale".to_string(),
                args: vec![
                    "ground".to_string(),
                    "--model".to_string(),
                    "internet_sales_catalog_BigQuery.internet_sales_schema.internet_sales"
                        .to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "ousia-grounding",
                    "bfo_classes_assigned": 7,
                    "grounding_score": 0.88
                })),
            },
            ScenarioStep {
                pillar: "rosetta-credential".to_string(),
                tool: "rosetta-credential".to_string(),
                args: vec![
                    "sign".to_string(),
                    "--answer".to_string(),
                    "gross-margin-emea-amer-yoy".to_string(),
                ],
                mock_response: Some(serde_json::json!({
                    "status": "ok",
                    "pillar": "rosetta-credential",
                    "signed": true,
                    "credential_id": "cred-mock-0001"
                })),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── AC1: mock run executes all pillar steps in order and emits transcript ──

    #[test]
    fn ac1_mock_run_all_pillars_in_order() {
        let scenario = flagship_scenario();
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        assert_eq!(
            transcript.entries.len(),
            scenario.steps.len(),
            "transcript must have one entry per pillar"
        );
        for (entry, step) in transcript.entries.iter().zip(scenario.steps.iter()) {
            assert_eq!(
                entry.pillar, step.pillar,
                "pillar order must be preserved"
            );
        }
        assert!(
            transcript.blocked_at.is_none(),
            "mock run with all-ok responses must not block"
        );
        assert!(
            transcript.final_answer.is_some(),
            "mock run must produce final_answer"
        );
    }

    // ── AC2: transcript records exact subprocess invocation and verdict ──

    #[test]
    fn ac2_transcript_records_invocation_and_verdict() {
        let scenario = flagship_scenario();
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        for entry in &transcript.entries {
            assert!(
                !entry.invocation.is_empty(),
                "invocation must be non-empty for pillar {}",
                entry.pillar
            );
            assert!(
                entry.invocation.starts_with(&entry.tool),
                "invocation must start with the tool name for pillar {}",
                entry.pillar
            );
            // Verify no synthesised output — output must come from mock_response
            assert!(
                entry.output != serde_json::Value::Null || entry.verdict == Verdict::Skipped,
                "non-skipped entry must have non-null output"
            );
        }
    }

    // ── AC3: blocked step sets blocked_at; downstream steps are skipped ──

    #[test]
    fn ac3_blocked_step_sets_blocked_at_downstream_skipped() {
        let mut scenario = flagship_scenario();
        // Force the second step to produce a blocked verdict.
        if let Some(step) = scenario.steps.get_mut(1) {
            step.mock_response = Some(serde_json::json!({"status": "blocked"}));
        }
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);

        assert_eq!(
            transcript.blocked_at.as_deref(),
            Some("binding-confidence"),
            "blocked_at must identify the blocking pillar"
        );

        // All steps after index 1 must be skipped.
        for entry in transcript.entries.iter().skip(2) {
            assert_eq!(
                entry.verdict,
                Verdict::Skipped,
                "downstream pillar {} must be skipped after block",
                entry.pillar
            );
        }

        assert!(
            transcript.final_answer.is_none(),
            "final_answer must be absent when pipeline is blocked"
        );
    }

    // ── AC4: missing binary produces blocked_at (not panic) ──

    #[test]
    fn ac4_missing_binary_produces_blocked_not_panic() {
        let scenario = Scenario {
            scenario: "missing-binary-test".to_string(),
            description: "Test with missing binary".to_string(),
            question: "Does missing binary degrade legibly?".to_string(),
            model: None,
            steps: vec![ScenarioStep {
                pillar: "nonexistent-pillar".to_string(),
                tool: "__nonexistent_xyzzy_binary_12345__".to_string(),
                args: vec![],
                mock_response: None,
            }],
        };
        let opts = RunOptions { mock: false };
        let transcript = run_scenario(&scenario, &opts);

        assert_eq!(
            transcript.blocked_at.as_deref(),
            Some("nonexistent-pillar"),
            "missing binary must produce blocked_at"
        );
        assert_eq!(
            transcript.entries[0].verdict,
            Verdict::Blocked,
            "entry verdict must be Blocked"
        );
        assert!(
            transcript.final_answer.is_none(),
            "no final_answer when binary missing"
        );
    }

    // ── AC5: --format md renders per-pillar demo script ──

    #[test]
    fn ac5_render_markdown_has_per_pillar_sections() {
        let scenario = flagship_scenario();
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        let md = render_markdown(&transcript);

        for step in &scenario.steps {
            assert!(
                md.contains(&step.pillar),
                "markdown must contain pillar section for {}",
                step.pillar
            );
        }
        assert!(
            md.contains("# Demo Transcript"),
            "markdown must have a top-level heading"
        );
        assert!(
            md.contains("Final Answer"),
            "markdown must include final answer when not blocked"
        );
    }

    // ── AC7: mock mode is deterministic (excluding ms field) ──

    #[test]
    fn ac7_mock_mode_deterministic() {
        let scenario = flagship_scenario();
        let opts = RunOptions { mock: true };

        let t1 = run_scenario(&scenario, &opts);
        let t2 = run_scenario(&scenario, &opts);

        // Compare excluding ms fields.
        fn strip_ms(t: &Transcript) -> serde_json::Value {
            let mut v = serde_json::to_value(t).expect("serialize");
            if let Some(entries) = v.get_mut("entries").and_then(|e| e.as_array_mut()) {
                for entry in entries.iter_mut() {
                    if let Some(obj) = entry.as_object_mut() {
                        obj.remove("ms");
                    }
                }
            }
            v
        }

        assert_eq!(
            strip_ms(&t1),
            strip_ms(&t2),
            "two mock runs must produce identical transcripts (excluding ms)"
        );
    }

    // ── AC8: no network / no binaries needed in mock mode ──

    #[test]
    fn ac8_mock_mode_needs_no_binaries() {
        // Scenario with tools that definitely don't exist.
        let scenario = Scenario {
            scenario: "binary-free-test".to_string(),
            description: "Test binary-free mock pipeline".to_string(),
            question: "Does mock mode work without any binaries?".to_string(),
            model: None,
            steps: vec![
                ScenarioStep {
                    pillar: "fake-pillar-a".to_string(),
                    tool: "__fake_a__".to_string(),
                    args: vec![],
                    mock_response: Some(serde_json::json!({"status": "ok"})),
                },
                ScenarioStep {
                    pillar: "fake-pillar-b".to_string(),
                    tool: "__fake_b__".to_string(),
                    args: vec![],
                    mock_response: Some(serde_json::json!({"status": "ok"})),
                },
            ],
        };
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        assert_eq!(transcript.entries.len(), 2);
        assert!(transcript.blocked_at.is_none());
        assert!(transcript.final_answer.is_some());
    }

    // ── probe_steps returns available field ──

    #[test]
    fn probe_steps_returns_tool_availability() {
        let scenario = Scenario {
            scenario: "probe-test".to_string(),
            description: "Test step probing".to_string(),
            question: "Can I see step availability?".to_string(),
            model: None,
            steps: vec![
                ScenarioStep {
                    pillar: "real-tool".to_string(),
                    // Use a binary guaranteed to exist on any Linux system.
                    tool: "sh".to_string(),
                    args: vec![],
                    mock_response: None,
                },
                ScenarioStep {
                    pillar: "missing-tool".to_string(),
                    tool: "__definitely_absent_xyzzy__".to_string(),
                    args: vec![],
                    mock_response: None,
                },
            ],
        };
        let infos = probe_steps(&scenario);
        assert_eq!(infos.len(), 2);
        assert!(infos[0].available, "sh must be on PATH");
        assert!(!infos[1].available, "fake binary must not be on PATH");
    }

    // ── scenario JSON round-trips cleanly ──

    #[test]
    fn flagship_scenario_json_round_trip() {
        let scenario = flagship_scenario();
        let json = serde_json::to_string(&scenario).expect("serialize");
        let back: Scenario = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.scenario, scenario.scenario);
        assert_eq!(back.steps.len(), scenario.steps.len());
    }

    // ── MCP server handles unknown tool gracefully ──

    #[test]
    fn mcp_unknown_tool_returns_error() {
        let call = serde_json::json!({"tool": "nonexistent", "args": {}});
        let response = handle_mcp_call(&call.to_string(), Path::new("/tmp"));
        assert!(
            response.get("error").is_some(),
            "unknown tool must return error field"
        );
    }

    // ── MCP server handles run tool in mock mode ──

    #[test]
    fn mcp_run_tool_handles_missing_scenario_path() {
        let call = serde_json::json!({"tool": "run"});
        let response = handle_mcp_call(&call.to_string(), Path::new("/tmp"));
        assert!(
            response.get("error").is_some(),
            "missing scenario_path must return error"
        );
    }

    // ── blocked scenario has no final answer ──

    #[test]
    fn blocked_scenario_has_no_final_answer() {
        let mut scenario = flagship_scenario();
        // Make the first step block.
        if let Some(step) = scenario.steps.first_mut() {
            step.mock_response = Some(serde_json::json!({"status": "blocked"}));
        }
        let opts = RunOptions { mock: true };
        let transcript = run_scenario(&scenario, &opts);
        assert!(transcript.final_answer.is_none());
        assert!(transcript.blocked_at.is_some());
    }
}

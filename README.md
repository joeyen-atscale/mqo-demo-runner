# mqo-demo-runner

Calls a chain of AtScale MQO pillar CLIs in declared order and emits one transcript that records exactly what ran, what each step returned, and where the chain stopped.

A demo of a multi-step AI answer is only convincing if you can see the steps. This runner makes that the artifact: it spawns each pillar tool as a subprocess, captures the verbatim invocation and output of each, and writes a single transcript. If a step's binary is missing or a step fails, the chain short-circuits — the transcript names the pillar that blocked and marks everything downstream `skipped`, rather than fabricating an answer the upstream steps never produced. The runner itself contains no analytics; it is the conductor, and the pillar binaries are the instruments.

## Install

Requires a Rust toolchain (edition 2021, rustc 1.85+).

```sh
cargo build --release
install -Dm755 target/release/mqo-demo-runner ~/.local/bin/mqo-demo-runner
```

Or run in place with `cargo run --release -- <args>`.

## Two modes

The runner has two independent paths, both ordered subprocess chains:

- **Simple demo** (`run` with no `--scenario`, and `list`) — a fixed five-step script: BFO grounding, coverage report, aggregate advice, anomaly scan, semantic regression. Steps are filled in against a `--model` identifier.
- **Scenario pipeline** (`run --scenario <file>`, `steps`, `serve`) — an ordered chain of pillars read from a JSON scenario file, each declaring its own tool and arguments. The bundled flagship scenario has eight pillars.

Which path `run` takes is decided by one flag: pass `--scenario` and it runs the pipeline; omit it and it runs the simple demo.

## Quickstart

None of the pillar binaries are part of this repo, so a real run needs them on `PATH`. Without them, the simple demo marks each step `skipped` and the scenario pipeline blocks at the first missing tool. Use mock mode (scenario path) or accept skips (simple path) to see the shape of a run with nothing else installed.

```sh
# Simple demo, no siblings installed — every step skips, exit 0:
mqo-demo-runner run

# Preview the simple demo's steps and the exact commands, run nothing:
mqo-demo-runner list

# Scenario pipeline against the bundled flagship, canned responses, no binaries needed:
mqo-demo-runner run --scenario fixtures/scenarios/gross-margin-by-region-yoy.json --mock

# Same, rendered as a Markdown demo script instead of raw transcript JSON:
mqo-demo-runner run --scenario fixtures/scenarios/gross-margin-by-region-yoy.json --mock --scenario-format md

# Show the scenario's pillar order and which tool each needs, before a real run:
mqo-demo-runner steps --scenario fixtures/scenarios/gross-margin-by-region-yoy.json
```

`steps` prints each pillar with a `✓`/`✗ MISSING` marker for whether its binary is on `PATH`, so you know what a real run will skip before you start it.

## Subcommands

| Command | What it does |
| --- | --- |
| `run [--model <id>] [--dry-run] [--format text\|json]` | Run the simple five-step demo. `--dry-run` prints the commands without executing. |
| `run --scenario <file> [--mock] [--scenario-format json\|md] [--out <file>]` | Run the scenario pipeline. `--mock` uses each step's canned `mock_response`. Exits 1 if the pipeline blocked. |
| `list [--model <id>]` | Print the five simple-demo steps and their commands. Runs nothing. |
| `steps [--scenario <file>] [--json]` | Print scenario pillar order and per-tool `PATH` availability. Defaults to the bundled flagship. |
| `serve` | MCP subprocess server over stdin/stdout. Reads newline-delimited JSON; one tool, `run`, taking `{scenario_path, mock}`. |

## Scenario format

A scenario is a JSON file with an ordered `steps` array. Each step names a `pillar`, the `tool` binary to invoke, its `args`, and an optional `mock_response` used under `--mock`:

```json
{
  "scenario": "gross-margin-by-region-yoy",
  "description": "Gross margin by region year-over-year: EMEA vs AMER",
  "question": "What is the gross margin by region, year over year, for EMEA vs AMER?",
  "model": "internet_sales_catalog_BigQuery.internet_sales_schema.internet_sales",
  "steps": [
    {
      "pillar": "catalog-embed",
      "tool": "mqo-catalog-embed",
      "args": ["embed", "--model", "internet_sales_catalog_BigQuery..."],
      "mock_response": { "status": "ok", "top_match": "Gross_Profit_Margin" }
    }
  ]
}
```

In a real run, a step's verdict is `ok` on exit 0 and `blocked` on a non-zero exit; a step whose binary is not on `PATH` is `blocked` too. Under `--mock`, the verdict comes from the `status` field of `mock_response` (`ok`, `blocked`, or otherwise `failed`). The first blocked or failed step sets `blocked_at`; every step after it is `skipped`. A `final_answer` appears only when all pillars succeed — and it is a templated summary of the run, not a computed analytics result.

## How it fits

The runner orchestrates a chain of separate MQO pillar CLIs — among them `mqo-catalog-embed`, `mqo-binding-confidence`, `mqo-clarify`, `mqo-time-intelligence`, `mqo-engine-parity`, `mqo-sensitivity-scan`, `ousia-atscale`, and `rosetta-credential`. Those tools live in their own repos and are invoked by name off `PATH`; this repo is just the conductor and the transcript format.

## Status

Working orchestrator and transcript format, with a bundled flagship scenario and a full unit-test suite. Mock mode is self-contained and deterministic (the per-step `ms` timing is the only varying field). The mock responses are illustrative, and the pillar binaries the chain calls are not included here, so an end-to-end real run depends on installing them separately.

## License

MIT — Joe Yen

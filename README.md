# mqo-demo-runner

Orchestrate the five-pillar AtScale AI story as one provable, inspectable transcript.

Chains semantic retrieval, confidence scoring, clarification, time-intelligence, engine
parity, PII sensitivity, ontological grounding, and credential signing in declared order.
Each pillar's subprocess invocation is captured verbatim. A blocked step short-circuits
cleanly with `blocked_at` rather than fabricating downstream results.

## Subcommands

- `run [--scenario <path>] [--mock] [--format json|md] [--out <file>]` — execute the pipeline
- `steps [--scenario <path>] [--json]` — list pillar order and binary availability
- `serve` — MCP subprocess server over stdin/stdout (tool: `run`)

## Usage

```sh
# Run the flagship scenario in mock mode (no sibling binaries required):
mqo-demo-runner run --mock

# Run with a custom scenario file:
mqo-demo-runner run --scenario path/to/scenario.json --mock

# Render a Markdown demo script:
mqo-demo-runner run --mock --format md

# Check which binaries are available:
mqo-demo-runner steps

# Real run (requires sibling pillar CLIs on PATH):
mqo-demo-runner run --scenario fixtures/scenarios/gross-margin-by-region-yoy.json
```

## Scenario format

A scenario is a JSON file with ordered `steps`, each declaring a `pillar` name, `tool`
binary, `args`, and an optional `mock_response` for CI use:

```json
{
  "scenario": "gross-margin-by-region-yoy",
  "question": "What is the gross margin by region, year over year, for EMEA vs AMER?",
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

## Building

```sh
cargo build --release
install -Dm755 target/release/mqo-demo-runner ~/.local/bin/mqo-demo-runner
```

## License

MIT — Joe Yen

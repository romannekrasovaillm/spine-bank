# Spine Core

<p align="center">
  <img src="docs/screenshots/00-banner.png" alt="Spine — architecture control loop for CLI agents" width="100%">
</p>

<p align="center">
  <b>Spine: an architecture control loop — inside your CLI agent, or as a standalone harness with a TUI</b><br>
  <sub>GigaCode CLI · Claude Code · Kimi Code · Qwen Code · omp · OpenClaw — MCP server, 66 skills, gate hooks, API-key-free judging</sub>
</p>

<p align="center">
  <a href="https://github.com/romannekrasovaillm/spine-bank/actions/workflows/ci.yml"><img src="https://github.com/romannekrasovaillm/spine-bank/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-edition_2024-e43717?logo=rust&logoColor=white" alt="Rust edition 2024">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License MIT">
</p>

> **Language note.** The project's primary language is Russian: the full
> documentation, ADRs and user-facing text are RU. This file is a compact
> English entry point; [README.md](README.md) and
> [README-full.md](README-full.md) remain the source of truth.

## What it is

**Spine** is a *domain agent harness for solution architects*, built as one
Rust binary `arch-be` (TUI + CLI + library, MSRV 1.85). It gives a coding
agent — or a human architect — a deterministic, LLM-free control loop:

- **Fitness gates** over the repository (`CONSTRAINTS.yaml` — regex rules,
  file-exists, executable checks), with anti-weakening protection of the
  ruleset itself.
- **Architecture spine**: invariants with `Binds`/`Prevents`/`Rule` fields —
  only what independent implementers could get *incompatibly wrong* —
  enforced as a gate, not written up after the fact.
- **Change routing** (Fast/Standard/Critical) via a 15-trigger Architecture
  Significance Score derived from the git diff — not from self-assessment.
- **Traceability** REQ → NFR → AD/ADR → CMP → rule as a fitness function,
  a typed architecture model (`model/`), quantitative NFR checks, contract
  diffs (OpenAPI, proto/gRPC, Avro, JSON Schema, DDL) with breaking-change
  detection.
- **Rubric judging without API keys**: a CLI judge (`kind = "cli"`) or
  split-judge — Spine issues the prompt and JSON schema, your host agent
  judges k times, Spine builds the median and verifies every quote against
  the target document.
- **Evidence discipline**: evidence bundles with hash integrity, a verdict
  passport (`arch-be gate --explain` — what a green verdict does NOT mean),
  a trust metric (`arch-be trust`), mutation testing of the control loop
  itself (`arch-be redteam`).

## Who it is for

- **Architects** who work inside a coding agent and want verifiable
  architecture control (gates, traceability, contracts) without leaving it.
- **Teams** that need an auditable, deterministic release gate for
  agent-produced changes.
- **Researchers** studying SDD harnesses and agentic governance — the repo
  carries reproducible experiments under [docs/experiments/](docs/experiments/).

## Two editions, one codebase

| | **Spine Core** | **Spine Harness (TUI)** |
|---|---|---|
| For whom | You already have a coding agent — the architect works inside it | You are the architect, working without an external agent |
| What it is | An "organ" of your harness: MCP server + 66 skills + gate hooks | Full architect harness: TUI + agent loop + the same core |
| LLM | **None needed** — your agent thinks; judging via `kind="cli"` or split-judge | Built-in: DeepSeek / GLM / Kimi / GigaChat / self-hosted |
| Release binary | `arch-be-core-<platform>` (~10 MB) | `arch-be-<platform>` (~19 MB) |
| Build | `cargo build --release --no-default-features --features core` | `cargo build --release` |

## Install

From the latest release (Linux x86_64 example; also `linux-aarch64`,
`macos-arm64`, `windows-x86_64.exe`):

```bash
curl -L -o arch-be https://github.com/romannekrasovaillm/spine-bank/releases/latest/download/arch-be-core-linux-x86_64
chmod +x arch-be && mv arch-be ~/.local/bin/
```

Verify integrity with `SHA256SUMS` from the same release:
`sha256sum --check SHA256SUMS`.

From source:

```bash
cargo build --release          # full build (feature "harness", default)
cargo build --release --no-default-features --features core   # slim build
```

## 5-minute quickstart (Core inside your agent)

```bash
cd ~/projects/my-project
arch-be connect claude     # or: kimi / qwen / omp / codex / generic
```

`connect` writes the project MCP config (`arch-be mcp serve`), gate hooks
and the skill library — merging, never clobbering existing settings. Restart
your agent and approve the project server when prompted. From then on the
agent calls 40 read-only MCP tools (`fitness_check`, `architect_review`,
`significance_from_diff`, `trace_check`, `contract_diff`, `rubric_prompt` /
`rubric_verify`, …) and the Stop hook blocks a session that ends on a red
gate (exit 2 — findings go back to the agent as feedback).

No-LLM smoke test: `arch-be mermaid examples/mermaid/flow.mmd`,
`arch-be control score --trigger new_component=true`, `arch-be doctor`.

The standalone TUI harness (Mode 2) — `arch-be init`, API keys via
environment only, `arch-be` to launch — is covered in
[README.md](README.md) → «Режим 2» and [README-full.md](README-full.md).

## Where to read what

| Topic | Document |
|---|---|
| Full feature tour (RU) | [README.md](README.md), [README-full.md](README-full.md) |
| Getting started (RU) | [docs/getting_started.md](docs/getting_started.md) |
| Connecting harnesses, hooks, troubleshooting | [docs/CONNECT.md](docs/CONNECT.md) |
| Verified harness matrix (5 harnesses, live runs) | [docs/HARNESSES.md](docs/HARNESSES.md) |
| MCP server contract and tool map | [docs/mcp.md](docs/mcp.md), [docs/mcp_for_architects.md](docs/mcp_for_architects.md) |
| Control loop: fitness, gate, verdicts | [docs/control.md](docs/control.md), [docs/verdict.md](docs/verdict.md) |
| Architecture decisions (54 ADRs) | [docs/adr/](docs/adr/) |
| Spine Core API and boundary (workspace split plan) | [docs/SPINE-CORE-API.md](docs/SPINE-CORE-API.md) |
| Live-case experiment reports | [docs/experiments/](docs/experiments/) |
| Threat model, supply chain | [docs/threat-model.md](docs/threat-model.md), [docs/supply-chain.md](docs/supply-chain.md) |
| Worked example cases | [кейсы/](кейсы/) (registry: [кейсы/AGENTS.md](кейсы/AGENTS.md)) |
| Contributing: gates, deltas, ADRs, PR rules | [CONTRIBUTING.md](CONTRIBUTING.md) |

## License

Core — MIT ([LICENSE](LICENSE)). This repository is the public snapshot of
the **Spine Banking Edition** fork: the proprietary `banking/` layer
([LICENSE.banking](LICENSE.banking)) is not included here. Fork provenance
and licensing model — [NOTICE.md](NOTICE.md) (open-core dual, ADR-013).

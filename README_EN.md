
# ExMachina — A Collaborative Agent Cluster

> [!WARNING]
> This project is in pre-alpha. Interfaces and data formats may break without notice. Do not use in production.
> Contracts are defined in [docs/](./docs) (Chinese, source of truth).

<div align="center">

```text
███████╗  ██╗  ██╗  ███╗   ███╗   █████╗    ██████╗  ██╗  ██╗  ██╗  ███╗   ██╗   █████╗ 
██╔════╝  ╚██╗██╔╝  ████╗ ████║  ██╔══██╗  ██╔════╝  ██║  ██║  ██║  ████╗  ██║  ██╔══██╗
█████╗     ╚███╔╝   ██╔████╔██║  ███████║  ██║       ███████║  ██║  ██╔██╗ ██║  ███████║
██╔══╝     ██╔██╗   ██║╚██╔╝██║  ██╔══██║  ██║       ██╔══██║  ██║  ██║╚██╗██║  ██╔══██║
███████╗  ██╔╝ ██╗  ██║ ╚═╝ ██║  ██║  ██║  ╚██████╗  ██║  ██║  ██║  ██║ ╚████║  ██║  ██║
╚══════╝  ╚═╝  ╚═╝  ╚═╝     ╚═╝  ╚═╝  ╚═╝   ╚═════╝  ╚═╝  ╚═╝  ╚═╝  ╚═╝  ╚═══╝  ╚═╝  ╚═╝
```

![Stage](https://img.shields.io/badge/stage-pre--alpha-orange)
![Rust](https://img.shields.io/badge/Rust-stable-dea584?logo=rust&logoColor=white)
![License](https://img.shields.io/badge/license-MIT-blue)
![Tests](https://img.shields.io/badge/cargo%20test-passing-3fb950)
![WebUI](https://img.shields.io/badge/WebUI-React%2019%20%C2%B7%20Vite%20%C2%B7%20antd%206-61dafb?logo=react&logoColor=white)

</div>

<p align="center">
  <a href="./README.md">简体中文</a> · <a href="./README_EN.md">English</a>
</p>

---

## Quick Start

Prerequisites: [Rust](https://rustup.rs) (stable) and Node.js ≥ 22 (WebUI build only).

**One-click start** (downloads prebuilt release, no compilation):

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/scripts/quickstart.sh | bash
```

```batch
:: Windows
curl -fsSL -o quickstart.bat https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/scripts/quickstart.bat
quickstart.bat
```

Opens http://127.0.0.1:4173 — configure your API key on the Providers page.

**From source** (auto-installs Rust + Node, clones, builds, runs):

```bash
curl -fsSL https://raw.githubusercontent.com/KurohaneKaoruko/ExMachina/main/scripts/install.sh | bash
```

**Manual build**:

```bash
git clone https://github.com/KurohaneKaoruko/ExMachina.git && cd ExMachina
npm install && npm run build:webui
cargo run -p exm-gateway --release        # http://127.0.0.1:4173
```

## Why a Cluster, Not Another Assistant

Single-agent assistants hit a clear ceiling: **one context, one chain of thought, one answer**. The larger the task, the more likely they lose constraints, lose evidence, and compromise with themselves mid-chain — you can't trace how the conclusion was reached, or intervene when it goes off track.

EXMACHINA takes a different path: turning "accomplish something complex" into an **auditable organizational behavior**.

- **Division of labor** — The orchestrator does one thing: lock boundaries, decompose tasks, dispatch, arbitrate, converge. 39 sub-agents each hold a domain (research, architecture, implementation, verification, decision-making, operations, security, documentation…), producing in parallel.
- **Contract-driven** — Every sub-agent's output must pass SyncReport structural + business validation. Failures auto-rewrite (max 2 attempts), then escalate to the orchestrator.
- **Full auditability** — Three ledgers (task / evidence / risk) evolve per round, so every answer can answer "what's the evidence and where's the risk".
- **Composable** — Groups are the unit of isolation and switching. Build any team structure you need; the primary agent can expand its own roster mid-task.

## Core Features

**Cluster collaboration**
- Task decomposition into DAG, topological scheduling with concurrency pool, dynamic arbitration nodes
- SyncReport contract enforcement with auto-rewrite; full event sourcing for replay
- Agent groups with workspace isolation; primary agent can expand roster mid-task

**Memory & evolution**
- Hybrid retrieval: lexical (CJK bigram + inverted index) + semantic (embedding cosine, provider API)
- Auto-write: session digests, boundary decisions, A/B evidence, failure lessons
- Experience optimization: lessons distilled into behavioral notes, injected into future dispatches
- Reliability stats feed back into orchestrator routing

**Platform & models**
- 4 protocols: OpenAI-compatible, Anthropic, Google Gemini, Azure OpenAI
- Per-agent / per-group default model resolution (agent → group → global)
- Multi-key pools with sticky load balancing (rotate only on quota errors)
- Model failover chains with cooldown (45s) — switch only before content is emitted
- Native function calling across all 4 protocols; text-protocol fallback for mock/compat

**Voice & multimodal**
- Voice input: WebUI mic → API transcription (Whisper) → text; Telegram voice messages supported
- Image input: attach up to 4 images per message (vision-capable models)

**Distributed execution**
- Old devices join as worker nodes (`exm worker --url ws://hub:4173/worker`)
- Workers execute sub-agent tasks with their own local models (e.g., Ollama)
- Automatic fallback to local execution on failure / timeout

**Security & reliability**
- Terminal command approval gate (off / risky / always) + prefix allowlist + audit logging
- Session chat allowlist per channel
- Context compaction: rolling summary + recent window for multi-turn memory
- Session token budget (estimated chars/4, 0 = unlimited)
- Interrupted-session resume on gateway boot

**MCP client**
- stdio + Streamable HTTP transports, lazy initialize, per-agent tool visibility
- Tools mounted as `mcp:server:tool` into native function calling

## WebUI Console

14 panels with 5 accent themes: Chat (target switching, streaming, voice, images), Groups (roster + default model), Units, Providers, Task Graph (session-labeled DAG), Three Ledgers, Memory (deep db or memory.md file mode), Skills, Automations, Approvals, Channels, Events, Settings.

## Documentation

| Doc | Content |
|-----|---------|
| [docs/Installation](./docs/安装指南.md) | Setup, provider config, Docker |
| [docs/Usage](./docs/使用指南.md) | Full CLI + WebUI handbook |
| [docs/Architecture](./docs/架构与设计.md) | Design principles, core mechanisms |
| [docs/Protocol](./docs/协议与契约.md) | Entity schemas, REST/WS API, extension points |
| [PROGRESS](./PROGRESS.md) | Development log |

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) (bilingual zh/en).

## License

[MIT](./LICENSE)
# AI Session Replay contributor guide

Last updated: 2026-09-03

## Project state

AI Session Replay is a local-only Windows desktop application for indexing,
reviewing, presenting, and exporting supported AI coding sessions. The current
source tree is the initial public 0.0.1 version. There are no remaining tasks in
the approved implementation plan; new work should begin with an issue or an
explicitly approved specification change.

The canonical project documents are:

- `docs/spec.md` — product and engineering contract.
- `docs/plan.md` — current implementation status and later backlog.
- `CONTEXT.md` — approved domain language.
- `docs/indexed-session-evolution.md` — durable-library design record.
- `docs/security.md` and `SECURITY.md` — technical boundaries and private
  vulnerability reporting.
- `CONTRIBUTING.md` — contributor workflow and review expectations.

## Supported sources

- Claude Code
- Codex CLI
- GitHub Copilot CLI
- VS Code Stable and Insiders Copilot Chat
- Detector-only GitHub Copilot status for JetBrains IDEs; native JetBrains chat
  transcripts are not claimed as supported.

## Required toolchain

- Windows 10/11 x64
- Bun exactly 1.4.0
- Rust 1.90.0 with the components in `rust-toolchain.toml`
- Remotion packages exactly 4.0.516
- `cargo-llvm-cov` exactly 0.9.0 plus
  `nightly-2025-09-18-x86_64-pc-windows-msvc` for the full coverage gate

Common commands:

```powershell
bun install --frozen-lockfile
bun run tauri dev
bun run typecheck
bun run test
bun run test:rust
bun run check
bun run build:composition
bun run prepare:render-runtime
bun run check:visual-equivalence
bun run tauri build --no-bundle
```

`bun run prepare:render-runtime` creates ignored files under `.runtime/`. Run it
from the developer Bun installation, not the staged `.runtime` Bun executable,
because Windows cannot replace an executable that is currently running.

## Architecture and safety boundaries

- Rust owns discovery, bounded snapshots, normalization, SQLite, migrations,
  deletion, presentation-plan creation, and render-worker launch.
- The WebView receives opaque IDs and typed path-free DTOs. It has no generic
  SQL, filesystem, shell, or network capability.
- Treat database content, vendor records, transcript text, Markdown, paths,
  child-process output, and media metadata as untrusted.
- Never execute commands or arguments derived from transcript content. Render
  transcript content as inert React text; terminal links and active markup are
  forbidden.
- No runtime external network access or telemetry is allowed. The only runtime
  network exception is the ephemeral loopback Remotion server during export.
- Source files are read-only except for the explicit, separately confirmed,
  identity-checked deletion of one adapter-declared bounded session artifact
  set.
- Export destinations are user-selected, backend-validated local paths. Existing
  output files are never overwritten.
- Keep capabilities and CSP narrow unless a reviewed requirement explicitly
  changes the boundary.

## Data and test rules

- Use only synthetic scrubbed fixtures and temporary databases. Tests must not
  inspect real user stores.
- Never commit credentials, real transcripts, local paths, databases, videos,
  generated browsers/binaries, coverage, or build output.
- Every behavioral or security change requires automated coverage at the lowest
  useful layer and cross-layer coverage when persistence or rendering matters.
- New pure domain/database modules target at least 90% branch coverage. Adapter
  coverage has an explicit 80% threshold; indexed-library, database, and index
  coordinator scopes have 90% thresholds.
- Preserve visual stability during asynchronous transitions. Keep session
  metadata and controls visible while conversation entries scroll.

## Current verified baseline

- 296 TypeScript tests; aggregate branch coverage 91.66%.
- 364 Rust unit tests plus two isolated desktop integration tests.
- Adapter branch coverage 80.47%; indexed-library contract 95.34%; database
  90.12%; index coordinator 94.44%.
- Strict Clippy, production frontend/worker builds, the Tauri release build, and
  six-frame presentation/export pixel equivalence pass.

## Working agreement

- Keep changes focused and reviewable. Update the specification when behavior or
  a public boundary changes.
- Use parameterized SQL and validate every IPC/external boundary in both
  TypeScript and Rust where applicable.
- Preserve unrelated worktree changes. Do not stage generated files or personal
  data.
- Run task-specific checks while developing and the aggregate gate before a pull
  request is ready.

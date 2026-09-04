# Contributing to AI Session Replay

Thanks for helping improve AI Session Replay. The project is deliberately local,
offline-first, and strict about treating AI-session data as untrusted.

## Before you start

- Search existing issues before opening a new one.
- Use an issue to discuss substantial features, new source adapters, schema
  changes, capability changes, or new bundled binaries before implementation.
- Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md),
  not in a public issue.
- Never attach real transcripts, databases, source paths, tokens, credentials, or
  exported videos to an issue or pull request.

By contributing, you agree that your contribution is licensed under the
[MIT License](LICENSE).

## Development environment

Milestone one targets Windows 10/11 x64. You need:

- [Bun 1.4.0](https://bun.sh/)
- [Rust 1.90.0](https://www.rust-lang.org/tools/install); the repository's
  `rust-toolchain.toml` installs the required components
- WebView2, which is normally present on supported Windows versions

Clone and start the app:

```powershell
git clone https://github.com/dsebastien/ai-session-replay.git
Set-Location ai-session-replay
bun install --frozen-lockfile
bun run tauri dev
```

MP4 export additionally needs the pinned local runtime:

```powershell
bun run prepare:render-runtime
```

That command downloads and verifies pinned artifacts under the ignored
`.runtime/` directory. Run it with your developer Bun installation, not the
staged `.runtime` Bun executable.

## Project contracts

Read these before changing behavior:

- [Product and engineering specification](docs/spec.md)
- [Domain language](CONTEXT.md)
- [Security and privacy boundaries](docs/security.md)
- [Durable-library design record](docs/indexed-session-evolution.md)
- [Agent/contributor implementation guide](AGENTS.md)

Important invariants include:

- no runtime external network access or telemetry;
- no generic WebView filesystem, SQL, shell, or path access;
- inert rendering of transcript content;
- bounded, identity-checked source reads and deletion;
- synthetic fixtures only in automated tests;
- presentation and MP4 export consume the same frozen plan.

## Verification

Run the narrowest useful checks while developing:

```powershell
bun run typecheck
bun run test
bun run test:rust
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
bun run build
```

Before requesting final review, run the aggregate gate:

```powershell
bun run check
```

The Rust coverage portion additionally requires:

```powershell
rustup toolchain install nightly-2025-09-18-x86_64-pc-windows-msvc --profile minimal --component llvm-tools-preview
cargo install cargo-llvm-cov --version 0.9.0 --locked
```

Changes to shared presentation visuals should also run:

```powershell
bun run build:composition
bun run check:visual-equivalence
```

## Pull requests

- Keep each pull request focused on one behavior or coherent vertical slice.
- Add tests before or alongside behavior changes.
- Explain user-visible behavior, security implications, and verification.
- Update documentation when changing a contract, workflow, dependency, or safety
  boundary.
- Do not commit generated runtime files, build output, coverage, databases, real
  session data, or media exports.
- Confirm that your branch merges cleanly and that CI passes.

Maintainers may ask to split unrelated changes or revise an approach that
widens a security boundary without an approved requirement.

# AI Session Replay

AI Session Replay is a local-only Windows desktop application for indexing, reviewing, presenting, and explicitly exporting supported AI coding sessions from Codex CLI, Claude Code, GitHub Copilot CLI, and VS Code Copilot Chat. It also reports GitHub Copilot availability in JetBrains IDEs without claiming access to unsupported native JetBrains chat transcripts.

> **Project status:** early 0.0.1 release for Windows 10/11 x64. Interfaces and
> storage may change before the first stable release.

The library defaults to newest-first session ordering by source session date, falling back to indexed date when the source has no timestamp. Search covers titles and normalized conversation content, including reasoning, tool details, and file-change summaries. Use the **Sort** control beside search to switch between newest-first and oldest-first; filtering, full-text search, and paging retain the selected order.

## Features

- Durable local SQLite library that remains usable when source sessions move or
  disappear
- Full-text and date search, newest/oldest sorting, local names, and persisted
  entry curation
- Rich Claude Code, Codex CLI, GitHub Copilot CLI, and VS Code Copilot Chat
  normalization
- Full-screen keyboard-driven presentations and explicit offline MP4 export
- Narrow path-free IPC, strict CSP, bounded source access, and no telemetry

## Build from source

Requirements: Windows 10/11 x64, Bun 1.4.0, and the Rust toolchain declared by the repository.

```powershell
git clone https://github.com/dsebastien/ai-session-replay.git
Set-Location ai-session-replay
bun install --frozen-lockfile
bun run tauri dev
```

Useful verification commands:

```powershell
bun run typecheck
bun run test
bun run test:rust
bun run check
bun run check:release
```

`bun run check:release` prepares and verifies the pinned local render runtime,
builds the NSIS installer, and runs the installed export smoke test. It downloads
large verified tool archives into the ignored `.runtime/` directory.

All automated tests use synthetic fixtures and temporary databases. Do not add real transcripts, source paths, databases, videos, generated browsers, or runtime artifacts to Git.

## Data and privacy

- Session content is normalized into an app-owned plaintext SQLite database under the current Windows user's Tauri app-config directory.
- No external runtime network access or telemetry is permitted. Export uses only a temporary loopback Remotion server.
- Transcript content is rendered as inert text and never used as a command, path, SQL statement, URL, or HTML.
- Library deletion preserves vendor source files by default. Optional source deletion is separately confirmed and limited to adapter-declared session artifacts.
- **Settings → Reset local database** clears app-owned indexed data and suppression records while preserving vendor source files. A later manual refresh can index those sources again.
- Refresh reports indexed, unchanged, failed, suppressed, and source-warning counts separately. A partial or bounded scan retains every readable session it found and does not mark unseen sessions missing.

Reset performs a logical SQLite deletion with secure-delete enabled and best-effort WAL truncation/compaction. It is not a promise of forensic erasure from SSDs, backups, snapshots, or filesystem history.

This unreleased build uses `session-library-v3.sqlite3`. Earlier development databases are intentionally not migrated or opened; they remain untouched in the app-config directory and can be removed manually after confirming they are no longer needed.

Uninstalling the application does not promise to remove its app-config database. Users who want the indexed data removed should use **Reset local database** before uninstalling, then remove the application through Windows Settings.

## Presentation and export

Opening a session and entering presentation mode never create media. Presentation uses the persisted selection, visibility, timing, theme, and font settings; new sessions use a five-second entry delay by default. ANSI console styling is safely preserved in review, presentation, and export without activating terminal links or controls. **Export MP4** asks for a destination and sends the same frozen presentation plan to the bundled worker. An in-flight export can be cancelled and remains isolated in a kill-on-close Windows Job Object; a stable operation bar reports progress and a completed job can reveal its verified MP4 in Explorer.

Local export destinations may be ordinary folders or redirected local user folders such as OneDrive-backed Desktop/Documents. The resolved directory is pinned for safe atomic publication, and configured session-source roots remain excluded.

The NSIS installer includes Tauri's offline WebView2 installer. Export also packages pinned Bun, Chrome Headless Shell, Remotion compositor, FFmpeg, and FFprobe resources, so the installed app does not depend on global developer tools.

## Contributing and security

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening
a pull request and follow the [Code of Conduct](CODE_OF_CONDUCT.md). Report
vulnerabilities privately according to [SECURITY.md](SECURITY.md).

Architecture and constraints are documented in [the specification](docs/spec.md),
[implementation plan](docs/plan.md), [security boundaries](docs/security.md), and
[contributor implementation guide](AGENTS.md).

## License

Project-owned source code is available under the [MIT License](LICENSE).
Third-party components retain their own licenses; Remotion in particular is not
covered by this project's MIT license. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

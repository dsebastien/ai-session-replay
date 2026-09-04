# Spec: AI Session Replay

## Assumptions

1. The existing empty `ai-session-replay` directory is the project root and the product name is **AI Session Replay**.
2. Milestone one targets Windows 10/11 on x64. macOS, Linux, and Windows ARM64 are later work.
3. Project-owned code is MIT-licensed. Remotion remains a separately licensed, source-available dependency; users are responsible for complying with its terms.
4. All discovery, indexing, review, presentation, and rendering happen on-device with no account, uploads, analytics, telemetry, or external network traffic. Remotion may use an ephemeral loopback server only during an explicit export.
5. The supported local sources are Codex CLI, Claude Code, GitHub Copilot CLI, and VS Code Stable/Insiders Copilot Chat. JetBrains support is limited to honest installation/native-format detection until a documented transcript format is available.
6. Normalized session content is stored in plaintext SQLite under Tauri's per-user app-config directory within the current Windows user's roaming app-data profile and protected by that user's filesystem ACL. At-rest encryption is outside milestone one.
7. Previous immutable revisions remain hidden recovery history in v1. They are retained until the library session is deleted and are not exposed through a revision-history UI.
8. Deleted library sessions remain suppressed until the user explicitly restores their source. Library deletion preserves vendor data by default; a separate, additionally confirmed option may delete the exact source session artifacts.
9. Presentation timing uses a configurable fixed delay per newly revealed entry plus a playback-speed multiplier. The default delay is five seconds. Original wall-clock gaps are not replayed in milestone one.
10. Visibility preferences apply consistently to the conversation, presentation, and export. Entry selection is the definitive inclusion list.

## Objective

Build a local-first desktop application that discovers and indexes supported AI coding sessions into a durable app-owned library, lets an individual developer review and curate the complete conversation, presents the selected content interactively, and exports a deterministic 1920x1080 MP4 only after an explicit Export action.

The first release supports:

- Claude Code sessions under the configured Claude data root.
- Codex CLI active and archived rollout files.
- GitHub Copilot CLI session-state directories.
- VS Code Stable and Insiders workspace and empty-window Copilot Chat sessions.
- Detection-only status for GitHub Copilot in JetBrains configuration roots, with native JetBrains chat transcripts explicitly reported as unavailable.
- Durable browsing, presentation, and export after a source file moves or disappears.

### Primary user flow

1. Launch the desktop app, see a dedicated loading screen until the initial list/index state is reconciled, then browse previously indexed sessions.
2. Let a bounded background refresh scan supported sources, or start the same refresh manually.
3. Search session titles, normalized conversation text, and human-readable dates; sort newest-first or oldest-first; then open a complete, virtualized two-sided conversation.
4. Rename the session locally, show or hide supported tool calls/details/reasoning, and use arrow keys plus Space or pointer controls to select or exclude entries. The focused row is unmistakable, selection updates optimistically without disabling unrelated entries, and all new entries are selected by default.
5. Enter a full-screen, monospace presentation that focuses its stage, shows one full-width scrollable entry at a time, and supports play/pause, keyboard stepping, configurable delay, and speed.
6. Optionally choose Export, confirm a writable local destination, and render the same frozen presentation plan to a 1080p MP4. A stable neutral operation bar reports preparation, progress, cancellation, completion, and available actions; actual failures use toasts. Completion offers a direct “Show in folder” action.
7. Hide only the indexed copy by default, optionally and separately delete its exact source artifacts, or restore a suppressed source for future indexing.
8. From Settings, explicitly confirm a reset of all app-owned library data without changing any original source files; a later refresh may rebuild the library.

Scanning, opening a session, changing selections or preferences, and entering presentation mode never create a video.

### Non-goals

- Exact replicas of Claude Code, Codex, Copilot CLI, VS Code, or JetBrains interfaces.
- Cloud rendering, synchronization, sharing, accounts, or telemetry.
- Audio, narration, captions, multiple templates, collaborative editing, or auto-update infrastructure.
- A user-facing revision-history browser in milestone one.
- Preserving arbitrary raw vendor records or synthesizing reasoning that a source does not provide.
- Writing to, migrating, or repairing vendor session stores. Source deletion is the sole exception and occurs only through the explicit bounded operation in this spec.
- Parsing unsupported native JetBrains Copilot Chat caches by scraping private IDE data.

## Domain Language

- **Source location:** Backend-only locator for a vendor session file or bundle. It never crosses IPC.
- **Session:** Durable app-owned identity for one logical vendor conversation.
- **Revision:** Immutable normalized snapshot created when a session's canonical content hash changes.
- **Current revision:** Newest successfully indexed revision. Failed refreshes never replace it.
- **Entry:** Ordered unit in a revision: user message, assistant message, reasoning trace, tool call, file change, or unknown record.
- **Stable entry key:** Adapter-produced identity used to carry selection across revisions.
- **Entry selection:** Persisted include/exclude decision. Absence of an override means selected.
- **Visibility preferences:** Persisted display choices for tool calls, tool details, and reasoning. They do not mutate selection.
- **Local session title:** Optional app-owned display-name override that survives refresh/restart and never mutates a vendor source.
- **Presentation plan:** Immutable projection of one revision, its selections, visibility preferences, timing, and appearance. Presentation and export consume the same plan.
- **Suppression tombstone:** App-owned record preventing a deliberately deleted source from being automatically re-indexed.
- **Source-deletion plan:** Short-lived backend-owned description of the exact adapter-declared source artifacts that an explicitly confirmed destructive operation may remove.

## Tech Stack

Versions are evaluated and lock-pinned as of 2026-08-31.

| Area | Technology | Version |
| --- | --- | ---: |
| Runtime and package manager | Bun | 1.4.0 |
| Desktop shell | Tauri | 2.11.x |
| Backend | Rust | Edition 2024 |
| UI | React / React DOM | 19.2.8 |
| Build | Vite / React plugin | 8.2.2 / 6.1.0 |
| Styling | Tailwind CSS / Vite plugin | 4.3.3 |
| Database | `tauri-plugin-sql` with its SQLite feature and `STRICT` tables | `tauri-plugin-sql` 2.4.1 / SQLx 0.8.6, exact |
| Video | Remotion packages | 4.0.516, exact |
| Language | TypeScript | 7.0.2, strict |
| Unit/component tests | Vitest / Testing Library | 4.1.11 / 16.3.2 |

All Remotion packages use the exact same version without range prefixes. Rust exclusively owns SQLite through a narrow repository module backed by the Tauri SQL plugin; no JavaScript SQL binding, frontend SQL permission, or database capability is permitted.

## Architecture

- **Rust/Tauri core:** discovers stores, reads bounded snapshots, normalizes vendor records, owns SQLite and migrations, coordinates refresh, performs confirmed source deletion, and launches/cancels exports.
- **Versioned Rust/TypeScript contracts:** bounded DTOs for indexed summaries, revisions, entries, selections, preferences, refresh state, deletion confirmation, and presentation plans.
- **React application:** startup loading state, persistent source navigation, indexed-session browser, virtualized conversation workspace with pinned controls, toast notifications, curation controls, interactive presentation, and explicit export UI.
- **Shared presentation model:** React presentation and Remotion export share entry projection, timing/frame calculations, message components, and theme tokens.
- **Bun render worker:** calls Remotion server-side APIs against the prebuilt bundle. Tauri packages pinned Bun and browser/compositor resources, launches fixed arguments, and exchanges bounded newline-delimited JSON progress.

The WebView receives opaque IDs and typed path-free data, never source paths, database paths, SQL, generic filesystem access, or shell access. User-selected export destinations cross narrow Tauri commands, are canonicalized in Rust, and must resolve to allowed local Windows volumes. A restrictive CSP blocks external resources and navigation; transcript content is rendered only as inert text. Supported ANSI SGR styling is converted to safe React spans, while terminal hyperlinks and non-rendering control sequences are stripped rather than activated.

### Indexed-library ownership

- The database lives in Tauri's per-user app-config directory within the Windows roaming app-data profile, as required by the pinned SQL plugin's SQLite path mapper.
- Canonical source locations are backend-only UTF-16LE blobs so valid Windows paths do not require lossy Unicode conversion.
- SQLite enables foreign keys, WAL, a bounded busy timeout, and an explicit synchronous policy. Versioned migrations run transactionally before commands become available.
- All SQL is parameterized and contained in the repository module.
- Normalized content includes supported prompts, responses, tool details, file-change summaries, and source-provided reasoning traces, but not arbitrary raw vendor records.

### Revision and refresh policy

1. Return indexed sessions immediately at startup.
2. Discover bounded candidates and resolve their private source identities.
3. Ignore suppressed identities while reporting path-free progress.
4. Snapshot, normalize, validate, and hash canonical content.
5. For unchanged content, update last-seen metadata without creating a revision.
6. For changed content, insert an immutable revision and entries, carry explicit selection overrides by stable entry key, and atomically advance the current revision.

Each session refresh is one transaction. A read, parse, validation, permission, or disk failure retains the last-known-good revision and records only a safe diagnostic. Missing sources set `sourcePresent = false`; indexed content remains browseable, presentable, and exportable. Startup and manual refreshes coalesce, and monotonically increasing scan generations prevent stale results from winning.

### Selection, visibility, and timing

- Every entry is selected by default; only overrides need storage.
- Selection follows a stable adapter-provided entry key across revisions. New or uncertain identities are treated as new and selected rather than guessed.
- Visibility settings for tool calls, tool details, and reasoning affect all three viewing surfaces but do not change selection.
- Presentation reveals selected visible entries in order using the configured delay and speed multiplier; new sessions default to a five-second delay.
- Space toggles play/pause; Up/Down and Left/Right step; Home/End jump; pointer controls provide the same transitions. Reduced-motion behavior remains usable.
- Export freezes a validated presentation plan. Later edits or deletion cannot mutate an in-flight job.

### Deletion policy

- `delete_indexed_session` is the ordinary, default operation: it deletes app-owned rows and writes a suppression tombstone without touching source artifacts.
- Optional source deletion is a distinct destructive operation with an unchecked-by-default UI choice and an additional irreversible-action confirmation.
- The backend prepares a short-lived, one-use token bound to the session and current source identities. The WebView receives only the token and a path-free impact summary.
- On confirmation, Rust re-resolves the stored source identity, confines it to an approved vendor root, rejects changed identities and reparse-point or junction swaps, and deletes only the bounded artifact set declared by that adapter. It never recursively deletes a vendor root or project directory.
- Source artifacts are deleted before the library transaction. If source deletion fails, the indexed session remains intact. If the following database transaction fails, the indexed content remains available as source-missing and a safe error is reported.
- Both deletion modes leave a suppression tombstone. Explicit restore removes only that tombstone; it cannot recreate source data.
- Resetting the local database is a separately confirmed settings action. It atomically clears all app-owned session, preference, scan, diagnostic, and suppression data while retaining the migrated schema, never touches source artifacts, and is rejected while refresh is active.

## Strict Schema

All tables are `STRICT`. Booleans use `INTEGER NOT NULL CHECK(value IN (0, 1))`; timestamps are non-negative UTC epoch milliseconds; enums use `CHECK`; identifiers and user-visible strings have length bounds; foreign keys specify intentional cascades.

- **`sessions`:** opaque app ID, unique source/vendor-session identity, safe source title plus optional local title override, current revision, source-presence state, and first-indexed/last-seen timestamps.
- **`source_locations`:** session key plus backend-only canonical path blob, root identity, file identity, size, modified time, and last-seen scan. These fields never cross IPC or logs.
- **`session_revisions`:** immutable content hash, indexed time, bounded source metadata, event count, duration, and diagnostic count. Unique `(session_id, content_hash)` makes refresh idempotent.
- **`revision_entries`:** stable key, ordinal, relative time, kind, and kind-constrained typed payload columns. Primary key `(revision_id, entry_key)` and unique `(revision_id, ordinal)`.
- **`entry_selection_overrides`:** `(session_id, stable_entry_key)` plus selected state. Writes include the expected current revision ID.
- **`session_preferences`:** per-session visibility, delay, speed, palette, and font settings with database, Rust, and TypeScript validation.
- **`scan_runs`, `source_diagnostics`, and `suppressed_sources`:** bounded operational state and private suppression keys. Old scan rows are bounded; revisions are never automatically pruned.

## Narrow IPC Surface

- `list_indexed_sessions(filter, query, sortOrder, cursor)` returns bounded summaries and source-presence status; the cursor is bound to the filter, query, and sort order.
- `get_indexed_session(sessionId, entryCursor)` returns a validated paged conversation and preferences.
- `get_jetbrains_copilot_status()` returns path-free plugin/native-transcript/Copilot-CLI availability without exposing or parsing unsupported native chat storage.
- `refresh_index()` starts or joins the single refresh operation; typed progress contains counts and safe codes only.
- `set_entry_selections(sessionId, revisionId, changes)` persists a bounded batch and rejects stale revisions.
- `rename_indexed_session(sessionId, title)` persists an app-local title override without changing source data.
- `set_session_preferences(sessionId, revisionId, preferences)` validates and persists display/presentation settings.
- `delete_indexed_session(sessionId)` performs source-preserving library deletion and suppression.
- `prepare_source_deletion(sessionId)` returns a one-use confirmation token and path-free impact summary after backend validation.
- `delete_indexed_session_with_source(sessionId, confirmationToken)` revalidates and removes only the approved source artifacts before library deletion and suppression.
- `list_suppressed_sources(cursor)` returns a bounded path-free tombstone page so suppressed sources remain restorable after restart.
- `restore_suppressed_source(suppressionId)` removes a tombstone through an opaque ID without exposing its path.
- `reset_local_database()` accepts only the versioned reset mode, returns a path-free completion timestamp, and cannot run concurrently with index refresh.
- `create_presentation_plan(sessionId, revisionId)` returns a fully validated immutable path-free plan.
- Export accepts an opaque frozen-plan ID plus the explicitly confirmed output path; no other action starts a render job.

## Commands

```powershell
bun install --frozen-lockfile
bun run dev
bun run tauri dev
bun run typecheck
bun run test
bun run test:rust
bun run build
bun run build:composition
bun run prepare:render-runtime
bun run stage:render-worker
bun run tauri build --no-bundle
bun run check
```

## Project Structure

```text
docs/                       Product, implementation, architecture, and release docs
src/app/                    Desktop UI composition and state
src/components/             Accessible reusable UI components
src/features/sessions/      Indexed library and source navigation
src/features/conversation/  Virtualized review and curation workspace
src/features/presentation/  Interactive presenter and controls
src/features/export/        Explicit export lifecycle UI
src/lib/                    Typed Tauri client and pure utilities
packages/replay-contract/   Versioned normalized and indexed-library DTOs
packages/replay-engine/     Projection, presentation timing, and frame logic
packages/remotion-composition/ Deterministic export composition
packages/render-worker/     Bun entry point for local MP4 rendering
src-tauri/src/adapters/     Source-specific discovery and normalization
src-tauri/src/commands/     Narrow Tauri command handlers
src-tauri/src/database/     SQLite connection, migrations, and repository
src-tauri/src/discovery/    Roots, bounded snapshots, and refresh coordination
src-tauri/src/export/       Worker process and job lifecycle
src-tauri/src/model/        Rust contracts and validation
tests/fixtures/             Synthetic, scrubbed vendor-format fixtures
```

## Code Style

- TypeScript uses all configured strictness flags; exported contracts are immutable, versioned discriminated unions with exhaustive switches.
- React components use named exports and explicit props.
- Rust commands return typed values or stable structured errors; no panic or raw exception crosses a user-controlled boundary.
- Format-specific logic and source-deletion artifact declarations stay inside adapters. Shared code never switches on vendor-private fields.
- Comments explain unstable formats or non-obvious security constraints, not routine syntax.

## Testing Strategy

Every acceptance behavior and security boundary receives an automated test at its lowest useful layer, plus cross-layer integration coverage where persistence or rendering matters.

- **Schema/migrations:** clean creation, every upgrade path, strict type/enum/check/FK rejection, rollback, corrupt database, and too-new database.
- **Repository:** CRUD, parameter binding, transaction rollback, idempotent revisions, immutable history, stale writes, selection carry-forward, tombstones, missing sources, restart persistence, WAL/busy behavior, and IO failures.
- **Index coordinator:** coalesced startup/manual scans, unchanged/append/truncate/rewrite/disappear/reappear, bounded work, partial failures, shutdown, and last-known-good retention.
- **Adapters:** Claude Code, Codex CLI, GitHub Copilot CLI, and VS Code Copilot messages, reasoning, tool details, missing fields, unknown records, stable keys, hostile markup, limits, and bounded deletion artifact declarations.
- **IPC:** malformed IDs, oversized pages/batches, forged revision IDs, path leakage, stable error codes, and serialized-size limits.
- **Deletion:** default filesystem non-mutation, explicit confirmation, forged/stale/replayed tokens, root/identity revalidation, reparse/junction swaps, unrelated-file preservation, partial deletion failure, database failure after source removal, tombstone persistence, and restore.
- **Database reset:** strict request/response validation, explicit UI confirmation, transaction rollback, refresh exclusion, source non-mutation, restart persistence, stale-read invalidation, and re-indexing.
- **Conversation UI:** startup loading, empty/error/stale/source-missing states, virtualization limits, full-text/date search, sort order, pinned session details/controls, local rename, visibility, per-entry and bulk selection, stable active-card keyboard navigation, isolated optimistic selection saves, ANSI/control-sequence rendering, toast errors, optimistic rollback, and visual stability.
- **Presentation:** reducer/property tests for one-entry order, timer drift, pause/resume, speed, stepping, hidden categories, reduced motion, and end states; browser/native tests for initial focus, keyboard, full-width scrolling, and fullscreen behavior.
- **Export:** no implicit jobs, frozen-plan semantics, selected-entry filtering, presentation/export metadata equality, representative-frame pixel comparison, cancellation, and media verification.
- **End to end:** index synthetic sources, restart with sources removed, browse, curate, restart again, present, explicitly export, and exercise both deletion modes without touching real stores.

New pure domain/database modules target at least 90% branch coverage. Safety-critical validation, migrations, selection merging, deletion, and presentation-reducer branches require explicit boundary cases regardless of percentage. `bun run check` is the aggregate gate.

## Boundaries

### Always

- Treat vendor files, transcript text, Markdown, tool output, paths, database contents, worker output, and media metadata as untrusted.
- Resolve opaque IDs to approved backend-owned roots and identities at every filesystem operation.
- Read bounded snapshots and retain last-known-good revisions on refresh failure.
- Parameterize SQL and validate all IPC inputs and database outputs.
- Render transcript content as inert text; never inject transcript HTML or activate transcript links/images/scripts.
- Keep source navigation, the app header, session details, and primary session controls visible while only their content panes scroll; keep navigation collapse keyboard-operable.
- Present actionable operation failures as dismissible, accessible toast notifications without exposing private details.
- Preserve control/card geometry across asynchronous loading, progress, success, and error transitions.
- Keep presentation and export on one immutable projection/timing/visual model.
- Use only synthetic scrubbed fixtures and temporary databases in automated tests.
- Require an additional confirmation and backend identity revalidation before optional source deletion.

### Ask first

- Add a dependency with native code or a new external binary beyond approved bundled SQLite.
- Broaden Tauri filesystem, process, database, or network capabilities.
- Add external network access, telemetry, crash reporting, or remote session discovery.
- Change the indexed schema or IPC contracts incompatibly.
- Add automatic revision pruning or alter plaintext-at-rest policy.
- Expand source mutation beyond the approved, explicit deletion operation.
- Add a new export codec or redistribute additional media binaries.

### Never

- Upload session content or silently enable external network access.
- Log or return raw prompts, responses, source code, tool output, paths, exceptions, tokens, or secrets.
- Send SQL, database paths, source paths, or generic filesystem/shell capabilities to the WebView.
- Modify a vendor store except through explicitly confirmed source deletion of a backend-resolved, bounded session artifact set.
- Recursively delete a vendor root, project directory, or an artifact set containing an unknown/reparse-point entry.
- Save exports inside any configured vendor session root.
- Scrape arbitrary JetBrains caches looking for transcript-like strings.
- Execute commands or arguments obtained from session content.
- Use unpinned Remotion versions or bypass Remotion licensing notices.
- Commit credentials, real sessions, databases, videos, browser binaries, coverage, or build output.

## Success Criteria

1. After the startup loading state reconciles the library/index status, previously indexed sessions appear and remain usable after every source artifact is removed.
2. Unchanged refresh is idempotent; changed content creates one immutable revision; any failed refresh retains the current revision.
3. Synthetic Claude Code, Codex CLI, Copilot CLI, and VS Code Copilot fixtures normalize rich entries without panics, invented reasoning, or silent loss; JetBrains detection distinguishes absent, installed/native-unsupported, separate CLI, and explicitly attributed CLI states without scraping caches or projects.
4. Complete conversations remain navigable at the 100,000-entry bound and safely render untrusted content.
5. New entries default selected; explicit selections and visibility/presentation preferences survive restart and carry safely across revisions.
6. Presentation starts without rendering media, enters a true full-screen desktop view when available, focuses its stage, shows one full-width scrollable entry at a time, and supports equivalent keyboard/pointer state transitions.
7. Presentation and export consume the same frozen plan and agree on projected entries, duration, metadata, and representative frames.
8. Export begins only after explicit confirmation and produces a verified 1920x1080 H.264 `yuv420p` MP4 offline.
9. Library deletion never modifies source data by default and suppression prevents silent resurrection.
10. Optional source deletion cannot accept a path from the WebView, requires separate confirmation, preserves the indexed session on source failure, and never deletes unrelated source data.
11. The app makes no external network requests; only the export-time loopback composition server is permitted.
12. `bun run check` passes under Bun 1.4.0; the installed Windows x64 build works with global Bun absent and retains the library across upgrades.

## Risks

- **Sensitive plaintext database:** current-user ACL, Tauri app-config location within roaming app data, no raw records, no content/path logs, explicit deletion, and clear documentation.
- **Refresh corrupts curation:** immutable revisions, stable entry keys, expected-revision writes, transactional current-pointer swaps, and conservative new-entry handling.
- **Optional source deletion is irreversible:** safe default, distinct confirmation, one-use token, root/identity revalidation, bounded adapter-declared artifacts, and source-first failure semantics.
- **Vendor formats change:** guarded version dispatch, unknown accounting, synthetic conformance fixtures, and no invented data.
- **Large sessions or database growth:** bounded paging and virtualization, bounded scans, size reporting, and no automatic history deletion.
- **Presentation/export divergence:** one plan, shared projection/timing/visual primitives, metadata assertions, and pixel-equivalence coverage.
- **Render packaging/process risk:** pinned offline resources, loopback-only browser, bounded worker protocol, and kill-on-close Windows Job Objects.
- **Remotion licensing:** separate notice and no claim that Remotion is OSI open source.

## Source Basis

- Accepted design record: `docs/indexed-session-evolution.md`
- Remotion licensing: https://www.remotion.dev/docs/license/faq
- Remotion SSR with Bun: https://www.remotion.dev/docs/ssr-node
- Remotion version alignment: https://www.remotion.dev/docs/version-mismatch
- Tauri capabilities: https://v2.tauri.app/security/capabilities/
- Tauri Windows installer: https://v2.tauri.app/distribute/windows-installer/
- SQLite strict tables: https://www.sqlite.org/stricttables.html
- SQLite WAL: https://www.sqlite.org/wal.html
- Tauri SQL migrations and permissions: https://v2.tauri.app/plugin/sql/#migrations
- `tauri-plugin-sql` Rust API: https://docs.rs/tauri-plugin-sql/2.4.1/tauri_plugin_sql/
- Claude Code sessions: https://code.claude.com/docs/en/sessions.md
- Codex rollout source: https://github.com/openai/codex/tree/main/codex-rs/rollout
- Copilot CLI session state: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference
- JetBrains Windows configuration directories: https://www.jetbrains.com/help/idea/directories-used-by-the-ide-to-store-settings-caches-plugins-and-logs.html#config-directory
- Copilot CLI source reference: https://github.com/github/copilot-cli/tree/be82101e70f0253b57519bebb9cc9d0f6dfb2ed2
- VS Code chat-session storage: https://github.com/microsoft/vscode/blob/f9a71837c3e6a2b974948bd06b6f9c80b377e01e/src/vs/workbench/contrib/chat/common/model/chatSessionStore.ts
- VS Code chat mutation log: https://github.com/microsoft/vscode/blob/f9a71837c3e6a2b974948bd06b6f9c80b377e01e/src/vs/workbench/contrib/chat/common/model/objectMutationLog.ts

## Open Questions

None. The durable-library decisions and optional source-deletion behavior were approved on 2026-08-31. Changes to these assumptions or boundaries require updating this spec before implementation.

# Draft Spec and Implementation Plan: Durable Session Library

Status: **Approved and incorporated into `docs/spec.md` and `docs/plan.md`**
Date: 2026-08-31

This proposal supersedes the former Tasks 19–26 in `docs/plan.md`. Tasks 1–18 remain valid and complete. The user approved the five decisions below on 2026-08-31 and added an explicit, non-default option to delete a session's source artifacts together with its library record.

## Objective

Evolve AI Session Replay from a transient discovery-to-video editor into a durable, local session library:

1. Scan supported local AI tools at startup and on explicit refresh.
2. Normalize and index session content into an app-owned SQLite database.
3. Keep indexed sessions usable when their original files move or disappear.
4. Append immutable revisions when a source changes; never erase a last-known-good revision during refresh.
5. Display the complete session as a scannable two-sided conversation.
6. Persist per-entry inclusion choices, with every entry included by default.
7. Offer an interactive presentation mode with timed reveal, play/pause, speed, and keyboard stepping.
8. Export a video only after an explicit user action, using the same selected content and visual model as presentation mode.

Scanning, opening a session, changing selections, and entering presentation mode never create a video.

## Approved Assumptions

1. The database stores normalized content, including prompts, responses, tool details, file-change summaries, and source-provided reasoning traces. It does not retain arbitrary raw vendor records.
2. The database is local plaintext protected by the current Windows user's filesystem ACL. Database encryption is outside this milestone unless explicitly requested.
3. “Additive refresh” means revisions are immutable: unchanged content creates no revision; changed content creates a new revision and atomically becomes current; previous revisions remain until the user deletes the session.
4. A deleted library session receives a private suppression tombstone so the same source is not silently re-imported at the next scan. An explicit restore action can remove the tombstone.
5. Selection follows a stable normalized entry key across revisions. New entries are selected by default. If an adapter cannot prove identity after a rewrite, the entry is treated as new rather than guessing.
6. Conversation visibility and export inclusion are different concepts. Selection controls inclusion. `showToolCalls`, `showToolDetails`, and `showReasoning` control how included content is displayed in the conversation, presentation, and export.
7. Presentation timing initially uses a configurable delay per newly revealed entry plus a playback-speed multiplier. Preserving original wall-clock pauses can be added later without changing stored entries.
8. Assistant messages appear on the left, user messages on the right, and tool/reasoning/file entries appear as full-width subordinate cards.
9. Library deletion never touches source artifacts by default. The user may separately opt into deleting the exact backend-resolved source file or bounded session bundle after an additional irreversible-action confirmation. A source-deletion failure keeps the indexed session intact.

## Domain Language

- **Source location:** Backend-only locator for a vendor session file or bundle. It never crosses IPC.
- **Session:** Durable app-owned identity for one logical vendor conversation.
- **Revision:** Immutable normalized snapshot created when a session's canonical content hash changes.
- **Current revision:** The newest successfully indexed revision. Failed refreshes never replace it.
- **Entry:** Ordered unit in a revision: user message, assistant message, reasoning trace, tool call, file change, or unknown record.
- **Stable entry key:** Adapter-produced identity used to carry selection across revisions.
- **Entry selection:** Persisted include/exclude decision. Absence of an override means selected.
- **Visibility preferences:** Persisted display choices for tool calls, tool details, and reasoning. They do not mutate selection.
- **Presentation plan:** Immutable projection of one revision, its selections, visibility preferences, timing, and appearance. Presentation and export consume the same plan.
- **Suppression tombstone:** App-owned record preventing a deliberately deleted source from being automatically re-indexed.

## Architecture Decisions

### SQLite ownership

- Rust exclusively owns SQLite through a narrow repository module; the WebView receives no SQL, database path, generic query, or filesystem capability.
- Use an exactly pinned `tauri-plugin-sql` release with only its SQLite feature and its built-in migration system. Do not grant any `sql:*` WebView capability or install the JavaScript SQL binding. If the Rust repository needs a direct SQLx dependency to access the plugin-managed pool, pin it exactly to a version compatible with the selected plugin release. Verify that SQLite remains bundled and fully offline.
- Store the database under Tauri's per-user app-config directory within Windows roaming app data, matching the pinned SQL plugin's SQLite path mapper. Canonical source locations remain backend-only and are stored as UTF-16LE blobs so valid Windows paths do not need lossy Unicode conversion.
- Enable `foreign_keys=ON`, `journal_mode=WAL`, a bounded busy timeout, and an explicit synchronous policy. Run versioned migrations in a startup transaction before commands become available.
- All SQL is parameterized and contained in the database/repository module. UI commands expose typed operations only.

### Revision and refresh policy

```text
discover bounded candidates
        |
        v
resolve private source identity ---- suppressed? ---> report ignored
        |
        v
snapshot + normalize + validate
        |
        v
canonical content hash ---- unchanged ---> update last-seen metadata only
        |
      changed
        v
insert immutable revision + entries
carry explicit selection overrides by stable entry key
atomically advance current_revision_id
```

- Startup first returns indexed sessions immediately, then runs a bounded background refresh.
- Explicit Refresh uses the same coordinator and reports per-source progress without replacing the visible library.
- Each session refresh is one transaction. A read/parse/validation/disk-full failure retains the current revision and records only a safe diagnostic.
- Missing sources set `sourcePresent = false`; indexed content remains browseable, presentable, and exportable.
- Normal append-only growth creates one new revision whose unchanged entry keys retain selection. Truncation or rewrite also creates a revision; the old snapshot remains available internally.
- Concurrent startup/manual refreshes coalesce into one scan. A monotonically increasing scan generation prevents stale results from winning.

### Deletion policy

- Ordinary library deletion removes app-owned session content and writes a suppression tombstone; it never modifies vendor data.
- Optional source deletion is a distinct, explicit destructive operation. The WebView supplies an opaque session ID and one-time confirmation token, never a path.
- Rust resolves the stored source identity again, confines it to an approved vendor root, rejects stale identities and reparse-point or junction swaps, and deletes only the adapter-declared session artifact set. It never recursively deletes a vendor root or project directory.
- Source artifacts are deleted before the library transaction. If source deletion fails, the indexed session remains usable. If the subsequent library transaction fails, the retained indexed session is marked source-missing and the failure is reported with a safe code.
- Successful deletion still creates a suppression tombstone so sync or backup restoration cannot silently re-import the session. Explicit restore removes only the tombstone; it cannot recreate deleted source data.

### Presentation and export

- The normal session view is a virtualized conversation, not a video player.
- Presentation mode is an animated React view. It reveals selected visible entries in order and supports Space for play/pause, Left/Right for previous/next, Home/End, an accessible speed control, and a configurable delay.
- Entering presentation mode does not start an export or launch the render worker.
- Export requires an explicit button, destination confirmation, and a frozen `PresentationPlan`. Later edits do not mutate an in-flight job.
- React presentation and Remotion export share message components, theme tokens, entry projection, and frame/timing calculations. Representative states receive pixel-equivalence coverage.

## Proposed Strict Schema

All tables are `STRICT`; booleans use `INTEGER NOT NULL CHECK(value IN (0, 1))`; timestamps are non-negative UTC epoch milliseconds; enums use `CHECK`; IDs and user-visible strings have length checks; foreign keys specify intentional cascades.

### `sessions`

- `id TEXT PRIMARY KEY`: random opaque app ID.
- `source TEXT NOT NULL CHECK (...)` and `vendor_session_id TEXT NOT NULL` with a unique pair.
- Safe display metadata: title, created time, source version, collection, display filename.
- `current_revision_id TEXT`, `source_present INTEGER`, `first_indexed_at_ms`, `last_seen_at_ms`.

### `source_locations`

- Session foreign key plus backend-only canonical path blob, root identity, file identity, size, modified time, and last-seen scan.
- Never serialized to the WebView or included in logs/errors.

### `session_revisions`

- `id TEXT PRIMARY KEY`, session foreign key, 32-byte canonical content hash, indexed time, source metadata, event count, duration, diagnostic count.
- Unique `(session_id, content_hash)` makes refresh idempotent.

### `revision_entries`

- Revision foreign key, stable entry key, ordinal, relative time, kind, and typed nullable payload columns.
- Primary key `(revision_id, entry_key)` and unique `(revision_id, ordinal)`.
- Kind-specific `CHECK` constraints prevent impossible payload combinations.
- Content and count limits mirror the Rust/TypeScript boundary validation.

### `entry_selection_overrides`

- `(session_id, stable_entry_key)` primary key and `selected INTEGER`.
- Only deviations from the default need storage; new entries therefore remain selected.
- Writes include the expected current revision ID to reject stale UI edits.

### `session_preferences`

- One row per session containing tool-call, tool-detail, and reasoning visibility; presentation delay; playback speed; palette; and font settings.
- Every value has database constraints and is revalidated in Rust and TypeScript.

### `scan_runs`, `source_diagnostics`, and `suppressed_sources`

- Track bounded status/progress without raw exception text.
- Suppression keys are backend-only and prevent automatic re-import after deletion.
- Old scan rows are bounded; session revisions are not automatically pruned.

## Narrow IPC Surface

- `list_indexed_sessions(filter, query, cursor)` returns bounded summaries and source-presence status.
- `get_indexed_session(sessionId, entryCursor)` returns a validated, paged conversation read model and persisted preferences.
- `refresh_index()` starts or joins the single refresh operation; typed progress events contain counts and safe codes only.
- `set_entry_selections(sessionId, revisionId, changes)` persists a bounded batch atomically and rejects stale revisions.
- `set_session_preferences(sessionId, revisionId, preferences)` validates and persists presentation/display settings.
- `delete_indexed_session(sessionId)` deletes app-owned content and creates a suppression tombstone; this default operation never touches vendor files.
- `prepare_source_deletion(sessionId)` validates the current backend-only source identity and returns a short-lived, one-use confirmation token plus a path-free impact summary.
- `delete_indexed_session_with_source(sessionId, confirmationToken)` revalidates the bound source identity, deletes only the approved session artifact set, then deletes app-owned content and creates the tombstone.
- `create_presentation_plan(sessionId, revisionId)` returns a fully validated immutable plan with no paths.
- Export accepts that validated plan plus the user-selected output path only after explicit confirmation.

## Testing Strategy

“Tests for everything” means every acceptance behavior below gets an automated test at its lowest useful layer, with cross-layer integration tests for persistence and rendering:

- **Schema/migrations:** clean creation, every upgrade path, `STRICT` type rejection, enum/check/FK constraints, rollback on migration failure, corrupt/too-new database handling.
- **Repository:** CRUD, parameter binding, transaction rollback, revision idempotency, immutable history, stale-write rejection, selection carry-forward, tombstones, missing-source retention, restart persistence, WAL/busy behavior, and disk/IO failures.
- **Deletion:** library-only non-mutation, explicit source-delete confirmation, forged/stale/replayed token rejection, root and identity revalidation, junction/reparse swaps, bounded multi-artifact deletion, unrelated-file preservation, source-delete failure retention, post-source database failure recovery, tombstone persistence, and explicit restore.
- **Index coordinator:** startup/manual coalescing, unchanged/append/truncate/rewrite/disappear/reappear cases, bounded batches, partial source failure, cancellation/app shutdown, and last-known-good retention.
- **Adapters:** synthetic fixtures for messages, reasoning, tool calls/details, missing fields, unknown records, stable keys, malicious markup, and size limits for every supported source.
- **IPC contracts:** malformed IDs, oversized pages/batches, forged revision IDs, no path leakage, stable error codes, and serialized size limits.
- **Conversation UI:** all loading/empty/error/stale/source-missing states, virtualization boundaries, left/right roles, visibility toggles, select/deselect/bulk operations, optimistic rollback, keyboard use, and visual stability.
- **Presentation:** pure reducer/property tests for order, timer drift, pause/resume, speed, stepping, hidden categories, reduced motion, and end states; browser tests for keyboard/focus/fullscreen behavior.
- **Export:** no implicit job creation, frozen-plan semantics, selected-entry filtering, presentation/export metadata equality, representative-frame pixel comparison, cancellation, and output verification.
- **End to end:** index synthetic sources, restart with sources removed, browse full content, change selections, restart again, present, then explicitly export.

New pure domain/database modules target at least 90% branch coverage. Safety-critical validation, migration dispatch, selection merge, and presentation reducer branches must have explicit boundary cases rather than relying on a percentage alone. `bun run check` remains the aggregate gate and gains database plus browser/integration stages that can run with the prepared offline runtime.

## Implementation Tasks

### Phase A — Freeze the model

#### Task 19: Approve the durable-library specification

**Description:** Reconcile this approved proposal into `docs/spec.md`, replace the former Tasks 19–26 in `docs/plan.md`, record the glossary, and discard the superseded uncommitted Player-first work.

**Acceptance:** Product semantics above are explicit; no unresolved choice can change the schema; Tasks 1–18 remain intact; optional source deletion is explicit and safe by default; the user's existing `docs/spec.md` wording edit is preserved and remains unstaged.

**Verify:** Documentation review plus `git diff --check`.

**Dependencies:** Task 18. **Scope:** Small.

#### Task 20: Define indexed-library contracts

**Status:** Complete.

**Description:** Add versioned Rust/TypeScript DTOs for summaries, revisions, entries, selections, preferences, sync state, deletion confirmation, and presentation plans.

**Acceptance:** All DTOs are bounded and runtime-validated; entry kinds model optional reasoning/tool detail honestly; paths and raw records are absent; library-only and source-destructive deletion are distinct typed operations.

**Verify:** Cross-language serialization fixtures and hostile-payload tests.

**Dependencies:** Task 19. **Scope:** Medium.

### Phase B — Durable indexed library

#### Task 21: Bootstrap strict SQLite storage

**Status:** Complete.

**Description:** Add the pinned Tauri SQL plugin with its SQLite driver, Tauri app-config path resolution within Windows roaming app data, connection policy, and migrations registered through `Builder::add_migrations`.

**Acceptance:** Startup preloads the app-owned database and applies ordered embedded migrations before commands become available. A fresh database contains only versioned `STRICT` tables with all required constraints; migration failure leaves the previous version usable. No SQL plugin capability or JavaScript binding is exposed to the WebView, and repository access remains behind typed Rust commands.

**Verify:** Temporary-database migration/constraint/corruption tests and Tauri startup smoke.

**Dependencies:** Task 20. **Scope:** Medium.

#### Task 22: Persist sessions and immutable revisions

**Status:** Complete.

**Description:** Implement the repository transaction that inserts a session/revision/entries or recognizes an unchanged content hash.

**Acceptance:** Inserts are atomic and idempotent; current revision advances only after full validation; prior revisions cannot be updated through repository APIs.

**Verify:** Repository tests for new, unchanged, appended, rewritten, rollback, restart, and concurrent-reader cases.

**Implementation record (2026-08-31):** The backend-only typed repository validates bounded input before opening an atomic transaction, writes immutable revisions and ordered entries with parameterized bounded batches, recognizes unchanged hashes, preserves curation and prior revisions, and returns stable path-free failures. Synthetic coverage includes all variants and boundaries, conflicts, rollback, restart, concurrent WAL readers, retained selections/preferences, and batching.

**Dependencies:** Task 21. **Scope:** Medium.

#### Task 23: Build the startup and refresh index coordinator

**Status:** Complete.

**Description:** Connect existing bounded discovery/snapshot/adapters to the repository with coalesced startup/manual scans and safe progress.

**Acceptance:** UI can read old indexed data while refresh runs; source failures retain last-known-good revisions; missing sources are marked without deleting content.

**Verify:** Synthetic three-source integration tests for unchanged/change/disappear/reappear, partial failure, and concurrent refresh.

**Implementation record (2026-08-31):** The backend coordinator starts after migration, coalesces startup/manual refreshes, emits typed path-free progress, and writes bounded monotonic scan generations. Guarded snapshots from all three v1 sources become immutable repository revisions; unchanged content is idempotent, partial failures retain last-known-good state, authoritative disappearance changes presence without deleting content, and suppression is checked before and after normalization. Synthetic coverage includes lifecycle changes, concurrent readers/callers, cancellation, root denial, malformed content, storage faults, deferred sources, suppression, and batch boundaries; focused branch coverage is 100%.

**Dependencies:** Tasks 17, 22. **Scope:** Medium.

#### Task 24: Switch the library UI to indexed sessions

**Status:** Complete.

**Description:** Replace transient catalog loading with paged database-backed summaries and session details while retaining source filters, search, fixed navigation, and visual stability.

**Acceptance:** Indexed sessions appear immediately after restart and remain openable with all source files removed; refresh progress never unmounts the library; no source path crosses IPC.

**Verify:** Component states plus restart/source-removal desktop integration test.

**Implementation record (2026-08-31):** Bounded backend repository reads, narrow validated path-free commands, and a strict frontend client now drive the indexed library. The UI supports source filters, search, pagination, durable two-sided conversations, missing-source state, and stable in-place refresh progress. Completed generations reconcile visible summaries and details while stale loads/events are rejected. The transient discovery/load IPC was removed from production. Synthetic tests cover request/response boundaries, cursor scope, refresh failures and races, restart, and source removal; an isolated real-Tauri test proves migrated indexed content remains readable across application restart with the source absent. Database branch coverage is 96.92%.

**Dependencies:** Task 23. **Scope:** Medium.

#### Task 25: Add safe library/source deletion and suppression

**Status:** Complete.

**Description:** Add default library-only deletion, optional separately confirmed source deletion, cascaded app-owned cleanup, explicit suppression restore, and persistent source tombstones.

**Acceptance:** Vendor files are untouched by default. Source deletion requires a one-use confirmation token, backend root and identity revalidation, and an adapter-declared bounded artifact set. Failure leaves the indexed session intact; success creates a tombstone; refresh does not resurrect a suppressed source; explicit restore makes an extant source indexable again.

**Verify:** Repository, forged/stale/replayed confirmation, filesystem non-mutation, junction/reparse race, unrelated-file preservation, partial failure, restart, tombstone restore, and UI confirmation tests.

**Implementation record (2026-09-01):** Strict cross-language contracts, transactional repository operations, suppression-aware refresh, handle-bound Windows deletion, adapter-declared artifact scopes, narrow IPC, and safe-default confirmation UI now implement the complete lifecycle. Tombstones are listed through a bounded path-free cursor API so restore remains discoverable after restart. Source artifacts are re-resolved and identity-checked immediately before deletion; live confirmations are capped; partial source or subsequent database failures retain the indexed copy, with the latter marked source-missing. Synthetic tests exercise all confirmation, identity, filesystem, transaction, paging, restart, restore, and async UI race boundaries, and pinned Chrome verifies the responsive dialogs and library toolbar.

**Dependencies:** Task 24. **Scope:** Medium.

### Checkpoint — Durable library

- A synthetic session survives two application restarts and complete source removal.
- Refresh is idempotent when unchanged and additive when changed.
- Library-only deletion never modifies source data; optional source deletion cannot remove unrelated data; neither mode silently re-imports.
- Aggregate TypeScript/Rust/database checks pass.

### Phase C — Rich review and persistent curation

#### Task 26: Add stable rich-entry normalization

**Status:** Complete.

**Description:** Version the normalized entry model with stable entry keys, reasoning entries, tool details, and explicit availability flags.

**Acceptance:** V1 data migrates without invented detail; new fields are bounded and inert; selection identity rules are deterministic.

**Verify:** Cross-language fixtures, V1 migration tests, malformed payload tests, and stable-key property tests.

**Implementation record (2026-09-01):** TypeScript and Rust now share a strict normalized-session V2 shape with bounded rich entries, explicit reasoning/tool-detail availability, and opaque stable keys. A deterministic V1 migration retains already-safe keys, maps legacy non-opaque IDs without embedding their content, and never invents unavailable reasoning or tool detail. The indexer consumes and hashes V2, persists every rich variant, and retains the narrow frontend contract. Content-derived adapter keys are stable under append-only growth and deliberately change when an identity-bearing payload changes. Synthetic cross-language, migration, malformed-payload, persistence-mapping, and property coverage exercise these rules.

**Dependencies:** Tasks 20–22. **Scope:** Medium.

#### Task 26B: Add a confirmed local-database reset setting

**Status:** Complete.

**Description:** Add a settings action that atomically clears all app-owned SQLite library state while preserving vendor source artifacts and the migrated schema.

**Acceptance:** The action requires explicit destructive confirmation, cannot race an active refresh, clears indexed content and suppression state, resets the in-memory library/refresh view, never mutates vendor sources, and allows a later explicit refresh to rebuild the library.

**Verify:** Contract, repository transaction/rollback, refresh-race, source non-mutation, restart, component state, and responsive browser tests.

**Implementation record (2026-09-01):** The Settings dialog now provides a separately confirmed reset for app-owned SQLite state. A narrow validated command coordinates with refresh, rejects an active scan, clears all application rows in one transaction, retains migrations and the open hardened pool, and resets the in-memory refresh state. The UI invalidates outstanding list/detail reads only after success and retains the current library on failure. Tests cover request/response validation, rollback, restart, source-file non-mutation, re-indexing, refresh races, stale frontend responses, and safe error rendering; pinned headless Chrome verifies the toolbar and both dialog stages responsively.

**Dependencies:** Tasks 21, 23–26. **Scope:** Medium.

#### Task 26C: Preserve sessions during partial discovery

**Status:** Complete.

**Description:** Keep readable sessions visible when another candidate is locked, disappears during traversal, or a bounded scan cannot inspect the complete source tree.

**Implementation record (2026-09-01):** Discovery now returns partial candidates with a safe source diagnostic and withholds authoritative-completion status, preventing both all-or-nothing candidate loss and false missing-source updates. Synthetic locked-sibling and traversal-limit regressions cover the reported missing-session behavior.

#### Task 27: Enrich the Codex adapter

**Status:** Complete.

**Description:** Extract source-supported reasoning traces and tool detail into the rich-entry contract without retaining raw records.

**Acceptance:** Supported data is preserved, absent data stays absent, unknown accounting remains exact, and stable keys survive append-only growth.

**Verify:** Current/legacy/unknown/malicious/append fixtures and conformance tests.

**Implementation record (2026-09-01):** Codex V2 normalization preserves explicit reasoning summaries and bounded function/shell arguments and results, correlates supported call IDs, uses vendor IDs for stable keys when available, and excludes encrypted and unknown raw payloads.

**Dependencies:** Task 26. **Scope:** Medium.

#### Task 28: Enrich the Claude adapter

**Status:** Complete.

**Description:** Extract source-supported thinking/tool-use/tool-result detail and stable relationships into rich entries.

**Acceptance:** Same guarantees as Task 27, including partial/missing blocks and redacted synthetic fixtures only.

**Verify:** Current/unknown/tool/thinking/append fixtures and conformance tests.

**Implementation record (2026-09-01):** Claude V2 normalization preserves explicit thinking, tool input/result detail, and session relationships while keeping mixed/unknown blocks conservative and inert. Vendor UUIDs and tool-use IDs provide stable identities where present.

**Dependencies:** Task 26. **Scope:** Medium.

#### Task 29: Enrich the Copilot CLI adapter

**Status:** Complete.

**Description:** Extract tool detail and reasoning only where the guarded source format explicitly provides it.

**Acceptance:** No inference from unrelated settings/cache files; absent reasoning is reported as unavailable; stable keys survive append-only growth.

**Verify:** Current/unknown/settings/tool/append fixtures and conformance tests.

**Implementation record (2026-09-01):** Copilot CLI V2 normalization preserves explicit reasoning and bounded tool details from event records, keeps settings metadata-only, and uses event/tool IDs for append-stable identity. Missing detail and reasoning remain explicitly unavailable.

**Dependencies:** Task 26. **Scope:** Medium.

#### Task 30: Build the virtualized conversation view

**Description:** Display the current revision as a terminal-inspired, two-sided conversation with assistant left, user right, and subordinate tool/reasoning cards.

**Acceptance:** The complete logical conversation is navigable at the 100,000-entry bound; transcript markup stays inert; active links/images/scripts are impossible.

**Verify:** Component tests, large-list performance fixture, accessibility scan, and real WebView smoke at mobile/desktop sizes.

**Dependencies:** Tasks 24, 26. **Scope:** Medium.

#### Task 31: Persist visibility and entry selection

**Description:** Add show/hide controls, per-entry checkboxes, and bounded bulk select/deselect with optimistic UI and transactional persistence.

**Acceptance:** Every new entry defaults selected; explicit choices survive restart and refresh; stale revision writes reconcile safely; visibility does not silently alter selection.

**Verify:** Reducer/component/repository tests plus refresh-and-restart end-to-end coverage.

**Dependencies:** Tasks 26, 30. **Scope:** Medium.

### Checkpoint — Review and curation

- All three source fixtures render as a readable conversation.
- Tool details/reasoning can be independently shown or hidden when available.
- Selection survives app restart and a source revision update.
- A missing source remains fully reviewable from SQLite.

### Phase D — Interactive presentation

#### Task 32: Build the deterministic presentation-plan projector

**Description:** Convert one revision plus selection, visibility, timing, and appearance into a bounded immutable sequence consumed by both UI and export.

**Acceptance:** Deselected entries are absent; hidden categories follow explicit preferences; timing is finite/deterministic; empty projections produce a typed validation error.

**Verify:** Table/property tests across selection, visibility, timing bounds, revision mismatch, and one-entry sessions.

**Dependencies:** Task 31. **Scope:** Medium.

#### Task 33: Build the presentation controller

**Description:** Implement a pure playback state machine and the presentation-mode shell.

**Acceptance:** Play/pause, speed, delay, Left/Right, Home/End, focus handling, reduced motion, and completion behavior are deterministic; no video/export job starts.

**Verify:** Fake-clock reducer tests, keyboard component tests, and real WebView interaction smoke.

**Dependencies:** Task 32. **Scope:** Medium.

#### Task 34: Share the visual conversation stage

**Description:** Extract message/tool/reasoning visual primitives and theme tokens for conversation, presentation, and Remotion without sharing stateful browser controls.

**Acceptance:** User/assistant sides and visibility match across modes; appearance changes are validated and immediate; async states do not shift established controls.

**Verify:** Component snapshots/DOM assertions, hostile-markup tests, font-load smoke, and responsive visual review.

**Dependencies:** Tasks 30, 33. **Scope:** Medium.

#### Task 35: Adapt Remotion to presentation plans

**Description:** Make the composition reveal the same projected entries at the same deterministic boundaries as presentation mode.

**Acceptance:** Remotion receives a frozen validated plan; selection/visibility/timing/theme/font are honored; no interactive transcript content exists.

**Verify:** Frame-state boundary tests and composition build.

**Dependencies:** Tasks 32, 34. **Scope:** Medium.

#### Task 36: Prove presentation/export equivalence

**Description:** Finish an offline harness that renders representative presentation states and staged-export frames through pinned Chrome and compares decoded pixels within a fixed tolerance.

**Acceptance:** Metadata and pixels match at first, transition, tool/reasoning visibility, and final states; font readiness is explicit; the harness uses loopback only.

**Verify:** Dedicated integration command plus aggregate CI/runtime gate.

**Dependencies:** Task 35. **Scope:** Medium.

### Checkpoint — Presentation

- Entering presentation mode is instant and never creates media.
- Keyboard and pointer controls produce identical state transitions.
- Presentation and export use one immutable plan and pass frame equivalence.

### Phase E — Explicit export and release hardening

#### Task 37: Bind export jobs to durable presentation plans

**Description:** Complete the existing Windows Job Object export lifecycle using a frozen plan loaded by opaque IDs and an explicitly chosen destination.

**Acceptance:** Export starts only from the Export action; edits/deletion cannot mutate an in-flight plan; all existing path/process/privacy constraints remain enforced.

**Verify:** Forged-command, race, cancellation, restart-cleanup, and verified-MP4 integration tests.

**Dependencies:** Tasks 21, 35–36. **Scope:** Medium.

#### Task 38: Build explicit export UI

**Description:** Add destination choice, summary, confirmation, progress, cancellation, retry, and job-scoped open action to the curated session workspace.

**Acceptance:** The UI clearly separates Present from Export; no background action launches rendering; progress transitions are visually stable and accessible.

**Verify:** Component state tests and synthetic end-to-end export/open flow.

**Dependencies:** Task 37. **Scope:** Medium.

#### Task 39: Harden database lifecycle and installed builds

**Description:** Verify app-config-directory permissions, migration/recovery behavior, WAL cleanup, installer resources, offline guarantees, and uninstall data policy.

**Acceptance:** Installed builds retain the library across upgrades; corrupt/too-new databases fail safely; uninstall behavior is documented; no remote requests or path leaks occur.

**Verify:** Installed-app migration/restart test, network-denial test, clean-machine package smoke, and binary/resource audit.

**Dependencies:** Tasks 21–38. **Scope:** Medium.

#### Task 40: Documentation, adversarial review, and release gate

**Description:** Update user/developer/security documentation and run independent reviews across schema, sync, UI, presentation, export, privacy, and specification compliance.

**Acceptance:** All findings are resolved or explicitly accepted; synthetic end-to-end flow passes; deferred v2 adapters remain out of milestone one.

**Verify:** `bun run check`, installed-runtime smoke, staged diff/secret scan, and independent multi-axis review.

**Dependencies:** Tasks 19–39. **Scope:** Medium.

## Dependency Summary

```text
approved spec
    -> contracts -> SQLite -> revision repository -> index coordinator -> indexed library
                      |                                      |
                      +-> selections/preferences <-----------+
                                      |
rich entry model -> source adapters -> conversation review/curation
                                      |
                            presentation plan
                             /             \
                  interactive presenter   Remotion composition
                             \             /
                             parity harness
                                   |
                         explicit export lifecycle/UI
                                   |
                          installer/security/release
```

## Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Database contains sensitive transcript content | Current-user roaming app-data ACL at Tauri's app-config path, no raw records, no path/content logs, explicit deletion, documented plaintext-at-rest assumption. |
| Optional source deletion removes vendor data irreversibly | Default to library-only deletion; require a separate confirmation token; revalidate backend-owned root and file identities; delete only a bounded adapter-declared artifact set; retain indexed data on source-delete failure. |
| Refresh corrupts user curation | Immutable revisions, stable entry keys, selection overrides, expected-revision writes, transactional current-pointer swap. |
| Vendor rewrites make entries look identical | Prefer vendor IDs; otherwise deterministic conservative keys and treat uncertain matches as new/selected. |
| Startup indexing is slow | Show indexed data first, hash unchanged candidates, bounded queue, coalesced refresh, progress without layout shifts. |
| Database grows indefinitely | Report size, never auto-delete user history, offer explicit revision/library cleanup in a later approved slice. |
| Large conversations degrade UI | Bounded paged IPC, virtualization, batched selection writes, performance fixtures at contract limits. |
| Presentation diverges from export | One presentation plan, shared projection/timing/visual primitives, browser-to-export pixel harness. |
| “Thinking” is absent or semantically different by vendor | Availability flags and source-honest adapter extraction; never synthesize or infer hidden reasoning. |

## Approved Decisions

Approved by the user on 2026-08-31:

1. SQLite remains plaintext under the Windows user profile for milestone one.
2. Previous immutable revisions remain hidden recovery history in v1.
3. Deletion suppresses automatic re-import until explicit restore.
4. Presentation timing starts with a fixed per-entry delay and speed multiplier.
5. Visibility preferences apply equally to conversation, presentation, and export by default, while selection remains the definitive inclusion list.
6. Library deletion is source-preserving by default; users may explicitly choose the separately confirmed source-deletion operation described above.

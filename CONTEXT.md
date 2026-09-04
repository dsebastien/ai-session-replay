# AI Session Replay Context

Use `docs/spec.md` for approved product and engineering requirements and `docs/plan.md` for dependency order and task acceptance criteria.

## Domain Glossary

- **Source location:** Backend-only locator for a vendor session file or bundle. It never crosses IPC.
- **Session:** Durable app-owned identity for one logical vendor conversation.
- **Revision:** Immutable normalized snapshot created when a session's canonical content hash changes.
- **Current revision:** Newest successfully indexed revision. Failed refreshes never replace it.
- **Entry:** Ordered unit in a revision: user message, assistant message, reasoning trace, tool call, file change, or unknown record.
- **Stable entry key:** Adapter-produced identity used to carry selection across revisions.
- **Entry selection:** Persisted include/exclude decision. Absence of an override means selected.
- **Visibility preferences:** Persisted display choices for tool calls, tool details, and reasoning. They do not mutate selection.
- **Presentation plan:** Immutable projection of one revision, its selections, visibility preferences, timing, and appearance. Presentation and export consume the same plan.
- **Suppression tombstone:** App-owned record preventing a deliberately deleted source from being automatically re-indexed.
- **Source-deletion plan:** Short-lived backend-owned description of the exact adapter-declared source artifacts that an explicitly confirmed destructive operation may remove.

## Approved Product Decisions

- SQLite is plaintext under the Windows user's app-data profile for milestone one.
- Previous immutable revisions are hidden recovery history in v1.
- Deleted sessions remain suppressed until explicitly restored.
- Presentation uses a fixed per-entry delay and speed multiplier.
- Visibility preferences apply equally to conversation, presentation, and export; selection controls inclusion.
- Library deletion preserves source data by default. Optional source deletion is a distinct, additionally confirmed backend operation that never accepts a source path from the WebView.

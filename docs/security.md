# Security and privacy boundaries

- The WebView receives typed path-free session data and opaque identifiers. It has no SQL, generic filesystem, shell, or network capability.
- SQLite is backend-owned, strict, parameterized, plaintext, and stored beneath Tauri's per-user app-config directory.
- The current unreleased schema uses `session-library-v3.sqlite3`; incompatible earlier development databases are neither opened nor silently deleted.
- Vendor records, paths, transcript text, tool arguments/results, diagnostics, process output, and media metadata are untrusted and bounded before use.
- Transcript fields render as React text. Active transcript-provided HTML, scripts, links, images, commands, and URLs are forbidden.
- Startup and manual indexing use identity-checked snapshots. Partial scans retain readable candidates but are never authoritative for source absence.
- Refresh telemetry distinguishes candidate failures, suppression, and source-level warnings; unavailable optional roots are not counted as failed sessions.
- Source deletion requires a short-lived one-use token, revalidates roots and identities, rejects reparse points, and deletes only the adapter-declared bounded artifact set.
- VS Code discovery is limited to Stable/Insiders workspace `chatSessions` and default-profile empty-window chat directories. It does not descend into unrelated workspace storage; paired `.json`/`.jsonl` source deletion remains bounded to the selected session stem.
- JetBrains detection checks only the documented roaming configuration root and exact `<product>/options/github-copilot.xml` files. It never scans caches, logs, `.idea` directories, or native chat-like data. Copilot CLI attribution examines only a bounded event-log prefix and requires an explicit source/client metadata value; transcript mentions never count as attribution.
- App-local session names are validated and stored only in SQLite. Refresh never writes them to source files or replaces them with a newly observed vendor title.
- Local-database reset is separately confirmed, excludes active refresh, executes its logical deletion in one transaction, and never mutates source artifacts. Secure-delete and best-effort WAL truncation/compaction reduce residual pages but do not promise forensic erasure from storage hardware, backups, or snapshots.
- Export starts only from the explicit Export action. The destination is selected by the user, the plan is frozen, the worker environment is scrubbed, browser traffic is loopback-only, and the process tree is owned by a kill-on-close Job Object. Rust follows an explicitly selected redirected local folder once, pins its resolved directory by handle, excludes resolved vendor roots, exclusively reopens and hashes the verified staging file, then atomically publishes it and deletes the staging link by handle.
- Presentation may toggle only the current Tauri window's fullscreen state through the narrow `core:window:allow-set-fullscreen` capability; it receives no broader window-management or filesystem authority.
- Errors and progress expose stable codes/counts only. Raw transcript text, paths, exceptions, tokens, and process output must not be logged or returned.

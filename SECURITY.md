# Security policy

## Supported versions

AI Session Replay is currently an early 0.0.x project. Security fixes are made
on the latest `main` branch and the latest published release, when releases are
available. Older development snapshots are not supported.

## Reporting a vulnerability

Please do not disclose a suspected vulnerability in a public issue, discussion,
or pull request.

Use GitHub's private vulnerability reporting page:

<https://github.com/dsebastien/ai-session-replay/security/advisories/new>

Include the affected version or commit, impact, reproduction steps using
synthetic data, and any suggested mitigation. Do not include real transcripts,
credentials, tokens, private paths, or other personal data.

You should receive an acknowledgement within seven days. Valid reports will be
investigated privately, with remediation and coordinated disclosure handled
according to severity and exploitability.

## Security model

The application processes untrusted local AI-session data. Its key boundaries
include path-free WebView contracts, strict and parameterized SQLite access,
bounded source snapshots, inert transcript rendering, narrow Tauri capabilities,
explicit source-deletion confirmation, and offline render workers. See
[docs/security.md](docs/security.md) for the detailed design.

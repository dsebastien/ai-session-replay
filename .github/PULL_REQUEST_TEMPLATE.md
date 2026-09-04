## Summary

Describe the user-visible outcome and why this change is needed.

## Security and privacy

Describe any change to source discovery, filesystem access, SQLite, IPC,
capabilities, child processes, networking, or untrusted transcript handling.
Write “No boundary changes” when none apply.

## Verification

List the exact commands and manual checks performed.

## Checklist

- [ ] The change is focused and matches an issue or approved requirement.
- [ ] Tests cover the changed behavior and relevant boundaries.
- [ ] TypeScript/Rust validation remains strict and path-free where applicable.
- [ ] Documentation is updated for changed behavior or contracts.
- [ ] No real transcripts, databases, paths, credentials, tokens, media, runtime binaries, or build output are included.
- [ ] `bun run check` passes, or the reason it could not be run is documented.

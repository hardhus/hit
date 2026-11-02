# Changelog

## [0.1.0] - 2026-11-03

The first release. Open it in the morning, see what needs you, deal with it.

- **Focus:** reviews waiting for you, your pull requests with failing checks or
  requested changes, mentions and assignments, most urgent first.
- **Inbox:** notifications with filters, marked read from the keyboard.
- **Pull requests:** conversation, comments on lines, highlighted diff, checks.
- **Issues:** lists with a label picker, and the full conversation.
- **Repository context:** opens on the repository of the current folder.
- **Actions (`x`):** comment, approve, request changes, merge, close, reopen, and
  run failed checks again.
- **Accounts:** device-flow sign-in, several accounts, read-only mode.
- **Settings (`,`):** apply at once, saved without touching your comments.
- **Command line:** `hit focus`, `inbox`, `pr`, `issue`, `repos`, with `--json`.
- **Local cache:** encrypted, for an instant start and working offline.
- `hit --demo` to try everything without an account.

Known limits: no inline review comments yet, no opening of issues or pull
requests, no CI logs. GitHub only for now.

Tested by hand on Windows, and on Linux only briefly and without a desktop
keyring. macOS builds pass the automated tests but have not been used by hand;
reports are welcome.

The binaries are not signed, so Windows SmartScreen and macOS Gatekeeper may warn
the first time you open them. On macOS, `xattr -d com.apple.quarantine hit` clears
it. `SHA256SUMS` lists each file's checksum.

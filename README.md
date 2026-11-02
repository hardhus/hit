# hit

Your GitHub morning in one terminal window. Type `hit` and see what needs you:
reviews waiting for you, your pull requests with failing checks or requested
changes, mentions, assignments. Then deal with it right there.

hit does not try to put all of GitHub in your terminal. It does the things you
do every day, comfortably, from the keyboard. For everything else, `o` opens the
same item in your browser.

> **Status:** early. It does what is described here, but expect rough edges and
> please report them. GitHub only for now; the core is built so that other forges
> can be added.

## What it does

- **Focus:** one list of what needs you, most urgent first.
- **Inbox:** your notifications, filtered and marked read from the keyboard.
- **Pull requests:** conversation, comments on lines, highlighted diff (unified
  or side by side), checks.
- **Issues:** lists with a label picker, and the full conversation.
- **Repository context:** inside a git repository, hit opens on that repository.
  `.` switches between it and everything.
- **Act on it:** comment, approve, request changes, merge, close, reopen, and run
  failed checks again. Press `x` on anything to see what you can do with it.
- **Read-only mode:** when you only want to look, hit refuses to write anything
  before a request is even sent.
- **Instant start:** the last data is kept locally, encrypted, and refreshed in
  the background. Old data is always marked as old.

Not there yet: inline review comments, opening issues and pull requests, editing
labels and assignees, and CI logs. Your browser is one key away.

## Try it without an account

```sh
hit --demo
```

Realistic sample data. Press any key you like: nothing leaves your machine, and
changes last until you quit.

## Install

**Prebuilt:** download the archive for your system from
[Releases](https://github.com/hardhus/hit/releases), unpack it and put `hit`
somewhere on your `PATH`. Each release lists SHA-256 checksums.

**With Cargo** (Rust 1.90 or newer):

```sh
cargo install hit-cli --locked
```

The command it installs is `hit`.

On Linux, tokens are kept in the Secret Service (GNOME Keyring, KWallet and so
on). If you have none, see `--insecure-storage` below.

macOS builds are compiled and tested automatically, but the author does not use
macOS; reports are welcome.

## Sign in

```sh
hit auth login
```

This uses GitHub's device flow: hit shows a code, you approve it in the browser.
hit asks for the `repo`, `read:org` and `notifications` scopes. Tokens go to your
system keyring, never to a config file.

| | |
|---|---|
| `hit auth status` | accounts, scopes and access problems |
| `hit auth logout` | removes the token and that account's local data |
| `hit auth login --read-only` | this account never writes |
| `hit auth login --with-token` | use a token you already have |
| `hit auth login --from-gh` | import the token of the GitHub CLI |
| `hit auth login --insecure-storage` | a private file instead of the keyring |

Sign in again to add another account; `a` switches between them.

**Missing an organization?** Organizations can restrict third-party apps. Until
an owner approves hit, that organization's repositories quietly do not appear.
`hit auth status` tells you where to ask.

## Keys

`?` shows them all in hit. The ones you will use most:

| | |
|---|---|
| `1`–`5`, `tab` | Focus, Inbox, Pull requests, Issues, Repos |
| `j` `k`, `J` `K` | move, jump between groups |
| `↵` | open |
| `x` | what can be done with this item |
| `o` | open in the browser |
| `.` | this repository / all repositories |
| `/` | filter |
| `f` | switch view |
| `r` | refresh |
| `:` | command palette, and jump to any `#123` |
| `,` | settings |

The action menu, on a pull request: `c` comment, `a` approve, `r` request changes,
`m` merge, `t` run failed checks again, `k` close. In the comment box, `ctrl+s`
sends, `tab` previews the Markdown and `ctrl+e` moves to your `$EDITOR`.

## From the command line

The same work without the TUI, for scripts. `--json` prints the raw result.

```sh
hit focus
hit inbox --all
hit pr list --review
hit pr view 7
hit pr approve 7 --body "Looks good"
hit pr merge 7             # shows what would happen; add --yes to merge
hit issue close 12 --not-planned
hit repos --counts
```

Inside a repository they use it; elsewhere add `--repo OWNER/NAME`.

## Configuration

Press `,` in hit: changes apply at once and are saved for you. Or edit the file:

```sh
hit config init    # a commented config.toml
hit config check   # warnings about it
hit paths          # where everything lives
```

Your comments in the file are kept when hit saves a setting. Keys are rebound
under `[keys]`.

## Privacy and security

- hit talks only to GitHub. No telemetry, no other servers.
- Tokens stay in the system keyring and are sent only to GitHub's API, over
  https, with redirects turned off.
- Local data is encrypted with a key derived from each account's token.
  `hit cache clear` deletes it, and signing out removes that account's.
- Logs do not contain tokens or what you write.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

# t3-import

Bring your existing Claude Code, Codex, Grok and OpenCode sessions into [T3 Code](https://github.com/pingdotgg/t3code), with their full history, so you can pick up where you left off instead of starting over.

T3 Code's built-in importer only covers Claude and Codex, only looks at the last 30 days, and imports every recent thread of a folder at once, capped at 100 threads and 200 messages each. t3-import lists every session on your machine, lets you choose which ones to bring in, and imports the complete conversation.

Imported threads resume the original session in its own CLI: T3 continues the same Claude, Codex, Grok or OpenCode conversation, not a copy.

## Use

Close T3 Code first: the app, the server, and the background service, if you installed it. t3-import writes to T3's database and refuses to run while T3 is open.

```sh
t3-import
```

| Key | |
|---|---|
| `space` | select a session, or every session of a project |
| `a` | select everything visible |
| `enter` / `→` / `←` | open or close a project |
| `/` | search titles, folders and session ids |
| `1`–`4` | show or hide Claude, Codex, Grok, OpenCode |
| `m` | keep only the newest N messages per thread (default: all) |
| `i` | review and import |

Sessions already in T3 are marked `in T3` and can't be selected again.

Each session goes into the T3 project for the folder it ran in, and t3-import creates missing projects with T3's own `t3 project add`. A session that ran inside a git worktree goes into the main repository's project and keeps its worktree and branch.

Scripting:

```sh
t3-import list --json
t3-import check
t3-import import claude:<session-id> codex:<session-id> --dry-run
```

## Safety

- **Backup first.** Before writing, t3-import copies `statev2.sqlite` to `statev2.sqlite.bak-t3import-<timestamp>` next to it. To undo an import, close T3 and copy the backup back.
- **Fail closed.** T3 Code changes quickly. Before writing anything, t3-import compares the tables it writes to, and the shape of the data already in them, with what it knows. If anything differs, it stops without touching the database and gives you a prefilled link to open an issue here, so support can be added.
- **One transaction.** All threads are written in a single transaction and re-checked before commit. Either everything lands or nothing does.
- **Read-only on your CLIs.** Session files and databases of Claude, Codex, Grok and OpenCode are only read.

## What gets imported

The complete user and assistant text of each conversation. Tool calls and their output aren't imported yet. Imported messages have no checkpoints, so you can't revert or fork from points inside the imported history; everything after the import works normally. T3's own importer has the same limitation.

## Install

macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/paulomanrique/t3code-import/releases/latest/download/t3-import-installer.sh | sh
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/paulomanrique/t3code-import/releases/latest/download/t3-import-installer.ps1 | iex"
```

Or download a binary from [Releases](https://github.com/paulomanrique/t3code-import/releases), or build from source with `cargo install --git https://github.com/paulomanrique/t3code-import`.

## Reporting a database change

If t3-import stops with "This version of T3 Code changed its database", open the link it prints. It fills in an issue with your T3 version and the tables or fields that differ, without any of your conversations. Support for the new shape usually needs only an update to the known schema list.

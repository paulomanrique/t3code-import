# t3-import

Bring your existing Claude Code, Codex, Grok and OpenCode sessions into [T3 Code](https://github.com/pingdotgg/t3code), with their full history, so you can pick up where you left off instead of starting over.

T3 Code's built-in importer only covers Claude and Codex, only looks at the last 30 days, and imports every recent thread of a folder at once, capped at 100 threads and 200 messages each. t3-import lists every session on your machine, lets you choose which ones to bring in, and imports the complete conversation.

Imported threads resume the original session in its own CLI: T3 continues the same Claude, Codex, Grok or OpenCode conversation, not a copy.

## Use

```sh
t3-import
```

A window opens with three columns:

- **Folders.** Every folder where a session ran, as a tree. A folder that is a git worktree says which repository it belongs to and on which branch; its threads go into that repository's T3 project.
- **Threads.** The sessions of the selected folder and its subfolders, newest first. Tick the ones you want, or use **Select all** at the top, then **Add to T3** or **Delete**.
- **Conversation.** The selected thread as you'll see it in T3, plus where it ran and which project it goes to.

Threads already in T3 are hidden; turn on **Show threads already in T3** to see them. The buttons at the top show or hide each CLI, and the search box looks through titles, folders and session ids.

**Add to T3** puts each thread into the T3 project for the folder it ran in, creating missing projects with T3's own `t3 project add`. By default the whole history comes along; you can keep only the newest messages of each thread instead.

T3 has to be closed while threads are written. If it's running, the dialog shows what is open and offers **Close T3**: a background service (launchd on macOS, systemd on Linux) is stopped and started again when the import ends, and the app is closed. If you cancel after closing it, a bar at the top lets you start the service again.

**Delete** moves the selected sessions to the system Trash, where you can restore them. T3 isn't changed. OpenCode keeps sessions in its own database, so those are exported with `opencode export` first (bring one back with `opencode import <file>`) and then removed with `opencode session delete`.

### Command line

The same binary runs without a window when given a command, and `t3-import-cli` is a build without the window at all, for machines without a desktop:

```sh
t3-import-cli list --json
t3-import-cli check
t3-import-cli import claude:<session-id> codex:<session-id> --dry-run
t3-import-cli import claude:<session-id> --close-t3
```

`--close-t3` closes T3 the same way the window does and starts its service again afterwards.

## Safety

- **Backup first.** Before writing, t3-import copies `statev2.sqlite` to `statev2.sqlite.bak-t3import-<timestamp>` next to it. To undo an import, close T3 and copy the backup back.
- **Fail closed.** T3 Code changes quickly. Before writing anything, t3-import compares the tables it writes to, and the shape of the data already in them, with what it knows. If anything differs, it stops without touching the database and gives you a prefilled link to open an issue here, so support can be added.
- **One transaction.** All threads are written in a single transaction and re-checked before commit. Either everything lands or nothing does.
- **Your CLIs' sessions are only read when importing.** Only **Delete** changes them, and it moves files to the Trash rather than erasing them. Before moving anything it checks that the files are exactly the ones you confirmed; a session that changed in the meantime is left alone.

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

On Linux the window needs WebKitGTK (`libwebkit2gtk-4.1-0` on Debian and Ubuntu, `webkit2gtk4.1` on Fedora, `webkit2gtk-4.1` on Arch). `t3-import-cli` doesn't.

Or download a binary from [Releases](https://github.com/paulomanrique/t3code-import/releases), or build from source with `cargo install --git https://github.com/paulomanrique/t3code-import`.

## Reporting a database change

If t3-import stops with "This version of T3 Code changed its database", open the link it prints. It fills in an issue with your T3 version and the tables or fields that differ, without any of your conversations. Support for the new shape usually needs only an update to the known schema list.

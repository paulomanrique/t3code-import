// The bridge to the Rust side. Inside Tauri every call is an IPC command; in a
// plain browser (window.__TAURI__ missing) the mock answers, so the interface can
// be developed and checked without building the app.
//
// Contract (all fields camelCase, paths absolute, dates RFC 3339):
//
// scan() -> {
//   home: "/Users/me",
//   t3: { found, base, error, projects: [root], disabledProviders: [provider] },
//   sessions: [{
//     key: "claude:<sessionId>", provider: "claude"|"codex"|"grok"|"opencode",
//     sessionId, title, cwd, createdAt, updatedAt, messageCount|null, model|null,
//     source, inT3, importable, blocked: null|"in_t3"|"not_resumable"|"no_provider",
//     providerOff,
//     placement: { root, worktreePath|null, branch|null, worktreeMissing },
//   }],
//   pendingRestart: [name],   // services t3-import stopped and hasn't started again
// }
// transcript(key) -> { messages: [{ role: "user"|"assistant", text, createdAt }] }
// check() -> { ok, running: [{ name, plan, canClose }], findings: [{ location, expected, actual }],
//              warnings: [string], issueUrl|null }
// closeT3() -> check() result, taken after T3 is closed. Rejects with a message on failure.
// restartT3() -> { restarted: [{ ok, text }], pendingRestart: [name] }
// importSessions(keys, maxMessages|null) -> { imported: [threadId], skipped: [[threadId, why]],
//              projectsCreated: [root], backup|null, warnings: [string],
//              restarted: [{ ok, text }] }; progress arrives as onProgress(line).
//              Rejects with { message, restarted, backup|null, projectsCreated: [root] }
//              when no thread was written (projects created before the failure stay).
// trashPlan(keys) -> { items: [{ key, title, paths: [path], export: path|null }],
//              warnings: [string], blocked: null|string }
//              `export` is where an OpenCode session's `opencode export` file will go.
// trashSessions(keys) -> { trashed: [key], failed: [[key, error]], exported: [path] }
//              Paths are re-checked first: a session whose files changed since
//              trashPlan fails instead of being trashed.
// reveal(path), openUrl(url)
// Event "restart-failed" (payload: message): quitting couldn't start a stopped T3
// service again, so the app stays open to say so.

const tauri = window.__TAURI__;

function call(cmd, args) {
  return tauri.core.invoke(cmd, args);
}

export const inTauri = Boolean(tauri);

// Served by the app (tauri:// on macOS and Linux, http://tauri.localhost on
// Windows) but without the bridge: a build problem. Never fall back to the mock
// there, or the window would show made-up sessions.
export const bridgeMissing =
  !tauri && (location.protocol === "tauri:" || location.hostname === "tauri.localhost");

export async function onProgress(handler) {
  if (tauri) return tauri.event.listen("import-progress", (e) => handler(e.payload));
  return (await import("./mock.js")).onProgress(handler);
}

export async function onRestartFailed(handler) {
  if (tauri) return tauri.event.listen("restart-failed", (e) => handler(e.payload));
  return () => {};
}

const mock = () => import("./mock.js");

export const api = {
  scan: async () => (tauri ? call("scan") : (await mock()).scan()),
  transcript: async (key) => (tauri ? call("transcript", { key }) : (await mock()).transcript(key)),
  check: async () => (tauri ? call("check") : (await mock()).check()),
  closeT3: async () => (tauri ? call("close_t3") : (await mock()).closeT3()),
  restartT3: async () => (tauri ? call("restart_t3") : (await mock()).restartT3()),
  importSessions: async (keys, maxMessages) =>
    tauri ? call("import_sessions", { keys, maxMessages }) : (await mock()).importSessions(keys, maxMessages),
  trashPlan: async (keys) => (tauri ? call("trash_plan", { keys }) : (await mock()).trashPlan(keys)),
  trashSessions: async (keys) => (tauri ? call("trash_sessions", { keys }) : (await mock()).trashSessions(keys)),
  reveal: async (path) => (tauri ? call("reveal", { path }) : console.info("reveal", path)),
  openUrl: async (url) => (tauri ? call("open_url", { url }) : window.open(url, "_blank")),
};

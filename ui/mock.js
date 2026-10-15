// Sample data for running the interface in a plain browser. Not shipped behavior:
// api.js only loads this when there is no Tauri bridge.

const HOME = "/Users/dev";
const now = Date.now();
const ago = (minutes) => new Date(now - minutes * 60_000).toISOString();

const WEB = `${HOME}/Code/acme/web-dashboard`;
const TOOLS = `${HOME}/Code/acme/tools`;
const IMPORTER = `${HOME}/Code/t3code-import`;
const RECIPES = `${HOME}/Code/side/recipe-app`;
const WT_BADGES = `${HOME}/.t3/worktrees/web-dashboard/ACME-8107`;
const WT_GONE = `${HOME}/.t3/worktrees/web-dashboard/ACME-8090`;
const VOICE = `${HOME}/Documents/Codex/2026-10-09/realtime-voice-chat`;

const projects = [WEB, TOOLS, `${HOME}/Code/side/blog`];

const placement = (cwd) => {
  if (cwd === WT_BADGES) return { root: WEB, worktreePath: WT_BADGES, branch: "ACME-8107", worktreeMissing: false };
  if (cwd === WT_GONE) return { root: WEB, worktreePath: WT_GONE, branch: "ACME-8090", worktreeMissing: true };
  return { root: cwd, worktreePath: null, branch: null, worktreeMissing: false };
};

const rows = [
  ["claude", WEB, "Badge scheduler overwrites agent badges", 4, 172, "claude-opus-5-5"],
  ["claude", WEB, "Revenue per minute report", 21, 64, "claude-opus-5-5"],
  ["codex", WEB, "Fix flaky checkout test", 60 * 5, 38, "gpt-6.1-sol"],
  ["claude", WEB, "Agent module: role filters", 60 * 26, 210, "claude-opus-5-5", true],
  ["grok", WEB, "Review PR 1518: call notes on outbound dialer", 60 * 24 * 26, 12, "grok-4.7"],
  ["codex", WEB, "Migrate cron jobs to the queue", 60 * 24 * 40, 95, "gpt-6.1-sol", true],
  ["opencode", WEB, "Explain the tenant cache invalidation", 60 * 24 * 3, 7, "muse-spark-1.3"],
  ["claude", WT_BADGES, "ACME-8107 keep manual badges on rerun", 90, 48, "claude-opus-5-5"],
  ["codex", WT_BADGES, "Write the backfill for badge history", 60 * 3, 22, "gpt-6.1-sol"],
  ["claude", WT_GONE, "ACME-8090 dialer recording links", 60 * 24 * 9, 133, "claude-opus-5-5"],
  ["codex", TOOLS, "Script to restore zeroed scores", 60 * 24 * 2, 41, "gpt-6.1-sol"],
  ["grok", TOOLS, "Bulk rename of report columns", 60 * 24 * 5, 18, "grok-4.7"],
  ["claude", TOOLS, "Worker id backfill", 60 * 24 * 12, 77, "claude-opus-5-5", true],
  ["claude", IMPORTER, "Importer for CLI threads into T3 Code", 2, 812, "claude-opus-5-5"],
  ["codex", IMPORTER, "Fix review findings in src/t3db", 60 * 24 * 5, 56, "gpt-6.1-sol"],
  ["grok", IMPORTER, "Fix scan review findings in src/scan", 60 * 24 * 5, 31, "grok-4.7"],
  ["opencode", IMPORTER, "Review the t3db module", 60 * 24 * 5, 3, "muse-spark-1.3"],
  ["codex", RECIPES, "Offline sync for shopping lists", 60 * 24 * 60, 120, "gpt-6.1-sol"],
  ["claude", RECIPES, "Ingredient parser handles fractions", 60 * 24 * 75, 44, "claude-opus-5-5"],
  ["codex", RECIPES, "Untitled", 60 * 24 * 90, null, null, false, false],
  ["codex", HOME, "Install an SSH server on this machine", 60 * 24 * 100, 16, "gpt-6.1-sol"],
  ["claude", HOME, "Clean up old Homebrew casks", 60 * 24 * 20, 9, "claude-opus-5-5"],
  ["codex", VOICE, "Realtime voice chat prototype", 60 * 24 * 5, 61, "gpt-6.1-sol"],
  ["grok", "/private/tmp/scratch", "Benchmark JSON parsers", 60 * 24 * 7, 14, "grok-4.7"],
];

const sessions = rows.map(([provider, cwd, title, minutes, messageCount, model, inT3 = false, resumable = true], i) => {
  const sessionId = `${provider === "codex" ? "019a" : "6a2f"}${String(i).padStart(4, "0")}-ea9d-4f23-aada-17e0f4d2cfd9`;
  const providerOff = provider === "grok" || provider === "opencode";
  const blocked = inT3 ? "in_t3" : resumable ? null : "not_resumable";
  return {
    key: `${provider}:${sessionId}`,
    provider,
    sessionId,
    title,
    cwd,
    createdAt: ago(minutes + 600),
    updatedAt: ago(minutes),
    messageCount,
    model,
    source: `${HOME}/.${provider}/sessions/${sessionId}.jsonl`,
    inT3,
    importable: !blocked,
    blocked,
    providerOff,
    placement: placement(cwd),
  };
});

const CONVERSATION = [
  ["user", "The nightly badge job keeps wiping badges that supervisors set by hand. Can you find where that happens?"],
  [
    "assistant",
    "Found it. The scheduler rebuilds every agent's badge list from scratch in `BadgeScheduler::run`, so anything that didn't come from a rule disappears:\n\n```php\nforeach ($agents as $agent) {\n    $badges = $this->rules->evaluate($agent);\n    $this->store->replace($agent->id, $badges); // drops manual ones\n}\n```\n\nTwo options:\n\n1. Mark manual badges with `source = 'manual'` and have `replace` keep them.\n2. Only touch badges the rules own, and diff instead of replacing.\n\nOption 2 is safer: it never deletes a row it didn't create.",
  ],
  ["user", "Go with 2. Keep it to the scheduler, don't change the store API."],
  [
    "assistant",
    "Done. The scheduler now reads the current badges, keeps every badge whose `source` isn't `rule`, and only adds or removes rule badges.\n\n| Case | Before | After |\n|---|---|---|\n| Manual badge | removed | kept |\n| Rule badge, still earned | rewritten | untouched |\n| Rule badge, no longer earned | removed | removed |\n\nTests: `BadgeSchedulerTest` covers all three rows. Want me to open the PR?",
  ],
];

export async function scan() {
  await new Promise((r) => setTimeout(r, 400));
  return {
    home: HOME,
    t3: { found: true, base: `${HOME}/.t3`, error: null, projects, disabledProviders: ["grok", "opencode"] },
    sessions,
    pendingRestart: stopped ? ["T3 background service (com.t3code.daemon, launchd)"] : [],
  };
}

export async function transcript(key) {
  await new Promise((r) => setTimeout(r, 120));
  const s = sessions.find((x) => x.key === key);
  const n = Math.max(4, Math.min(s?.messageCount ?? 4, 40));
  const messages = [];
  for (let i = 0; i < n; i++) {
    const [role, text] = CONVERSATION[i % CONVERSATION.length];
    messages.push({ role, text, createdAt: ago(n - i) });
  }
  return { messages };
}

let t3Open = true;
let stopped = false;

export async function restartT3() {
  await new Promise((r) => setTimeout(r, 700));
  stopped = false;
  t3Open = true;
  return { restarted: [{ ok: true, text: "Started the T3 background service com.t3code.daemon again." }], pendingRestart: [] };
}

export async function check() {
  await new Promise((r) => setTimeout(r, 500));
  if (t3Open) {
    return {
      ok: false,
      running: [{ name: "T3 background service (com.t3code.daemon, launchd)", plan: "stopped, then started again after the import", canClose: true }],
      findings: [],
      warnings: [],
      issueUrl: null,
    };
  }
  return { ok: true, running: [], findings: [], warnings: [], issueUrl: null };
}

export async function closeT3() {
  await new Promise((r) => setTimeout(r, 1200));
  t3Open = false;
  stopped = true;
  return check();
}

let progressHandler = () => {};
export function onProgress(handler) {
  progressHandler = handler;
  return () => {};
}

export async function importSessions(keys) {
  const steps = ["Reading sessions", "Backing up the T3 database", "Creating project ~/Code/t3code-import", `Writing threads ${keys.length}/${keys.length}`, "Verifying what was written", "Starting the T3 background service again"];
  for (const s of steps) {
    progressHandler(s);
    await new Promise((r) => setTimeout(r, 450));
  }
  for (const k of keys) {
    const s = sessions.find((x) => x.key === k);
    if (s) {
      s.inT3 = true;
      s.importable = false;
      s.blocked = "in_t3";
    }
  }
  stopped = false;
  t3Open = true;
  return {
    imported: keys.map((k) => `import:${k}`),
    skipped: [],
    projectsCreated: [IMPORTER],
    backup: `${HOME}/.t3/userdata/statev2.sqlite.bak-t3import-20261014T231012Z`,
    warnings: [],
    restarted: [{ ok: true, text: "Started the T3 background service com.t3code.daemon again." }],
  };
}

export async function trashPlan(keys) {
  const items = keys.map((key) => {
    const s = sessions.find((x) => x.key === key);
    return s.provider === "opencode"
      ? { key, title: s.title, paths: [], export: `${HOME}/.local/share/t3-import/opencode-exports/${s.sessionId}.json` }
      : { key, title: s.title, paths: [s.source], export: null };
  });
  const warnings = [];
  const inT3 = keys.filter((k) => sessions.find((x) => x.key === k)?.inT3).length;
  if (inT3) warnings.push(`${inT3} of these are in T3. Continuing them there stops working once their files are gone.`);
  return { items, warnings, blocked: null };
}

export async function trashSessions(keys) {
  await new Promise((r) => setTimeout(r, 600));
  for (const k of keys) {
    const i = sessions.findIndex((x) => x.key === k);
    if (i >= 0) sessions.splice(i, 1);
  }
  return { trashed: keys, failed: [], exported: [] };
}

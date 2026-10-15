import { api, onProgress, onRestartFailed, bridgeMissing } from "./api.js";

const PROVIDERS = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "grok", label: "Grok" },
  { id: "opencode", label: "OpenCode" },
];
const LABEL = Object.fromEntries(PROVIDERS.map((p) => [p.id, p.label]));
const CHUNK = 200;
const CACHE = 30; // conversations kept in memory

const $ = (id) => document.getElementById(id);

// ?theme=light|dark pins a theme, for checking both without changing the OS setting.
{
  const theme = new URLSearchParams(location.search).get("theme");
  if (theme) document.documentElement.dataset.theme = theme;
}
const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

function stored(key, fallback) {
  try {
    const v = localStorage.getItem(`t3-import.${key}`);
    return v == null ? fallback : JSON.parse(v);
  } catch {
    return fallback;
  }
}
function store(key, value) {
  try {
    localStorage.setItem(`t3-import.${key}`, JSON.stringify(value));
  } catch {}
}

const state = {
  home: "",
  t3: null,
  sessions: [],
  byKey: new Map(),
  providers: new Set(stored("providers", PROVIDERS.map((p) => p.id))),
  showInT3: false,
  search: "",
  selected: new Set(),
  folder: null, // null = every folder
  // First run: top-level folders start open, so the tree isn't a single closed row.
  expanded: new Set(stored("expanded", null) ?? []),
  firstRun: stored("expanded", null) === null,
  focusKey: null,
  anchorKey: null,
  list: [],
  tree: null,
  transcripts: new Map(), // key -> { updatedAt, messages }, oldest first
  pendingRestart: [],
  readerGen: 0,
  shown: CHUNK,
};

// ----- paths and dates ---------------------------------------------------

const sepOf = (p) => (p.includes("\\") && !p.includes("/") ? "\\" : "/");

function tilde(path) {
  if (!path) return "";
  if (state.home && path === state.home) return "~";
  if (state.home && path.startsWith(state.home + sepOf(path))) return "~" + path.slice(state.home.length);
  return path;
}

function inside(path, folder) {
  if (path === folder) return true;
  const sep = sepOf(folder);
  // A root ("/", "C:\\") already ends with its separator.
  return path.startsWith(folder.endsWith(sep) ? folder : folder + sep);
}

/** Root ("/", "C:\\", "\\\\server\\share\\") and the folder names below it. */
function splitPath(p) {
  const sep = sepOf(p);
  let root = "";
  if (sep === "\\") {
    const m = p.match(/^(\\\\[^\\]+\\[^\\]+|[A-Za-z]:)\\?/);
    if (m) root = m[1] + "\\";
  } else if (p.startsWith("/")) {
    root = "/";
  }
  return { sep, root, parts: p.slice(root.length).split(sep).filter(Boolean) };
}

function ago(iso) {
  const at = new Date(iso);
  const mins = Math.max(0, Math.round((Date.now() - at) / 60000));
  if (mins < 2) return "just now";
  if (mins < 60) return `${mins}m ago`;
  if (mins < 60 * 24) return `${Math.floor(mins / 60)}h ago`;
  if (mins < 60 * 48) return "yesterday";
  if (mins < 60 * 24 * 7) return `${Math.floor(mins / 1440)}d ago`;
  const sameYear = at.getFullYear() === new Date().getFullYear();
  return at.toLocaleDateString(undefined, sameYear ? { month: "short", day: "numeric" } : { year: "numeric", month: "short", day: "numeric" });
}

const fullDate = (iso) => new Date(iso).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });

// ----- icons -------------------------------------------------------------

const ICON = {
  chevron: '<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M3.5 2l3 3-3 3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  folder: '<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><path d="M1.75 4.25c0-.69.56-1.25 1.25-1.25h3.1l1.4 1.5h5.5c.69 0 1.25.56 1.25 1.25v6.5c0 .69-.56 1.25-1.25 1.25H3c-.69 0-1.25-.56-1.25-1.25z" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>',
  project: '<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><path d="M1.75 4.25c0-.69.56-1.25 1.25-1.25h3.1l1.4 1.5h5.5c.69 0 1.25.56 1.25 1.25v6.5c0 .69-.56 1.25-1.25 1.25H3c-.69 0-1.25-.56-1.25-1.25z" fill="currentColor" fill-opacity=".18" stroke="currentColor" stroke-width="1.3"/></svg>',
  branch: '<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3"/><circle cx="4.5" cy="12.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3"/><circle cx="11.5" cy="5.5" r="1.6" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M4.5 5.1v5.8M11.5 7.1c0 2.4-2.2 3-5.4 3.6" fill="none" stroke="currentColor" stroke-width="1.3"/></svg>',
  all: '<svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 4h11M2.5 8h11M2.5 12h11" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>',
};

// ----- derived data --------------------------------------------------------

function matchesSearch(s) {
  if (!state.search) return true;
  const q = state.search.toLowerCase();
  return s.title.toLowerCase().includes(q) || s.cwd.toLowerCase().includes(q) || s.sessionId.toLowerCase().includes(q);
}

function visibleSessions() {
  return state.sessions.filter((s) => state.providers.has(s.provider) && (state.showInT3 || !s.inT3) && matchesSearch(s));
}

function isProject(path) {
  return state.t3?.projects.includes(path);
}

/** Folder tree of the sessions' working directories, Explorer style. */
function buildTree(sessions) {
  const nodes = new Map();
  const top = [];
  const node = (path, label, sep) => {
    let n = nodes.get(path);
    if (!n) {
      n = { path, label, sep, own: 0, total: 0, children: [] };
      nodes.set(path, n);
    }
    return n;
  };
  for (const s of sessions) {
    // [path, label] from the top-level row down to the session's own folder.
    const chain = [];
    const { sep, root, parts } = splitPath(s.cwd);
    if (state.home && inside(s.cwd, state.home)) {
      chain.push([state.home, "~"]);
      const below = splitPath(state.home).parts.length;
      let path = state.home;
      for (const part of parts.slice(below)) {
        path = path + sep + part;
        chain.push([path, part]);
      }
    } else {
      // The root is a row too; compaction folds it into "/private/tmp" or
      // "C:\\work" when it has a single child and no sessions of its own.
      let path = root || s.cwd;
      chain.push([path, path]);
      for (const part of parts) {
        path = path.endsWith(sep) ? path + part : path + sep + part;
        chain.push([path, part]);
      }
    }
    let parent = null;
    for (const [path, label] of chain) {
      const n = node(path, label, sep);
      n.total++;
      if (parent) {
        if (!parent.children.includes(n)) parent.children.push(n);
      } else if (!top.includes(n)) {
        top.push(n);
      }
      parent = n;
    }
    parent.own++;
  }
  // A folder with no sessions of its own and one subfolder shares its row.
  const compact = (n) => {
    while (n.own === 0 && n.children.length === 1) {
      const child = n.children[0];
      n = { ...child, label: `${n.label}${n.label.endsWith(n.sep) ? "" : n.sep}${child.label}` };
    }
    n.children = n.children.map(compact).sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: "base" }));
    return n;
  };
  const roots = top.map(compact).sort((a, b) => (a.label.startsWith("~") ? -1 : b.label.startsWith("~") ? 1 : a.label.localeCompare(b.label)));
  const rows = new Set();
  const walk = (n) => {
    rows.add(n.path);
    n.children.forEach(walk);
  };
  roots.forEach(walk);
  const worktrees = new Map();
  for (const s of state.sessions) {
    const p = s.placement;
    if (p.worktreePath) worktrees.set(p.worktreePath, p);
  }
  return { roots, worktrees, rows };
}

/** Keeps the selected folder pointing at a row that exists after a rescan or a
 * filter change: the row that absorbed it, else its nearest surviving parent. */
function reconcileFolder() {
  const rows = state.tree.rows;
  if (!state.folder || rows.has(state.folder)) return;
  const absorbed = [...rows].filter((r) => inside(r, state.folder)).sort((a, b) => a.length - b.length)[0];
  if (absorbed && state.list.length) {
    state.folder = absorbed;
    return;
  }
  const parent = [...rows].filter((r) => inside(state.folder, r)).sort((a, b) => b.length - a.length)[0];
  state.folder = parent ?? null;
}

function listFor(sessions) {
  const list = state.folder ? sessions.filter((s) => inside(s.cwd, state.folder)) : sessions;
  return list.slice().sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
}

// ----- rendering -----------------------------------------------------------

function render() {
  const visible = visibleSessions();
  state.tree = buildTree(visible);
  if (state.firstRun && state.tree.roots.length) {
    state.tree.roots.forEach((r) => state.expanded.add(r.path));
    state.firstRun = false;
  }
  state.list = listFor(visible);
  reconcileFolder();
  state.list = listFor(visible);
  if (state.focusKey && !state.list.some((s) => s.key === state.focusKey)) state.focusKey = state.list[0]?.key ?? null;
  if (!state.focusKey && state.list.length) state.focusKey = state.list[0].key;
  renderProviders();
  renderTree();
  renderFolderHead();
  renderList();
  renderTools();
  renderReader();
}

function renderProviders() {
  const focused = document.activeElement?.dataset?.provider;
  const counts = {};
  for (const s of state.sessions) {
    if ((state.showInT3 || !s.inT3) && matchesSearch(s)) counts[s.provider] = (counts[s.provider] ?? 0) + 1;
  }
  $("providers").innerHTML = PROVIDERS.map(
    (p) => `<button type="button" class="prov-toggle" data-provider="${p.id}" aria-pressed="${state.providers.has(p.id)}" style="--c: var(--${p.id})"
      title="${state.providers.has(p.id) ? "Hide" : "Show"} ${p.label} sessions"><span class="dot"></span>${p.label}<span class="n">${counts[p.id] ?? 0}</span></button>`,
  ).join("");
  if (focused) document.querySelector(`[data-provider="${focused}"]`)?.focus();
}

function renderTree() {
  const { roots, worktrees } = state.tree;
  const total = roots.reduce((n, r) => n + r.total, 0);
  const row = (n, depth) => {
    const open = state.expanded.has(n.path);
    const leaf = n.children.length === 0;
    const wt = worktrees.get(n.path);
    const icon = wt ? `<span class="icon worktree" title="Git worktree">${ICON.branch}</span>`
      : isProject(n.path) ? `<span class="icon project" title="T3 project">${ICON.project}</span>`
      : `<span class="icon">${ICON.folder}</span>`;
    let html = `<li><button type="button" class="node" style="--depth:${depth}" data-path="${esc(n.path)}" aria-current="${state.folder === n.path}"
      ${leaf ? "" : `aria-expanded="${open}"`} title="${esc(tilde(n.path))}">
      <span class="twisty ${leaf ? "leaf" : ""} ${open ? "open" : ""}" data-twisty>${ICON.chevron}</span>${icon}
      <span class="label">${esc(n.label)}</span><span class="count">${n.total}</span></button>`;
    if (!leaf && open) html += `<ul>${n.children.map((c) => row(c, depth + 1)).join("")}</ul>`;
    return html + "</li>";
  };
  $("tree").innerHTML = total === 0 && !state.sessions.length
    ? `<p class="tree-empty">No sessions found.</p>`
    : `<button type="button" class="node all" data-path="" aria-current="${state.folder === null}">
         <span class="twisty leaf"></span><span class="icon">${ICON.all}</span><span class="label">All folders</span><span class="count">${total}</span></button>
       <ul>${roots.map((r) => row(r, 0)).join("")}</ul>`;
}

function renderFolderHead() {
  const el = $("folder-head");
  if (!state.folder) {
    el.innerHTML = `<h2>All folders</h2><p class="status">${plural(state.list.length, "thread", "threads")}, newest first.</p>`;
    return;
  }
  const wt = state.tree.worktrees.get(state.folder);
  let status = "";
  if (wt) {
    status = wt.worktreeMissing
      ? `<div class="worktree-note">${ICON.branch}<div>Git worktree of <code>${esc(tilde(wt.root))}</code>${wt.branch ? ` on branch <code>${esc(wt.branch)}</code>` : ""}. It has been deleted: its threads still go into that project, and continuing them runs in the project folder.</div></div>`
      : `<div class="worktree-note">${ICON.branch}<div>Git worktree of <code>${esc(tilde(wt.root))}</code>${wt.branch ? ` on branch <code>${esc(wt.branch)}</code>` : ""}. Its threads go into that project and keep the worktree and branch.</div></div>`;
  } else if (isProject(state.folder)) {
    status = `<p class="status project">T3 project</p>`;
  } else if (state.list.some((s) => s.placement.root === state.folder && s.importable)) {
    status = `<p class="status">Not a T3 project yet. Adding a thread from here creates it.</p>`;
  }
  el.innerHTML = `<h2>${esc(tilde(state.folder))}</h2>${status}`;
}

function rowTag(s) {
  if (s.inT3) return `<span class="tag in-t3">In T3</span>`;
  if (s.blocked === "not_resumable") return `<span class="tag blocked" title="This session can't be continued, so it can't be added to T3">Can't continue</span>`;
  if (s.blocked === "no_provider") return `<span class="tag blocked" title="T3 has no ${LABEL[s.provider]} provider set up">No ${LABEL[s.provider]} in T3</span>`;
  if (s.providerOff) return `<span class="tag off" title="Adds fine; continue it after turning ${LABEL[s.provider]} on in T3's settings">${LABEL[s.provider]} off in T3</span>`;
  return "";
}

function renderList() {
  const ul = $("threads");
  if (!state.list.length) {
    const why = !state.sessions.length ? "No Claude, Codex, Grok or OpenCode sessions found on this computer."
      : state.search ? `Nothing matches “${esc(state.search)}”.`
      : !state.showInT3 && state.sessions.some((s) => s.inT3 && (!state.folder || inside(s.cwd, state.folder))) ? "Every thread here is already in T3. Turn on “Show threads already in T3” to see them."
      : "No threads here. Check which CLIs are turned on above.";
    ul.innerHTML = `<li class="list-empty">${why}</li>`;
    return;
  }
  ul.innerHTML = state.list
    .map((s, i) => {
      const checked = state.selected.has(s.key);
      const where = state.folder ? (s.cwd === state.folder ? "" : s.cwd.slice(state.folder.length + 1)) : tilde(s.cwd);
      const count = s.messageCount != null ? `<span>${plural(s.messageCount, "message", "messages")}</span>` : "";
      return `<li class="row ${checked ? "checked" : ""} ${s.key === state.focusKey ? "focused" : ""} ${s.importable ? "" : "dim"}"
        id="row-${i}" data-key="${esc(s.key)}" role="option" aria-selected="${checked}">
        <input type="checkbox" tabindex="-1" ${checked ? "checked" : ""} aria-label="Select ${esc(s.title)}">
        <div class="row-title" title="${esc(s.title)}">${esc(s.title)}</div>
        <span class="row-when" title="${esc(fullDate(s.updatedAt))}">${ago(s.updatedAt)}</span>
        <div class="row-sub"><span class="prov" style="--c: var(--${s.provider})">${LABEL[s.provider]}</span>${rowTag(s)}${count}
          ${where ? `<span class="where" title="${esc(s.cwd)}">${esc(where)}</span>` : ""}</div>
      </li>`;
    })
    .join("");
  const active = state.list.findIndex((s) => s.key === state.focusKey);
  if (active >= 0) ul.setAttribute("aria-activedescendant", `row-${active}`);
  else ul.removeAttribute("aria-activedescendant");
}

function renderTools() {
  const keys = state.list.map((s) => s.key);
  const picked = keys.filter((k) => state.selected.has(k)).length;
  const all = $("check-all");
  all.checked = keys.length > 0 && picked === keys.length;
  all.indeterminate = picked > 0 && picked < keys.length;
  all.disabled = keys.length === 0;
  $("check-all-label").textContent = picked ? `${picked} of ${keys.length}` : `Select all ${keys.length}`;

  const elsewhere = state.selected.size - picked;
  $("selection-note").innerHTML = elsewhere > 0
    ? `+${elsewhere} in other folders <button type="button" id="clear-selection">Clear</button>`
    : state.selected.size ? `<button type="button" id="clear-selection">Clear</button>` : "";

  const importable = [...state.selected].filter((k) => state.byKey.get(k)?.importable).length;
  const add = $("act-import");
  add.textContent = importable ? `Add ${importable} to T3` : "Add to T3";
  add.disabled = importable === 0 || !state.t3?.found;
  const del = $("act-trash");
  del.textContent = state.selected.size ? `Delete ${state.selected.size}` : "Delete";
  del.disabled = state.selected.size === 0;
}

async function renderReader() {
  const el = $("reader");
  const s = state.byKey.get(state.focusKey);
  if (!s) {
    el.innerHTML = `<p class="reader-empty">${state.sessions.length ? "Pick a thread to read it here." : ""}</p>`;
    el.dataset.key = "";
    return;
  }
  if (el.dataset.key === s.key && el.dataset.version === s.updatedAt) return; // already showing it
  el.dataset.key = s.key;
  el.dataset.version = s.updatedAt;
  state.shown = CHUNK;
  // Only the newest request may render or cache: an older one finishing last
  // must not put stale messages under newer metadata.
  const gen = ++state.readerGen;

  const p = s.placement;
  const facts = [
    ["From", `<span style="color: var(--${s.provider}); font-weight: 600">${LABEL[s.provider]}</span>${s.model ? `, ${esc(s.model)}` : ""}`],
    ["Last used", esc(fullDate(s.updatedAt))],
    ["Started", esc(fullDate(s.createdAt))],
    ["Ran in", esc(tilde(s.cwd))],
    ["Goes to", `${esc(tilde(p.root))}${isProject(p.root) ? "" : " <span class=\"muted\">(new T3 project)</span>"}`],
  ];
  if (p.worktreePath) facts.push(["Branch", esc(p.branch ?? "none (detached)")]);
  facts.push(["Session", `<code>${esc(s.sessionId)}</code>`]);

  let note = "";
  if (s.inT3) note = `<p class="reader-note">Already in T3.</p>`;
  else if (s.blocked === "not_resumable") note = `<p class="reader-note">This session can't be continued, so it can't be added to T3.</p>`;
  else if (s.blocked === "no_provider") note = `<p class="reader-note">T3 has no ${LABEL[s.provider]} provider set up, so this can't be added.</p>`;
  else if (s.providerOff) note = `<p class="reader-note warn">${LABEL[s.provider]} is turned off in T3. The thread adds with its full history; continue it after turning ${LABEL[s.provider]} on in T3's settings.</p>`;
  if (p.worktreeMissing) note += `<p class="reader-note warn">Its worktree has been deleted, so continuing it runs in the project folder.</p>`;

  el.innerHTML = `<div class="reader-head">
      <h1>${esc(s.title)}</h1>
      <dl class="facts">${facts.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl>
      ${note}
      <div class="reader-actions"><button type="button" class="btn" data-reveal="${esc(s.source)}">Show file</button></div>
    </div>
    <div class="messages" id="messages"><p class="loading"><span class="spinner"></span>Loading the conversation…</p></div>`;

  let transcript = state.transcripts.get(s.key);
  if (transcript && transcript.updatedAt !== s.updatedAt) transcript = null; // the session moved on
  if (!transcript) {
    let loaded;
    try {
      loaded = await api.transcript(s.key);
    } catch (e) {
      if (gen === state.readerGen) $("messages").innerHTML = `<p class="reader-empty">Couldn't read this session: ${esc(errorText(e))}</p>`;
      return;
    }
    if (gen !== state.readerGen) return;
    transcript = { updatedAt: s.updatedAt, ...loaded };
    state.transcripts.delete(s.key);
    state.transcripts.set(s.key, transcript);
    while (state.transcripts.size > CACHE) state.transcripts.delete(state.transcripts.keys().next().value);
  }
  if (gen === state.readerGen) renderMessages(s, transcript.messages);
}

function renderMessages(s, messages) {
  const box = $("messages");
  if (!messages.length) {
    box.innerHTML = `<p class="reader-empty">This session has no messages.</p>`;
    return;
  }
  const start = Math.max(0, messages.length - state.shown);
  const earlier = start > 0
    ? `<button type="button" class="btn load-earlier" id="load-earlier">Show ${Math.min(CHUNK, start)} earlier messages (${start} hidden)</button>`
    : "";
  box.innerHTML = earlier + messages
    .slice(start)
    .map((m) => `<article class="msg ${m.role}" style="--c: var(--${s.provider})">
        <div class="msg-who">${m.role === "user" ? "You" : LABEL[s.provider]}</div>
        <div class="msg-body">${markdown(m.text)}</div></article>`)
    .join("");
  if (start === Math.max(0, messages.length - CHUNK)) box.scrollTop = box.scrollHeight;
}

// Conversations are untrusted text: only what Markdown produces gets through.
// No forms, inputs, images, SVG, ids or names: nothing that could navigate the
// window, load remote content, or be mistaken for one of this page's controls.
const PURIFY = {
  ALLOWED_TAGS: ["p", "br", "hr", "strong", "b", "em", "i", "del", "s", "code", "pre", "blockquote", "ul", "ol", "li",
    "a", "h1", "h2", "h3", "h4", "h5", "h6", "table", "thead", "tbody", "tr", "th", "td", "span", "kbd", "sup", "sub"],
  ALLOWED_ATTR: ["href", "title", "start", "align"],
  ALLOWED_URI_REGEXP: /^https?:/i,
};

function markdown(text) {
  try {
    return DOMPurify.sanitize(marked.parse(text, { breaks: true, gfm: true }), PURIFY);
  } catch {
    return `<p>${esc(text)}</p>`;
  }
}

function errorText(e) {
  return typeof e === "string" ? e : e?.message ?? JSON.stringify(e);
}

// ----- selection ------------------------------------------------------------

function toggle(key, on = !state.selected.has(key)) {
  if (on) state.selected.add(key);
  else state.selected.delete(key);
}

function selectRange(from, to) {
  const keys = state.list.map((s) => s.key);
  const a = keys.indexOf(from);
  const b = keys.indexOf(to);
  if (a < 0 || b < 0) return toggle(to);
  const on = !state.selected.has(to);
  for (const k of keys.slice(Math.min(a, b), Math.max(a, b) + 1)) toggle(k, on);
}

function focusRow(key, { scroll = true } = {}) {
  state.focusKey = key;
  renderList();
  renderReader();
  if (scroll) document.querySelector(`.row[data-key="${CSS.escape(key)}"]`)?.scrollIntoView({ block: "nearest" });
}

// ----- events -----------------------------------------------------------------

$("providers").addEventListener("click", (e) => {
  const b = e.target.closest("[data-provider]");
  if (!b) return;
  const p = b.dataset.provider;
  if (state.providers.has(p)) state.providers.delete(p);
  else state.providers.add(p);
  store("providers", [...state.providers]);
  render();
});

$("show-in-t3").addEventListener("change", (e) => {
  state.showInT3 = e.target.checked;
  render();
});

$("search").addEventListener("input", (e) => {
  state.search = e.target.value.trim();
  render();
});

$("tree").addEventListener("click", (e) => {
  const b = e.target.closest(".node");
  if (!b) return;
  const path = b.dataset.path || null;
  const hasChildren = b.hasAttribute("aria-expanded");
  if (e.target.closest("[data-twisty]") && hasChildren) {
    if (state.expanded.has(path)) state.expanded.delete(path);
    else state.expanded.add(path);
  } else {
    state.folder = path;
    if (path && hasChildren) state.expanded.add(path);
    state.focusKey = null;
  }
  store("expanded", [...state.expanded]);
  render();
  document.querySelector(`.node[data-path="${CSS.escape(path ?? "")}"]`)?.focus();
});

$("tree").addEventListener("keydown", (e) => {
  const b = e.target.closest(".node");
  if (!b || !b.dataset.path || !b.hasAttribute("aria-expanded")) return;
  const open = e.key === "ArrowRight" ? true : e.key === "ArrowLeft" ? false : null;
  if (open === null) return;
  e.preventDefault();
  if (open) state.expanded.add(b.dataset.path);
  else state.expanded.delete(b.dataset.path);
  store("expanded", [...state.expanded]);
  renderTree();
  document.querySelector(`.node[data-path="${CSS.escape(b.dataset.path)}"]`)?.focus();
});

$("threads").addEventListener("click", (e) => {
  const row = e.target.closest(".row");
  if (!row) return;
  const key = row.dataset.key;
  const onBox = e.target.matches("input[type=checkbox]");
  if (e.shiftKey && state.anchorKey) {
    e.preventDefault();
    selectRange(state.anchorKey, key);
  } else if (onBox || e.metaKey || e.ctrlKey) {
    toggle(key);
    state.anchorKey = key;
  } else {
    state.anchorKey = key;
  }
  focusRow(key, { scroll: false });
  renderTools();
  $("threads").focus({ preventScroll: true });
});

$("threads").addEventListener("keydown", (e) => {
  const keys = state.list.map((s) => s.key);
  const i = keys.indexOf(state.focusKey);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    const next = keys[Math.min(keys.length - 1, Math.max(0, i + (e.key === "ArrowDown" ? 1 : -1)))];
    if (next) {
      if (e.shiftKey) toggle(next, true);
      focusRow(next);
      renderTools();
    }
  } else if (e.key === " " && state.focusKey) {
    e.preventDefault();
    toggle(state.focusKey);
    state.anchorKey = state.focusKey;
    renderList();
    renderTools();
  } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
    e.preventDefault();
    keys.forEach((k) => toggle(k, true));
    renderList();
    renderTools();
  }
});

$("check-all").addEventListener("change", () => {
  const keys = state.list.map((s) => s.key);
  const allOn = keys.every((k) => state.selected.has(k));
  keys.forEach((k) => toggle(k, !allOn));
  renderList();
  renderTools();
});

document.querySelector(".list-tools").addEventListener("click", (e) => {
  if (e.target.id === "clear-selection") {
    state.selected.clear();
    renderList();
    renderTools();
  }
});

$("reader").addEventListener("click", (e) => {
  const reveal = e.target.closest("[data-reveal]");
  if (reveal) return void api.reveal(reveal.dataset.reveal);
  if (e.target.id === "load-earlier") {
    state.shown += CHUNK;
    const s = state.byKey.get(state.focusKey);
    const box = $("messages");
    const before = box.scrollHeight - box.scrollTop;
    renderMessages(s, state.transcripts.get(s.key).messages);
    box.scrollTop = box.scrollHeight - before;
    return;
  }
  // Links in a conversation open in the browser, never inside this window.
  const a = e.target.closest(".msg-body a[href]");
  if (a) {
    e.preventDefault();
    if (/^https?:/i.test(a.href)) api.openUrl(a.href);
  }
});

$("act-import").addEventListener("click", openImport);
$("act-trash").addEventListener("click", openTrash);

// Resizable columns, by dragging or with the arrow keys on a focused divider.
const WIDTH = { tree: ["--tree-w", 170, 520], list: ["--list-w", 260, 760] };
function setWidth(which, px) {
  const [name, min, max] = WIDTH[which];
  const panes = $("panes");
  panes.style.setProperty(name, `${Math.round(Math.min(max, Math.max(min, px)))}px`);
  store("widths", { tree: panes.style.getPropertyValue("--tree-w"), list: panes.style.getPropertyValue("--list-w") });
}
for (const bar of document.querySelectorAll(".splitter")) {
  bar.tabIndex = 0;
  bar.setAttribute("aria-label", bar.dataset.split === "tree" ? "Resize the folder column" : "Resize the thread column");
  bar.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const [name] = WIDTH[bar.dataset.split];
    const now = parseFloat(getComputedStyle($("panes")).getPropertyValue(name));
    setWidth(bar.dataset.split, now + (e.key === "ArrowRight" ? 16 : -16));
  });
  bar.addEventListener("pointerdown", (e) => {
    const which = bar.dataset.split;
    const panes = $("panes");
    const startX = e.clientX;
    const start = parseFloat(getComputedStyle(panes).getPropertyValue(which === "tree" ? "--tree-w" : "--list-w"));
    bar.setPointerCapture(e.pointerId);
    bar.classList.add("dragging");
    const move = (ev) => {
      const w = Math.round(Math.min(which === "tree" ? 520 : 760, Math.max(which === "tree" ? 170 : 260, start + ev.clientX - startX)));
      panes.style.setProperty(which === "tree" ? "--tree-w" : "--list-w", `${w}px`);
    };
    const up = () => {
      bar.classList.remove("dragging");
      bar.removeEventListener("pointermove", move);
      store("widths", { tree: panes.style.getPropertyValue("--tree-w"), list: panes.style.getPropertyValue("--list-w") });
    };
    bar.addEventListener("pointermove", move);
    bar.addEventListener("pointerup", up, { once: true });
  });
}
{
  const w = stored("widths", null);
  if (w?.tree) $("panes").style.setProperty("--tree-w", w.tree);
  if (w?.list) $("panes").style.setProperty("--list-w", w.list);
}

// ----- dialogs ------------------------------------------------------------------

const dialog = $("dialog");
dialog.setAttribute("aria-labelledby", "dlg-title");
// Controls are looked up inside the dialog only, so nothing else on the page
// (a conversation, say) can stand in for them.
const q = (id) => dialog.querySelector(`#${id}`);
let busy = false;
// Each dialog opening gets a number; work that finishes after its dialog was
// closed or replaced checks it and stops instead of touching the new one.
let dialogGen = 0;
const current = (token) => token === dialogGen && dialog.open;

dialog.addEventListener("cancel", (e) => {
  if (busy) e.preventDefault();
});
dialog.addEventListener("close", () => {
  dialogGen++;
  rescanQuietly();
});
function showDialog(html) {
  dialog.innerHTML = `<div class="dlg">${html}</div>`;
  dialog.querySelector("h2")?.setAttribute("id", "dlg-title");
  if (!dialog.open) dialog.showModal();
}
function closeDialog() {
  if (!busy) dialog.close();
}

// After a dialog, refresh what the backend knows (a service it stopped) without
// disturbing the current view.
async function rescanQuietly() {
  try {
    const data = await api.scan();
    state.pendingRestart = data.pendingRestart ?? [];
    renderBanner();
  } catch {}
}

function countBy(keys) {
  const by = {};
  for (const k of keys) {
    const p = state.byKey.get(k)?.provider;
    if (p) by[p] = (by[p] ?? 0) + 1;
  }
  return PROVIDERS.filter((p) => by[p.id]).map((p) => `${by[p.id]} ${p.label}`).join(", ");
}

function savedLimit() {
  const v = stored("limit", null);
  const n = Number.parseInt(v?.n, 10);
  return { all: v?.all !== false, n: Number.isFinite(n) ? Math.min(100000, Math.max(1, n)) : 200 };
}

function openImport() {
  const keys = [...state.selected].filter((k) => state.byKey.get(k)?.importable);
  if (!keys.length) return;
  const skipped = state.selected.size - keys.length;
  const roots = [...new Set(keys.map((k) => state.byKey.get(k).placement.root))].filter((r) => !isProject(r)).sort();
  const off = PROVIDERS.filter((p) => keys.some((k) => state.byKey.get(k).provider === p.id && state.byKey.get(k).providerOff));
  const limit = savedLimit();

  showDialog(`<div class="dlg-body">
      <h2>Add ${plural(keys.length, "thread", "threads")} to T3</h2>
      <dl class="summary">
        <dt>Threads</dt><dd>${countBy(keys)}</dd>
        <dt>Projects</dt><dd>${roots.length ? `${plural(roots.length, "new project", "new projects")}: ${roots.map((r) => esc(tilde(r))).join(", ")}` : "All already in T3"}</dd>
      </dl>
      <div class="limit">
        <label><input type="radio" name="limit" value="all" ${limit.all ? "checked" : ""}> Full history</label>
        <label><input type="radio" name="limit" value="n" ${limit.all ? "" : "checked"}> Only the newest <input type="number" id="limit-n" min="1" max="100000" value="${limit.n}"> messages of each thread</label>
      </div>
      ${skipped ? `<p class="muted">${plural(skipped, "selected thread isn't", "selected threads aren't")} added: already in T3 or can't be continued.</p>` : ""}
      ${off.length ? `<div class="callout warn">${off.map((p) => p.label).join(" and ")} ${off.length === 1 ? "is" : "are"} turned off in T3. Those threads add with their full history; continue them after turning ${off.length === 1 ? "it" : "them"} on in T3's settings.</div>` : ""}
      <div id="check-area"><p class="muted"><span class="spinner"></span>Checking T3…</p></div>
    </div>
    <div class="dlg-foot">
      <button type="button" class="btn left" data-close>Cancel</button>
      <button type="button" class="btn" id="close-t3" hidden>Close T3</button>
      <button type="button" class="btn" id="recheck" hidden>Check again</button>
      <button type="button" class="btn primary" id="do-import" disabled>Add to T3</button>
    </div>`);
  const token = ++dialogGen;

  const runCheck = async (fn = api.check) => {
    q("do-import").disabled = true;
    q("close-t3").hidden = true;
    q("recheck").hidden = true;
    let report;
    try {
      report = await fn();
    } catch (e) {
      if (!current(token)) return;
      q("check-area").innerHTML = `<div class="callout error"><strong>${fn === api.closeT3 ? "Couldn't close T3" : "Couldn't check T3"}</strong>${esc(errorText(e))}</div>`;
      q("recheck").hidden = false;
      return;
    }
    if (current(token)) showCheck(report);
  };

  dialog.onclick = async (e) => {
    if (e.target.closest("[data-close]")) return closeDialog();
    if (e.target.closest("[data-issue]")) return api.openUrl(e.target.closest("[data-issue]").dataset.issue);
    if (e.target.id === "recheck") {
      q("check-area").innerHTML = `<p class="muted"><span class="spinner"></span>Checking T3…</p>`;
      return runCheck();
    }
    if (e.target.id === "close-t3") {
      busy = true;
      try {
        q("check-area").innerHTML = `<p class="muted"><span class="spinner"></span>Closing T3…</p>`;
        await runCheck(api.closeT3);
      } finally {
        busy = false;
      }
      return;
    }
    if (e.target.id === "do-import") {
      const all = dialog.querySelector('input[name="limit"]:checked').value === "all";
      const n = Math.min(100000, Math.max(1, Number.parseInt(q("limit-n").value, 10) || 200));
      store("limit", { all, n });
      runImport(keys, all ? null : n);
    }
  };
  runCheck();
}

function showCheck(report) {
  let html = "";
  if (report.findings.length) {
    html += `<div class="callout error"><strong>This version of T3 Code changed its database</strong>
      t3-import doesn't know this shape yet, so it won't write anything.
      <ul>${report.findings.slice(0, 5).map((f) => `<li>${esc(f.location)}</li>`).join("")}</ul></div>`;
    if (report.issueUrl) html += `<p><button type="button" class="btn" data-issue="${esc(report.issueUrl)}">Report it on GitHub</button> <span class="muted">The report has your T3 version and the tables that differ, none of your conversations.</span></p>`;
  }
  if (report.running.length) {
    const closable = report.running.every((r) => r.canClose);
    html += `<div class="callout warn"><strong>T3 Code is still running</strong>
      It uses the same database, so it has to be closed while the threads are written.
      <ul>${report.running.map((r) => `<li>${esc(r.name)}: ${esc(r.plan)}</li>`).join("")}</ul></div>`;
    q("close-t3").hidden = !closable || report.findings.length > 0;
    q("recheck").hidden = false;
  }
  if (report.ok) {
    html += `<div class="callout ok"><strong>Ready</strong>T3 is closed and its database checks out. A backup is taken before anything is written.</div>`;
    q("do-import").disabled = false;
  }
  if (report.warnings.length) html += `<ul class="muted">${report.warnings.map((w) => `<li>${esc(w)}</li>`).join("")}</ul>`;
  q("check-area").innerHTML = html;
}

function doneDialog(body) {
  showDialog(`<div class="dlg-body">${body}</div><div class="dlg-foot"><button type="button" class="btn primary" data-close>Done</button></div>`);
  dialog.onclick = (e) => e.target.closest("[data-close]") && closeDialog();
}

async function runImport(keys, maxMessages) {
  busy = true;
  let body;
  let stop;
  try {
    showDialog(`<div class="dlg-body"><h2>Adding ${plural(keys.length, "thread", "threads")} to T3</h2>
      <ul class="progress" id="progress"><li><span class="spinner"></span>Starting</li></ul>
      <p class="muted">Keep T3 Code closed until this finishes.</p></div>`);
    dialog.onclick = null;
    const lines = [];
    // Listen before invoking, so the first progress lines aren't missed.
    stop = await onProgress((line) => {
      lines.push(line);
      const list = q("progress");
      if (list) list.innerHTML = lines.map((l, i) => `<li>${i === lines.length - 1 ? '<span class="spinner"></span>' : ""}${esc(l)}</li>`).join("");
    });
    const r = await api.importSessions(keys, maxMessages);
    body = `<h2>Added ${plural(r.imported.length, "thread", "threads")} to T3</h2>
      <dl class="summary">
        ${r.projectsCreated.length ? `<dt>New projects</dt><dd>${r.projectsCreated.map((p) => esc(tilde(p))).join("<br>")}</dd>` : ""}
        ${r.skipped.length ? `<dt>Skipped</dt><dd>${r.skipped.map(([id, why]) => `${esc(id)}: ${esc(why)}`).join("<br>")}</dd>` : ""}
        ${r.backup ? `<dt>Backup</dt><dd><code>${esc(tilde(r.backup))}</code></dd>` : ""}
      </dl>
      ${restartedHtml(r.restarted)}
      ${r.warnings.length ? `<ul class="muted">${r.warnings.map((w) => `<li>${esc(w)}</li>`).join("")}</ul>` : ""}
      <p>Open T3 Code to see them. To undo, close T3 and copy the backup over <code>statev2.sqlite</code>.</p>`;
    for (const k of keys) state.selected.delete(k);
  } catch (e) {
    const created = e?.projectsCreated ?? [];
    body = `<h2>No threads were added</h2><div class="callout error">${esc(errorText(e))}</div>
      <dl class="summary">
        ${created.length ? `<dt>Projects</dt><dd>${created.map((p) => esc(tilde(p))).join("<br>")} <span class="muted">(created before the failure, still in T3)</span></dd>` : ""}
        ${e?.backup ? `<dt>Backup</dt><dd><code>${esc(tilde(e.backup))}</code></dd>` : ""}
      </dl>${restartedHtml(e?.restarted)}`;
  } finally {
    try {
      stop?.();
    } catch {}
    busy = false;
  }
  doneDialog(body);
  await rescan();
}

function restartedHtml(lines) {
  if (!lines?.length) return "";
  return lines.map((l) => `<div class="callout ${l.ok ? "ok" : "error"}">${esc(l.text)}</div>`).join("");
}

async function openTrash() {
  const keys = [...state.selected];
  if (!keys.length) return;
  showDialog(`<div class="dlg-body"><h2>Move ${plural(keys.length, "session", "sessions")} to the Trash</h2>
    <p class="muted"><span class="spinner"></span>Finding their files…</p></div>
    <div class="dlg-foot"><button type="button" class="btn left" data-close>Cancel</button></div>`);
  const token = ++dialogGen;
  dialog.onclick = (e) => e.target.closest("[data-close]") && closeDialog();
  let plan;
  try {
    plan = await api.trashPlan(keys);
  } catch (e) {
    if (!current(token)) return;
    showDialog(`<div class="dlg-body"><h2>Can't delete these</h2><div class="callout error">${esc(errorText(e))}</div></div>
      <div class="dlg-foot"><button type="button" class="btn" data-close>Close</button></div>`);
    return;
  }
  if (!current(token)) return;
  const paths = plan.items.flatMap((i) => i.paths.map(tilde));
  const exports = plan.items.filter((i) => i.export);
  showDialog(`<div class="dlg-body">
      <h2>Move ${plural(keys.length, "session", "sessions")} to the Trash</h2>
      <p>${countBy(keys)}. You can restore files from the Trash. T3 isn't changed.</p>
      <ul class="trash-list">${plan.items.map((i) => `<li>${esc(i.title)}</li>`).join("")}</ul>
      ${exports.length ? `<p class="muted">OpenCode keeps sessions in its own database. ${plural(exports.length, "session is", "sessions are")} exported to a file with OpenCode first, then removed with OpenCode. Bring one back with <code>opencode import &lt;file&gt;</code>.</p>` : ""}
      ${plan.blocked ? `<div class="callout error">${esc(plan.blocked)}</div>` : ""}
      ${plan.warnings.map((w) => `<div class="callout warn">${esc(w)}</div>`).join("")}
      ${paths.length ? `<pre class="paths">${esc(paths.join("\n"))}</pre>` : ""}
    </div>
    <div class="dlg-foot">
      <button type="button" class="btn left" data-close>Cancel</button>
      <button type="button" class="btn danger" id="do-trash" ${plan.blocked ? "disabled" : ""}>Move to Trash</button>
    </div>`);
  dialog.onclick = async (e) => {
    if (e.target.closest("[data-close]")) return closeDialog();
    if (e.target.id !== "do-trash") return;
    busy = true;
    e.target.disabled = true;
    e.target.innerHTML = `<span class="spinner"></span>Moving`;
    let body;
    try {
      const r = await api.trashSessions(keys);
      for (const k of r.trashed) state.selected.delete(k);
      body = `<h2>Moved ${plural(r.trashed.length, "session", "sessions")} to the Trash</h2>
        ${r.exported.length ? `<p class="muted">OpenCode sessions were saved to:</p><pre class="paths">${esc(r.exported.map(tilde).join("\n"))}</pre>` : ""}
        ${r.failed.length ? `<div class="callout error"><strong>${plural(r.failed.length, "session wasn't", "sessions weren't")} moved</strong><ul>${r.failed.map(([k, why]) => `<li>${esc(state.byKey.get(k)?.title ?? k)}: ${esc(why)}</li>`).join("")}</ul></div>` : ""}`;
    } catch (err) {
      body = `<h2>Nothing was moved</h2><div class="callout error">${esc(errorText(err))}</div>`;
    } finally {
      busy = false;
    }
    doneDialog(body);
    await rescan();
  };
}

// ----- loading --------------------------------------------------------------------

function applyScan(data) {
  state.home = data.home;
  state.t3 = data.t3;
  state.sessions = data.sessions;
  state.byKey = new Map(data.sessions.map((s) => [s.key, s]));
  for (const k of [...state.selected]) if (!state.byKey.has(k)) state.selected.delete(k);
  state.pendingRestart = data.pendingRestart ?? [];
  renderBanner();
  $("reader").dataset.key = "";
  const status = $("t3-status");
  if (data.t3.found) {
    status.textContent = `T3 Code at ${tilde(data.t3.base)}, ${plural(data.t3.projects.length, "project", "projects")}`;
    status.classList.remove("error");
  } else {
    status.textContent = `T3 Code not found${data.t3.error ? `: ${data.t3.error}` : ""}`;
    status.classList.add("error");
  }
  render();
}

async function rescan() {
  try {
    applyScan(await api.scan());
  } catch (e) {
    $("t3-status").textContent = `Couldn't read sessions: ${errorText(e)}`;
  }
}

function renderBanner() {
  const b = $("banner");
  b.hidden = state.pendingRestart.length === 0;
  if (b.hidden) return;
  b.innerHTML = `<span>${esc(state.pendingRestart.join(", "))} ${state.pendingRestart.length === 1 ? "is" : "are"} stopped. t3-import stopped it for an import.</span>
    <button type="button" class="btn" id="restart-t3">Start it again</button>`;
}

$("banner").addEventListener("click", async (e) => {
  if (e.target.id !== "restart-t3") return;
  e.target.disabled = true;
  e.target.innerHTML = `<span class="spinner"></span>Starting`;
  try {
    const r = await api.restartT3();
    state.pendingRestart = r.pendingRestart;
    const failed = r.restarted.filter((l) => !l.ok);
    if (failed.length) {
      $("banner").innerHTML = `<span>${esc(failed.map((l) => l.text).join(" "))}</span><button type="button" class="btn" id="restart-t3">Try again</button>`;
      return;
    }
  } catch (err) {
    $("banner").innerHTML = `<span>${esc(errorText(err))}</span><button type="button" class="btn" id="restart-t3">Try again</button>`;
    return;
  }
  renderBanner();
});

(async function start() {
  if (bridgeMissing) {
    document.body.innerHTML = `<div class="fatal"><h1>t3-import can't reach its own backend</h1>
      <p>This build of the app is missing its connection between the window and the program. Please report it at github.com/paulomanrique/t3code-import/issues. The command line still works: <code>t3-import list</code>, <code>check</code> and <code>import</code>.</p></div>`;
    return;
  }
  onRestartFailed((message) => {
    const b = $("banner");
    b.hidden = false;
    b.innerHTML = `<span>T3 couldn't be started again: ${esc(message)} Try again, or close the window once more to quit and start it yourself.</span>
      <button type="button" class="btn" id="restart-t3">Try again</button>`;
  });
  $("threads").innerHTML = `<li class="list-empty"><span class="spinner"></span>Looking for Claude, Codex, Grok and OpenCode sessions…</li>`;
  $("reader").innerHTML = "";
  await rescan();
  $("threads").focus();
})();

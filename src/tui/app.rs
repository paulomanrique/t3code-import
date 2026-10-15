use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::model::{Provider, SessionSummary};
use crate::scan::Roots;
use crate::t3db::control::{Restart, RunningT3};
use crate::t3db::{self, GuardReport, ImportOptions, ImportReport, Placement, Progress, Snapshot, T3Home};

pub enum Msg {
    Scanned(Vec<SessionSummary>),
    Snapshot(Result<(T3Home, Snapshot), String>),
    /// The guard's verdict, plus what is running when T3 isn't closed.
    Guard(Result<GuardReport, String>, Option<Result<Vec<RunningT3>, String>>),
    Closed(Result<Restart, String>),
    Restarted(Vec<Result<String, String>>),
    Progress(String),
    Done(Result<ImportReport, String>),
}

#[derive(PartialEq, Eq)]
pub enum Screen {
    Loading,
    Picker,
    Confirm,
    Importing,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Input {
    None,
    Search,
    MaxMessages,
}

/// One line of the picker: a project header or a session under it.
#[derive(Clone, Copy)]
pub enum Row {
    Group(usize),
    Session(usize),
}

/// The sessions that land in one T3 project. Worktree sessions join their main
/// repository's project, so the group is keyed by that root, not by cwd.
pub struct Group {
    pub root: PathBuf,
    /// Visible sessions, newest first.
    pub sessions: Vec<usize>,
    /// Sessions of this project hidden because T3 already has them.
    pub hidden_in_t3: usize,
    pub expanded: bool,
}

pub struct App {
    roots: Roots,
    t3_home_override: Option<PathBuf>,
    tx: Sender<Msg>,
    pub screen: Screen,
    pub should_quit: bool,
    pub spinner: usize,

    pub sessions: Vec<SessionSummary>,
    /// Where each session goes in T3, parallel to `sessions`.
    pub placements: Vec<Placement>,
    scanned: bool,
    pub groups: Vec<Group>,
    hidden_in_t3: usize,
    pub rows: Vec<Row>,
    pub list: ListState,
    pub selected: HashSet<usize>,

    pub home: Option<T3Home>,
    pub snapshot: Option<Snapshot>,
    pub snapshot_error: Option<String>,

    pub providers: HashSet<Provider>,
    pub search: String,
    pub input: Input,
    pub input_buf: String,
    pub max_messages: Option<usize>,
    /// Show sessions T3 already has (hidden by default: they can't be picked).
    pub show_in_t3: bool,
    pub help: bool,

    pub guard: Option<Result<GuardReport, String>>,
    pub running: Option<Result<Vec<RunningT3>, String>>,
    /// True while t3-import is closing T3.
    pub closing: bool,
    pub close_error: Option<String>,
    /// Background services t3-import stopped and must start again.
    pub restart: Restart,
    pub restarted: Vec<Result<String, String>>,
    pub log: Vec<String>,
    pub report: Option<Result<ImportReport, String>>,
}

impl App {
    pub fn new(roots: Roots, t3_home_override: Option<PathBuf>, tx: Sender<Msg>) -> Self {
        App {
            roots,
            t3_home_override,
            tx,
            screen: Screen::Loading,
            should_quit: false,
            spinner: 0,
            sessions: Vec::new(),
            placements: Vec::new(),
            scanned: false,
            groups: Vec::new(),
            hidden_in_t3: 0,
            rows: Vec::new(),
            list: ListState::default(),
            selected: HashSet::new(),
            home: None,
            snapshot: None,
            snapshot_error: None,
            providers: Provider::ALL.into_iter().collect(),
            search: String::new(),
            input: Input::None,
            input_buf: String::new(),
            max_messages: None,
            show_in_t3: false,
            help: false,
            guard: None,
            running: None,
            closing: false,
            close_error: None,
            restart: Restart::default(),
            restarted: Vec::new(),
            log: Vec::new(),
            report: None,
        }
    }

    pub fn start_scan(&mut self) {
        let roots = self.roots.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Scanned(crate::scan_all(&roots)));
        });
        let home = self.t3_home_override.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = T3Home::detect(home)
                .and_then(|home| t3db::snapshot(&home).map(|snap| (home, snap)))
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Snapshot(result));
        });
    }

    /// The picker opens once both the sessions and T3's state are known, so
    /// sessions already in T3 never flash in and then vanish.
    fn leave_loading(&mut self) {
        let t3_known = self.snapshot.is_some() || self.snapshot_error.is_some();
        if self.screen == Screen::Loading && self.scanned && t3_known {
            self.screen = Screen::Picker;
        }
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Scanned(sessions) => {
                self.placements = sessions.iter().map(|s| t3db::resolve_placement(&s.cwd)).collect();
                self.sessions = sessions;
                self.scanned = true;
                self.rebuild_groups();
                self.leave_loading();
            }
            Msg::Snapshot(Ok((home, snap))) => {
                self.home = Some(home);
                self.snapshot = Some(snap);
                // "In T3" is only known now; regroup so those sessions hide.
                self.rebuild_groups();
                self.leave_loading();
            }
            Msg::Snapshot(Err(e)) => {
                self.snapshot_error = Some(e);
                self.leave_loading();
            }
            Msg::Guard(result, running) => {
                self.guard = Some(result);
                self.running = running;
            }
            Msg::Closed(result) => {
                self.closing = false;
                match result {
                    Ok(restart) => {
                        self.restart.extend(restart);
                        self.close_error = None;
                    }
                    Err(e) => self.close_error = Some(e),
                }
                self.run_guard();
            }
            Msg::Restarted(lines) => self.restarted = lines,
            Msg::Progress(line) => self.log.push(line),
            Msg::Done(result) => {
                self.report = Some(result);
                self.screen = Screen::Done;
            }
        }
    }

    // ----- derived state -------------------------------------------------

    pub fn instance_for(&self, provider: Provider) -> Option<&str> {
        self.snapshot.as_ref()?.provider_instances.get(&provider)?.first().map(String::as_str)
    }

    /// True when T3 already has this session (same deterministic thread id).
    pub fn in_t3(&self, idx: usize) -> bool {
        let s = &self.sessions[idx];
        match (&self.snapshot, self.instance_for(s.provider)) {
            (Some(snap), Some(instance)) => {
                snap.has_native(s.provider, &s.session_id) || snap.thread_ids.contains(&t3db::thread_id_for(instance, &s.session_id))
            }
            _ => false,
        }
    }

    pub fn provider_disabled(&self, provider: Provider) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.disabled_providers.contains(&provider))
    }

    pub fn selectable(&self, idx: usize) -> bool {
        let s = &self.sessions[idx];
        s.resumable && !self.in_t3(idx) && self.instance_for(s.provider).is_some()
    }

    fn matches_search(&self, s: &SessionSummary) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let q = self.search.to_lowercase();
        s.title.to_lowercase().contains(&q)
            || s.cwd.to_string_lossy().to_lowercase().contains(&q)
            || s.session_id.to_lowercase().contains(&q)
    }

    /// Sessions per provider that the current search and in-T3 setting would
    /// show if that provider were on.
    pub fn provider_counts(&self) -> [usize; 4] {
        let mut counts = [0; 4];
        for (i, s) in self.sessions.iter().enumerate() {
            if self.matches_search(s) && (self.show_in_t3 || !self.in_t3(i)) {
                counts[Provider::ALL.iter().position(|&p| p == s.provider).unwrap()] += 1;
            }
        }
        counts
    }

    /// Sessions hidden only because T3 already has them, including those of
    /// projects that drop out entirely.
    pub fn hidden_in_t3(&self) -> usize {
        self.hidden_in_t3
    }

    pub fn project_exists(&self, root: &std::path::Path) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.projects.iter().any(|p| p.workspace_root == root))
    }

    fn rebuild_groups(&mut self) {
        // Capture focus before `groups` changes: a Row::Group index only means
        // something against the groups it was taken from.
        let keep = self.current_row_key();
        // Projects start open; remember only the ones the user folded.
        let collapsed: HashSet<PathBuf> =
            self.groups.iter().filter(|g| !g.expanded).map(|g| g.root.clone()).collect();
        let mut by_root: BTreeMap<PathBuf, (Vec<usize>, usize)> = BTreeMap::new();
        for (i, s) in self.sessions.iter().enumerate() {
            if !self.providers.contains(&s.provider) || !self.matches_search(s) {
                continue;
            }
            let entry = by_root.entry(self.placements[i].workspace_root.clone()).or_default();
            if !self.show_in_t3 && self.in_t3(i) {
                entry.1 += 1;
            } else {
                entry.0.push(i);
            }
        }
        self.hidden_in_t3 = by_root.values().map(|(_, hidden)| hidden).sum();
        let mut groups: Vec<Group> = by_root
            .into_iter()
            .filter(|(_, (sessions, _))| !sessions.is_empty())
            .map(|(root, (sessions, hidden_in_t3))| Group {
                expanded: !collapsed.contains(&root),
                root,
                sessions,
                hidden_in_t3,
            })
            .collect();
        // Most recently active project first; sessions already newest-first.
        groups.sort_by(|a, b| {
            let latest = |g: &Group| self.sessions[g.sessions[0]].updated_at;
            latest(b).cmp(&latest(a))
        });
        self.groups = groups;
        self.rebuild_rows_keeping(keep);
    }

    fn rebuild_rows(&mut self) {
        let keep = self.current_row_key();
        self.rebuild_rows_keeping(keep);
    }

    fn rebuild_rows_keeping(&mut self, keep: Option<(bool, PathBuf, usize)>) {
        self.rows.clear();
        for (gi, g) in self.groups.iter().enumerate() {
            self.rows.push(Row::Group(gi));
            if g.expanded {
                self.rows.extend(g.sessions.iter().map(|&i| Row::Session(i)));
            }
        }
        let pos = keep
            .and_then(|k| self.rows.iter().position(|r| self.row_key(*r) == k))
            .unwrap_or(0);
        self.list.select(if self.rows.is_empty() { None } else { Some(pos) });
    }

    fn row_key(&self, row: Row) -> (bool, PathBuf, usize) {
        match row {
            Row::Group(g) => (true, self.groups[g].root.clone(), 0),
            Row::Session(i) => (false, self.placements[i].workspace_root.clone(), i),
        }
    }

    fn current_row_key(&self) -> Option<(bool, PathBuf, usize)> {
        let row = *self.rows.get(self.list.selected()?)?;
        match row {
            Row::Group(g) if g < self.groups.len() => Some(self.row_key(row)),
            Row::Session(_) => Some(self.row_key(row)),
            _ => None,
        }
    }

    pub fn current(&self) -> Option<Row> {
        self.rows.get(self.list.selected()?).copied()
    }

    pub fn group_selection(&self, g: usize) -> (usize, usize) {
        let group = &self.groups[g];
        let selectable = group.sessions.iter().filter(|&&i| self.selectable(i)).count();
        let chosen = group.sessions.iter().filter(|i| self.selected.contains(i)).count();
        (chosen, selectable)
    }

    pub fn selected_sessions(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.selected.iter().copied().collect();
        v.sort();
        v
    }

    /// Project roots the import will have to create.
    pub fn projects_to_create(&self) -> Vec<PathBuf> {
        let existing: HashSet<PathBuf> = self
            .snapshot
            .iter()
            .flat_map(|s| s.projects.iter().map(|p| p.workspace_root.clone()))
            .collect();
        let mut roots: Vec<PathBuf> = self
            .selected_sessions()
            .into_iter()
            .map(|i| self.placements[i].workspace_root.clone())
            .filter(|r| !existing.contains(r))
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    // ----- input ---------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            // Never leave mid-close or mid-import: a service could stay stopped.
            if self.screen != Screen::Importing && !self.closing {
                self.should_quit = true;
            }
            return;
        }
        if self.input != Input::None {
            self.on_input_key(key);
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        match self.screen {
            Screen::Loading => {
                if key.code == KeyCode::Char('q') {
                    self.should_quit = true;
                }
            }
            Screen::Picker => self.on_picker_key(key),
            Screen::Confirm if self.closing => {}
            Screen::Confirm => match key.code {
                KeyCode::Esc | KeyCode::Char('b') => self.screen = Screen::Picker,
                KeyCode::Char('r') => self.run_guard(),
                KeyCode::Char('c') if self.can_close_t3() => self.close_t3(),
                KeyCode::Enter | KeyCode::Char('y') if self.guard_ok() => self.start_import(),
                _ => {}
            },
            Screen::Importing => {}
            Screen::Done => match key.code {
                KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => self.should_quit = true,
                _ => {}
            },
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if self.input == Input::Search {
                    self.search.clear();
                    self.rebuild_groups();
                }
                self.input = Input::None;
            }
            KeyCode::Enter => {
                if self.input == Input::MaxMessages {
                    let v = self.input_buf.trim();
                    self.max_messages = v.parse::<usize>().ok().filter(|&n| n > 0);
                }
                self.input = Input::None;
            }
            KeyCode::Backspace => {
                self.input_buf.pop();
                self.after_input_edit();
            }
            KeyCode::Char(c) => {
                if self.input == Input::MaxMessages && !c.is_ascii_digit() {
                    return;
                }
                self.input_buf.push(c);
                self.after_input_edit();
            }
            _ => {}
        }
    }

    fn after_input_edit(&mut self) {
        if self.input == Input::Search {
            self.search = self.input_buf.clone();
            self.rebuild_groups();
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        let len = self.rows.len();
        let pos = self.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('t') => {
                self.show_in_t3 = !self.show_in_t3;
                self.rebuild_groups();
            }
            KeyCode::Esc if !self.search.is_empty() => {
                self.search.clear();
                self.rebuild_groups();
            }
            KeyCode::Down | KeyCode::Char('j') if len > 0 => self.list.select(Some((pos + 1).min(len - 1))),
            KeyCode::Up | KeyCode::Char('k') => self.list.select(Some(pos.saturating_sub(1))),
            KeyCode::PageDown if len > 0 => self.list.select(Some((pos + 20).min(len - 1))),
            KeyCode::PageUp => self.list.select(Some(pos.saturating_sub(20))),
            KeyCode::Home | KeyCode::Char('g') => self.list.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') if len > 0 => self.list.select(Some(len - 1)),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => self.set_expanded(true),
            KeyCode::Left | KeyCode::Char('h') => self.set_expanded(false),
            KeyCode::Char(' ') => self.toggle_current(),
            KeyCode::Char('a') => self.toggle_all_visible(),
            KeyCode::Char('e') => {
                let open = !self.groups.iter().all(|g| g.expanded);
                self.groups.iter_mut().for_each(|g| g.expanded = open);
                self.rebuild_rows();
            }
            KeyCode::Char('/') => {
                self.input = Input::Search;
                self.input_buf = self.search.clone();
            }
            KeyCode::Char('m') => {
                self.input = Input::MaxMessages;
                self.input_buf = self.max_messages.map(|n| n.to_string()).unwrap_or_default();
            }
            KeyCode::Char(c @ '1'..='4') => {
                let p = Provider::ALL[c as usize - '1' as usize];
                if !self.providers.remove(&p) {
                    self.providers.insert(p);
                }
                self.rebuild_groups();
            }
            KeyCode::Char('i') if !self.selected.is_empty() && self.home.is_some() => {
                self.screen = Screen::Confirm;
                self.run_guard();
            }
            _ => {}
        }
    }

    fn set_expanded(&mut self, open: bool) {
        match self.current() {
            Some(Row::Group(g)) => {
                self.groups[g].expanded = open;
                self.rebuild_rows();
            }
            Some(Row::Session(i)) if !open => {
                let root = self.placements[i].workspace_root.clone();
                if let Some(g) = self.groups.iter_mut().find(|g| g.root == root) {
                    g.expanded = false;
                }
                // The session row disappears; land on its project instead.
                self.rebuild_rows_keeping(Some((true, root, 0)));
            }
            _ => {}
        }
    }

    fn toggle_current(&mut self) {
        match self.current() {
            Some(Row::Session(i)) => {
                if !self.selected.remove(&i) && self.selectable(i) {
                    self.selected.insert(i);
                }
            }
            Some(Row::Group(g)) => {
                let ids = self.groups[g].sessions.clone();
                self.toggle_set(&ids);
            }
            None => {}
        }
    }

    fn toggle_all_visible(&mut self) {
        let ids: Vec<usize> = self.groups.iter().flat_map(|g| g.sessions.iter().copied()).collect();
        self.toggle_set(&ids);
    }

    /// Selects every selectable session in `ids`, or clears them if all already are.
    fn toggle_set(&mut self, ids: &[usize]) {
        let selectable: Vec<usize> = ids.iter().copied().filter(|&i| self.selectable(i)).collect();
        if selectable.iter().all(|i| self.selected.contains(i)) {
            ids.iter().for_each(|i| {
                self.selected.remove(i);
            });
        } else {
            self.selected.extend(selectable);
        }
    }

    // ----- guard & import ------------------------------------------------

    pub fn guard_ok(&self) -> bool {
        matches!(&self.guard, Some(Ok(r)) if r.ok())
    }

    /// T3 is running and everything running is something t3-import may close.
    pub fn can_close_t3(&self) -> bool {
        matches!(&self.running, Some(Ok(r)) if !r.is_empty() && r.iter().all(RunningT3::can_close))
    }

    fn run_guard(&mut self) {
        let Some(home) = self.home.clone() else { return };
        self.guard = None;
        self.running = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let guard = t3db::check(&home).map_err(|e| format!("{e:#}"));
            let open = matches!(&guard, Ok(r) if !r.t3_running.is_empty());
            let running = open.then(|| t3db::control::find(&home).map_err(|e| format!("{e:#}")));
            let _ = tx.send(Msg::Guard(guard, running));
        });
    }

    fn close_t3(&mut self) {
        let (Some(home), Some(Ok(running))) = (self.home.clone(), self.running.clone()) else { return };
        self.closing = true;
        self.close_error = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Closed(t3db::control::close(&home, &running).map_err(|e| format!("{e:#}"))));
        });
    }

    /// Services still owed a start; taken when leaving without importing.
    pub fn take_restart(&mut self) -> Restart {
        std::mem::take(&mut self.restart)
    }

    fn start_import(&mut self) {
        let (Some(home), Some(snapshot)) = (self.home.clone(), self.snapshot.clone()) else { return };
        let picked: Vec<SessionSummary> =
            self.selected_sessions().into_iter().map(|i| self.sessions[i].clone()).collect();
        let roots = self.roots.clone();
        let options = ImportOptions { max_messages: self.max_messages, dry_run: false };
        let mut restart = self.take_restart();
        let tx = self.tx.clone();
        self.screen = Screen::Importing;
        self.log.clear();
        thread::spawn(move || {
            let result = (|| -> anyhow::Result<ImportReport> {
                let mut requests = Vec::with_capacity(picked.len());
                for (n, s) in picked.iter().enumerate() {
                    let _ = tx.send(Msg::Progress(format!(
                        "Reading {}/{}: {} {}",
                        n + 1,
                        picked.len(),
                        s.provider.label(),
                        s.title
                    )));
                    requests.push(crate::request_for(&roots, &snapshot, s)?);
                }
                t3db::import(&home, requests, &options, &mut |p| {
                    let _ = tx.send(Msg::Progress(describe(&p)));
                })
            })();
            // Start the services again whether or not the import went through.
            if !restart.is_empty() {
                let _ = tx.send(Msg::Progress("Starting the T3 background service again".into()));
                let _ = tx.send(Msg::Restarted(restart.run()));
            }
            let _ = tx.send(Msg::Done(result.map_err(|e| format!("{e:#}"))));
        });
    }
}

fn describe(p: &Progress) -> String {
    match p {
        Progress::Guard => "Checking the T3 database".into(),
        Progress::Backup => "Backing up the T3 database".into(),
        Progress::CreatingProject(root) => format!("Creating project {}", root.display()),
        Progress::Writing { done, total } => format!("Writing threads {done}/{total}"),
        Progress::Verifying => "Verifying what was written".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use chrono::{TimeZone, Utc};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    fn session(cwd: &str, title: &str, minute: u32) -> SessionSummary {
        let at = Utc.with_ymd_and_hms(2026, 10, 1, 12, minute, 0).unwrap();
        SessionSummary {
            provider: Provider::Claude,
            session_id: format!("00000000-0000-4000-8000-0000000000{minute:02}"),
            cwd: PathBuf::from(cwd),
            title: title.into(),
            created_at: at,
            updated_at: at,
            message_count: None,
            model: None,
            source: PathBuf::new(),
            resumable: true,
        }
    }

    fn app() -> App {
        let (tx, _rx) = mpsc::channel();
        let roots = Roots::detect();
        let mut app = App::new(roots, None, tx);
        app.on_msg(Msg::Snapshot(Err("no T3 in tests".into())));
        // Newest first, so groups are A, B, C.
        app.on_msg(Msg::Scanned(vec![
            session("/a", "alpha", 3),
            session("/b", "beta", 2),
            session("/c", "gamma", 1),
        ]));
        app
    }

    fn key(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn focused_cwd(app: &App) -> PathBuf {
        match app.current().unwrap() {
            Row::Group(g) => app.groups[g].root.clone(),
            Row::Session(i) => app.placements[i].workspace_root.clone(),
        }
    }

    #[test]
    fn filtering_keeps_focus_on_the_same_project() {
        let mut app = app();
        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Down); // past /a's session to /b
        assert_eq!(focused_cwd(&app), PathBuf::from("/b"));
        key(&mut app, KeyCode::Char('/'));
        for c in "ta".chars() {
            key(&mut app, KeyCode::Char(c)); // matches beta; alpha drops out
        }
        assert_eq!(focused_cwd(&app), PathBuf::from("/b"));
    }

    #[test]
    fn collapsing_from_a_session_focuses_its_project() {
        let mut app = app();
        key(&mut app, KeyCode::End); // /c's session, the last row
        assert!(matches!(app.current(), Some(Row::Session(_))));
        key(&mut app, KeyCode::Left);
        assert!(matches!(app.current(), Some(Row::Group(_))));
        assert_eq!(focused_cwd(&app), PathBuf::from("/c"));
    }

    #[test]
    fn worktree_sessions_join_their_repository_project() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("repo");
        let work = temp.path().join("repo-feature");
        std::fs::create_dir_all(main.join(".git/worktrees/feature")).unwrap();
        std::fs::create_dir(&work).unwrap();
        std::fs::write(work.join(".git"), format!("gitdir: {}\n", main.join(".git/worktrees/feature").display()))
            .unwrap();
        std::fs::write(main.join(".git/worktrees/feature/commondir"), "../..\n").unwrap();
        std::fs::write(main.join(".git/worktrees/feature/HEAD"), "ref: refs/heads/feature\n").unwrap();

        let (tx, _rx) = mpsc::channel();
        let mut app = App::new(Roots::detect(), None, tx);
        app.on_msg(Msg::Snapshot(Err("no T3 in tests".into())));
        app.on_msg(Msg::Scanned(vec![
            session(work.to_str().unwrap(), "on the branch", 2),
            session(main.to_str().unwrap(), "on main", 1),
        ]));
        assert_eq!(app.groups.len(), 1);
        assert_eq!(app.groups[0].root, main.canonicalize().unwrap());
        assert_eq!(app.groups[0].sessions.len(), 2);
    }

    #[test]
    fn sessions_already_in_t3_hide_until_shown() {
        let mut app = app();
        let mut snapshot = Snapshot::default();
        snapshot.provider_instances.insert(Provider::Claude, vec!["claudeAgent".into()]);
        snapshot.thread_ids.insert(t3db::thread_id_for("claudeAgent", &app.sessions[1].session_id));
        app.snapshot = Some(snapshot);
        app.rebuild_groups();
        // /b's only session is in T3, so the project disappears.
        let roots: Vec<_> = app.groups.iter().map(|g| g.root.clone()).collect();
        assert_eq!(roots, [PathBuf::from("/a"), PathBuf::from("/c")]);
        assert_eq!(app.hidden_in_t3(), 1);
        assert_eq!(app.provider_counts()[0], 2);

        key(&mut app, KeyCode::Char('t'));
        assert_eq!(app.groups.len(), 3);
        assert_eq!(app.hidden_in_t3(), 0);
        // Shown, but still not selectable.
        let b = app.groups.iter().position(|g| g.root == PathBuf::from("/b")).unwrap();
        assert_eq!(app.group_selection(b), (0, 0));
    }
}

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::model::{Provider, SessionSummary};
use crate::scan::Roots;
use crate::t3db::{self, GuardReport, ImportOptions, ImportReport, Progress, Snapshot, T3Home};

pub enum Msg {
    Scanned(Vec<SessionSummary>),
    Snapshot(Result<(T3Home, Snapshot), String>),
    Guard(Result<GuardReport, String>),
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

pub struct Group {
    pub cwd: PathBuf,
    pub sessions: Vec<usize>,
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
    pub groups: Vec<Group>,
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

    pub guard: Option<Result<GuardReport, String>>,
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
            groups: Vec::new(),
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
            guard: None,
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

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
    }

    pub fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Scanned(sessions) => {
                self.sessions = sessions;
                self.rebuild_groups();
                if self.screen == Screen::Loading {
                    self.screen = Screen::Picker;
                }
            }
            Msg::Snapshot(Ok((home, snap))) => {
                self.home = Some(home);
                self.snapshot = Some(snap);
            }
            Msg::Snapshot(Err(e)) => self.snapshot_error = Some(e),
            Msg::Guard(result) => self.guard = Some(result),
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
                snap.thread_ids.contains(&t3db::thread_id_for(instance, &s.session_id))
            }
            _ => false,
        }
    }

    pub fn selectable(&self, idx: usize) -> bool {
        let s = &self.sessions[idx];
        s.resumable && !self.in_t3(idx) && self.instance_for(s.provider).is_some()
    }

    fn matches(&self, s: &SessionSummary) -> bool {
        if !self.providers.contains(&s.provider) {
            return false;
        }
        if self.search.is_empty() {
            return true;
        }
        let q = self.search.to_lowercase();
        s.title.to_lowercase().contains(&q)
            || s.cwd.to_string_lossy().to_lowercase().contains(&q)
            || s.session_id.to_lowercase().contains(&q)
    }

    fn rebuild_groups(&mut self) {
        let expanded: HashSet<PathBuf> =
            self.groups.iter().filter(|g| g.expanded).map(|g| g.cwd.clone()).collect();
        let mut by_cwd: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (i, s) in self.sessions.iter().enumerate() {
            if self.matches(s) {
                by_cwd.entry(s.cwd.clone()).or_default().push(i);
            }
        }
        let mut groups: Vec<Group> = by_cwd
            .into_iter()
            .map(|(cwd, sessions)| Group { expanded: expanded.contains(&cwd), cwd, sessions })
            .collect();
        // Most recently active project first; sessions already newest-first.
        groups.sort_by(|a, b| {
            let latest = |g: &Group| self.sessions[g.sessions[0]].updated_at;
            latest(b).cmp(&latest(a))
        });
        self.groups = groups;
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let keep = self.current_row_key();
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
            Row::Group(g) => (true, self.groups[g].cwd.clone(), 0),
            Row::Session(i) => (false, self.sessions[i].cwd.clone(), i),
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
            .map(|i| t3db::resolve_placement(&self.sessions[i].cwd).workspace_root)
            .filter(|r| !existing.contains(r))
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    // ----- input ---------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.screen != Screen::Importing {
                self.should_quit = true;
            }
            return;
        }
        if self.input != Input::None {
            self.on_input_key(key);
            return;
        }
        match self.screen {
            Screen::Loading => {
                if key.code == KeyCode::Char('q') {
                    self.should_quit = true;
                }
            }
            Screen::Picker => self.on_picker_key(key),
            Screen::Confirm => match key.code {
                KeyCode::Esc | KeyCode::Char('b') => self.screen = Screen::Picker,
                KeyCode::Char('r') => self.run_guard(),
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
                let cwd = self.sessions[i].cwd.clone();
                if let Some(g) = self.groups.iter_mut().find(|g| g.cwd == cwd) {
                    g.expanded = false;
                }
                self.rebuild_rows();
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

    fn run_guard(&mut self) {
        let Some(home) = self.home.clone() else { return };
        self.guard = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::Guard(t3db::check(&home).map_err(|e| format!("{e:#}"))));
        });
    }

    fn start_import(&mut self) {
        let (Some(home), Some(snapshot)) = (self.home.clone(), self.snapshot.clone()) else { return };
        let picked: Vec<SessionSummary> =
            self.selected_sessions().into_iter().map(|i| self.sessions[i].clone()).collect();
        let roots = self.roots.clone();
        let options = ImportOptions { max_messages: self.max_messages, dry_run: false };
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

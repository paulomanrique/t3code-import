use std::path::Path;

use chrono::{DateTime, Datelike, Local, Utc};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};

use crate::model::Provider;

use super::app::{App, Input, Row, Screen};

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
const DIM: Color = Color::DarkGray;
const ACCENT: Color = Color::Rgb(224, 138, 95);

fn provider_color(p: Provider) -> Color {
    match p {
        Provider::Claude => Color::Rgb(234, 88, 12),
        Provider::Codex => Color::Rgb(96, 140, 245),
        Provider::Grok => Color::Rgb(200, 200, 200),
        Provider::OpenCode => Color::Rgb(74, 196, 140),
    }
}

fn tilde(path: &Path) -> String {
    let s = path.display().to_string();
    match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => {
            format!("~{}", &s[home.display().to_string().len()..])
        }
        _ => s,
    }
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        return s;
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Shortens from the left, keeping the end of a path, which is the part that names it.
fn truncate_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(n - max.saturating_sub(1)).collect();
    format!("…{tail}")
}

fn ago(at: DateTime<Utc>) -> String {
    let now = Utc::now();
    let mins = (now - at).num_minutes().max(0);
    match mins {
        0..=1 => "just now".into(),
        2..=59 => format!("{mins}m ago"),
        60..=1439 => format!("{}h ago", mins / 60),
        1440..=2879 => "yesterday".into(),
        2880..=10079 => format!("{}d ago", mins / 1440),
        _ if at.year() == now.year() => at.format("%b %d").to_string(),
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let filter_line = filter_line(app);
    // The filter bar wraps to a second line on narrow terminals.
    let filter_height = if filter_line.width() < f.area().width as usize { 2 } else { 3 };
    let [header, filters, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(filter_height),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    match app.screen {
        Screen::Loading => draw_loading(f, app, body),
        _ => {
            let block = Block::new().borders(Borders::BOTTOM).border_style(Style::new().fg(DIM));
            f.render_widget(Paragraph::new(filter_line).block(block).wrap(Wrap { trim: true }), filters);
            draw_picker(f, app, body);
        }
    }
    draw_footer(f, app, footer);
    match app.screen {
        Screen::Confirm => draw_confirm(f, app),
        Screen::Importing => draw_importing(f, app),
        Screen::Done => draw_done(f, app),
        _ => {}
    }
    if app.help {
        draw_help(f);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" t3-import ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::raw("  Bring your CLI sessions into T3 Code"),
    ];
    let right = match (&app.home, &app.snapshot_error) {
        (Some(home), _) => {
            let projects = app.snapshot.as_ref().map(|s| s.projects.len()).unwrap_or(0);
            Span::styled(format!("T3 at {}  ·  {} ", tilde(&home.base), plural(projects, "project", "projects")), Style::new().fg(DIM))
        }
        (None, Some(err)) => Span::styled(format!("T3 Code not found: {} ", truncate(err, 60)), Style::new().fg(Color::Red)),
        (None, None) => Span::styled("reading T3 Code… ", Style::new().fg(DIM)),
    };
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.width());
    spans.push(Span::raw(" ".repeat(gap.max(2))));
    spans.push(right);
    f.render_widget(Line::from(spans), area);
}

fn draw_loading(f: &mut Frame, app: &App, area: Rect) {
    let text = format!("{}  Looking for Claude, Codex, Grok and OpenCode sessions…", SPINNER[app.spinner % SPINNER.len()]);
    let [_, mid, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(text).centered().fg(DIM), mid);
}

/// The row of toggles above the list: which CLIs are shown, whether sessions
/// already in T3 are shown, and the search.
fn filter_line(app: &App) -> Line<'static> {
    let key = |k: &str| Span::styled(format!("{k} "), Style::new().fg(DIM));
    let mut spans = vec![Span::styled(" Show  ", Style::new().fg(DIM))];
    let counts = app.provider_counts();
    for (n, p) in Provider::ALL.into_iter().enumerate() {
        spans.push(key(&(n + 1).to_string()));
        if app.providers.contains(&p) {
            spans.push(Span::styled(format!("■ {}", p.label()), Style::new().fg(provider_color(p)).bold()));
            spans.push(Span::styled(format!(" {}", counts[n]), Style::new().fg(DIM)));
        } else {
            spans.push(Span::styled(format!("□ {}", p.label()), Style::new().fg(DIM).crossed_out()));
        }
        spans.push(Span::raw("   "));
    }
    spans.push(key("t"));
    if app.show_in_t3 {
        spans.push(Span::raw("■ already in T3"));
    } else {
        spans.push(Span::styled("□ already in T3", Style::new().fg(DIM)));
        let hidden = app.hidden_in_t3();
        if hidden > 0 {
            spans.push(Span::styled(format!(" ({hidden} hidden)"), Style::new().fg(DIM)));
        }
    }
    spans.push(Span::raw("   "));
    spans.push(key("/"));
    if app.input == Input::Search {
        spans.push(Span::styled(format!("search: {}▏", app.input_buf), Style::new().fg(ACCENT)));
    } else if app.search.is_empty() {
        spans.push(Span::styled("search", Style::new().fg(DIM)));
    } else {
        spans.push(Span::styled(format!("search: {}", app.search), Style::new().fg(ACCENT)));
        spans.push(Span::styled("  (esc clears)", Style::new().fg(DIM)));
    }
    Line::from(spans)
}

fn draw_picker(f: &mut Frame, app: &mut App, area: Rect) {
    let wide = area.width >= 110;
    let (list_area, detail_area) = if wide {
        let [l, d] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(46)]).areas(area);
        (l, d)
    } else {
        let [l, d] = Layout::vertical([Constraint::Fill(1), Constraint::Length(11)]).areas(area);
        (l, d)
    };

    let width = list_area.width.saturating_sub(1) as usize;
    let items: Vec<ListItem> = app.rows.iter().map(|&row| ListItem::new(row_line(app, row, width))).collect();
    let empty = items.is_empty();
    let border = if wide { Borders::RIGHT } else { Borders::BOTTOM };
    let list = List::new(items)
        .block(Block::new().borders(border).border_style(Style::new().fg(DIM)))
        .highlight_style(Style::new().bg(Color::Rgb(58, 58, 74)).bold());
    f.render_stateful_widget(list, list_area, &mut app.list);
    if empty {
        let msg = if app.sessions.is_empty() {
            "No Claude, Codex, Grok or OpenCode sessions found on this machine.".to_string()
        } else if !app.search.is_empty() {
            format!("Nothing matches \"{}\". Press esc to clear the search.", app.search)
        } else if app.hidden_in_t3() > 0 {
            "Every session shown is already in T3. Press t to see them.".to_string()
        } else {
            "Nothing to show. Turn a CLI back on with 1-4.".to_string()
        };
        let [_, mid, _] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1), Constraint::Fill(1)]).areas(list_area);
        f.render_widget(Paragraph::new(msg).fg(DIM).centered(), mid);
    }

    draw_detail(f, app, detail_area);
}

fn row_line(app: &App, row: Row, width: usize) -> Line<'static> {
    match row {
        Row::Group(g) => {
            let group = &app.groups[g];
            let (chosen, selectable) = app.group_selection(g);
            let mut right: Vec<Span> = Vec::new();
            if chosen > 0 {
                right.push(Span::styled(format!("{chosen} selected  "), Style::new().fg(ACCENT).bold()));
            }
            right.push(Span::styled(
                if selectable == 0 { "nothing to import".to_string() } else { format!("{selectable} to import") },
                Style::new().fg(DIM),
            ));
            if group.hidden_in_t3 > 0 {
                right.push(Span::styled(format!(" · {} in T3", group.hidden_in_t3), Style::new().fg(DIM)));
            }
            if !app.project_exists(&group.root) {
                right.push(Span::styled(" · new project", Style::new().fg(DIM)));
            }
            right.push(Span::raw(" "));
            let right_w: usize = right.iter().map(|s| s.width()).sum();
            let path_w = width.saturating_sub(right_w + 4).max(10);
            let path = truncate_left(&tilde(&group.root), path_w);
            let gap = width.saturating_sub(2 + path.chars().count() + right_w).max(1);
            let mut spans = vec![
                Span::styled(if group.expanded { "▾ " } else { "▸ " }, Style::new().fg(DIM)),
                Span::styled(path, Style::new().bold()),
                Span::raw(" ".repeat(gap)),
            ];
            spans.extend(right);
            Line::from(spans)
        }
        Row::Session(i) => {
            let s = &app.sessions[i];
            let placement = &app.placements[i];
            let in_t3 = app.in_t3(i);
            let selectable = app.selectable(i);
            let check = if in_t3 {
                Span::styled("  ✓  ", Style::new().fg(Color::Green))
            } else if !selectable {
                Span::styled("  ·  ", Style::new().fg(DIM))
            } else if app.selected.contains(&i) {
                Span::styled("  [x] ", Style::new().fg(ACCENT).bold())
            } else {
                Span::styled("  [ ] ", Style::new().fg(DIM))
            };
            let check = Span::styled(format!("{:<6}", check.content), check.style);

            let mut tags: Vec<Span> = Vec::new();
            if in_t3 {
                tags.push(Span::styled("in T3", Style::new().fg(Color::Green)));
            } else if !s.resumable {
                tags.push(Span::styled("can't be resumed", Style::new().fg(DIM)));
            } else if app.provider_disabled(s.provider) {
                tags.push(Span::styled(format!("{} is off in T3", s.provider.label()), Style::new().fg(Color::Yellow)));
            }
            if placement.worktree_path.is_some() {
                let branch = placement.branch.clone().unwrap_or_else(|| "worktree".into());
                if !tags.is_empty() {
                    tags.push(Span::raw("  "));
                }
                tags.push(Span::styled(format!("⎇ {}", truncate(&branch, 28)), Style::new().fg(DIM)));
            }
            let tags_w: usize = tags.iter().map(|t| t.width()).sum();
            let fixed = 6 + 10 + 11;
            let title_w = width.saturating_sub(fixed + tags_w + 3).max(16);
            let title = truncate(&s.title, title_w);
            let gap = width.saturating_sub(fixed + title.chars().count() + tags_w + 1).max(2);

            let base = if selectable || in_t3 { Style::new() } else { Style::new().fg(DIM) };
            let mut spans = vec![
                check,
                Span::styled(format!("{:<10}", s.provider.label()), Style::new().fg(provider_color(s.provider))),
                Span::styled(format!("{:<11}", ago(s.updated_at)), Style::new().fg(DIM)),
                Span::styled(title, if in_t3 { Style::new().fg(DIM) } else { base }),
                Span::raw(" ".repeat(gap)),
            ];
            spans.extend(tags);
            Line::from(spans)
        }
    }
}

fn kv(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{key:<11}"), Style::new().fg(DIM)), Span::raw(value.into())])
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::new().padding(Padding::horizontal(1));
    let lines: Vec<Line> = match app.current() {
        Some(Row::Session(i)) => {
            let s = &app.sessions[i];
            let placement = &app.placements[i];
            let mut lines = vec![
                Line::from(Span::styled(s.title.clone(), Style::new().bold())),
                Line::raw(""),
                kv("from", s.provider.label()),
                kv("last used", s.updated_at.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()),
                kv("started", s.created_at.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()),
            ];
            if let Some(n) = s.message_count {
                lines.push(kv("messages", n.to_string()));
            }
            if let Some(model) = &s.model {
                lines.push(kv("model", model.clone()));
            }
            lines.push(kv("session", s.session_id.clone()));
            lines.push(Line::raw(""));
            lines.push(kv("goes to", tilde(&placement.workspace_root)));
            if let Some(wt) = &placement.worktree_path {
                let branch = placement.branch.clone().unwrap_or_else(|| "none (detached)".into());
                lines.push(kv("worktree", tilde(wt)));
                lines.push(kv("branch", branch));
                if placement.worktree_missing {
                    lines.push(Line::styled(
                        "The worktree is gone; continuing it runs in the project folder.",
                        Style::new().fg(Color::Yellow),
                    ));
                }
            } else if placement.workspace_root != s.cwd {
                lines.push(kv("ran in", tilde(&s.cwd)));
            }
            lines.push(Line::raw(""));
            if app.in_t3(i) {
                lines.push(Line::styled("Already in T3.", Style::new().fg(Color::Green)));
            } else if !s.resumable {
                lines.push(Line::styled(
                    "This session can't be continued, so it can't be imported.",
                    Style::new().fg(DIM),
                ));
            } else if app.instance_for(s.provider).is_none() {
                lines.push(Line::styled(
                    format!("T3 has no {} provider set up, so this can't be imported.", s.provider.label()),
                    Style::new().fg(DIM),
                ));
            } else if app.provider_disabled(s.provider) {
                lines.push(Line::styled(
                    format!(
                        "{} is turned off in T3. It imports fine, but you continue it only after turning it on in T3's settings.",
                        s.provider.label()
                    ),
                    Style::new().fg(Color::Yellow),
                ));
            } else {
                lines.push(Line::styled("Imports with its full history and continues the same session.", Style::new().fg(DIM)));
            }
            lines
        }
        Some(Row::Group(g)) => {
            let group = &app.groups[g];
            let (chosen, selectable) = app.group_selection(g);
            let mut lines = vec![
                Line::from(Span::styled(tilde(&group.root), Style::new().bold())),
                Line::raw(""),
                if app.project_exists(&group.root) {
                    Line::styled("Already a T3 project.", Style::new().fg(Color::Green))
                } else {
                    Line::styled("Not a T3 project yet; importing creates it.", Style::new().fg(DIM))
                },
                Line::raw(""),
                kv("to import", selectable.to_string()),
            ];
            if group.hidden_in_t3 > 0 {
                lines.push(kv("in T3", format!("{} (hidden, t shows)", group.hidden_in_t3)));
            }
            if chosen > 0 {
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<11}", "selected"), Style::new().fg(DIM)),
                    Span::styled(chosen.to_string(), Style::new().fg(ACCENT).bold()),
                ]));
            }
            lines.push(Line::raw(""));
            for p in Provider::ALL {
                let n = group.sessions.iter().filter(|&&i| app.sessions[i].provider == p).count();
                if n > 0 {
                    lines.push(Line::from(vec![
                        Span::styled(format!("{:<11}", p.label()), Style::new().fg(provider_color(p))),
                        Span::raw(plural(n, "session", "sessions")),
                    ]));
                }
            }
            let worktrees = group.sessions.iter().filter(|&&i| app.placements[i].worktree_path.is_some()).count();
            if worktrees > 0 {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    format!("{} ran in git worktrees and keep their branch.", plural(worktrees, "session", "sessions")),
                    Style::new().fg(DIM),
                ));
            }
            lines
        }
        None => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let [status, keys] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let key = |k: &str| Span::styled(k.to_string(), Style::new().fg(ACCENT).bold());
    let label = |t: &str| Span::styled(format!(" {t}   "), Style::new().fg(DIM));

    let history = app.max_messages.map(|n| format!("newest {n} messages each")).unwrap_or_else(|| "full history".into());
    let total = app.selected.len();
    let status_line = if app.screen != Screen::Picker && app.screen != Screen::Loading {
        Line::raw("")
    } else if total == 0 {
        Line::styled(" Pick sessions with space, then press i to import them.", Style::new().fg(DIM))
    } else {
        let mut per: Vec<String> = Vec::new();
        for p in Provider::ALL {
            let n = app.selected.iter().filter(|&&i| app.sessions[i].provider == p).count();
            if n > 0 {
                per.push(format!("{n} {}", p.label()));
            }
        }
        Line::from(vec![
            Span::styled(format!(" {} selected", plural(total, "session", "sessions")), Style::new().fg(ACCENT).bold()),
            Span::styled(format!("  ({})  ·  {history}  ·  ", per.join(", ")), Style::new().fg(DIM)),
            Span::styled("press i to import", Style::new().fg(ACCENT)),
        ])
    };
    f.render_widget(status_line, status);

    let mut spans = vec![Span::raw(" ")];
    match (app.input, &app.screen) {
        (Input::Search, _) => {
            spans.extend([key("enter"), label("done"), key("esc"), label("clear search")]);
        }
        (Input::MaxMessages, _) => {
            spans.push(Span::raw(format!("Keep only the newest N messages per thread (empty = all): {}▏   ", app.input_buf)));
            spans.extend([key("enter"), label("ok"), key("esc"), label("cancel")]);
        }
        (_, Screen::Picker) => {
            let on_group = matches!(app.current(), Some(Row::Group(_)));
            spans.extend([
                key("space"),
                label(if on_group { "select project" } else { "select" }),
                key("i"),
                label("import"),
                key("↑↓"),
                label("move"),
                key("←→"),
                label("fold"),
                key("/"),
                label("search"),
                key("?"),
                label("all keys"),
                key("q"),
                label("quit"),
            ]);
        }
        (_, Screen::Confirm) if app.closing => {}
        (_, Screen::Confirm) => {
            if app.guard_ok() {
                spans.extend([key("enter"), label("import")]);
            }
            if app.can_close_t3() {
                spans.extend([key("c"), label("close T3")]);
            }
            spans.extend([key("r"), label("check again"), key("esc"), label("back")]);
        }
        (_, Screen::Done) => spans.extend([key("enter"), label("quit")]),
        _ => {}
    }
    f.render_widget(Line::from(spans), keys);
}

fn draw_help(f: &mut Frame) {
    let area = modal(f, "Keys", 64, 24);
    let row = |k: &str, what: &str| {
        Line::from(vec![Span::styled(format!("{k:<14}"), Style::new().fg(ACCENT).bold()), Span::raw(what.to_string())])
    };
    let lines = vec![
        Line::styled("Pick", Style::new().bold()),
        row("space", "select a session, or all of a project"),
        row("a", "select everything shown"),
        row("i", "review and import what you selected"),
        Line::raw(""),
        Line::styled("Move", Style::new().bold()),
        row("↑ ↓  j k", "up and down"),
        row("pgup pgdn", "a page at a time"),
        row("g G", "top, bottom"),
        row("← →  enter", "fold or unfold a project"),
        row("e", "fold or unfold every project"),
        Line::raw(""),
        Line::styled("Show", Style::new().bold()),
        row("1 2 3 4", "Claude, Codex, Grok, OpenCode on or off"),
        row("t", "sessions already in T3 on or off"),
        row("/", "search titles, folders and session ids"),
        row("m", "keep only the newest N messages per thread"),
        Line::raw(""),
        row("q", "quit"),
        Line::raw(""),
        Line::styled("Press any key to close.", Style::new().fg(DIM)),
    ];
    f.render_widget(Paragraph::new(lines), area);
}

fn modal(f: &mut Frame, title: &str, width: u16, height: u16) -> Rect {
    let area = f.area();
    let w = width.min(area.width.saturating_sub(4));
    let h = height.min(area.height.saturating_sub(2));
    let rect = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    f.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(format!(" {title} "), Style::new().bold()))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

fn draw_confirm(f: &mut Frame, app: &App) {
    let area = modal(f, "Import into T3 Code", 84, 26);
    let selected = app.selected_sessions();
    let mut lines = Vec::new();

    let mut per: Vec<String> = Vec::new();
    for p in Provider::ALL {
        let n = selected.iter().filter(|&&i| app.sessions[i].provider == p).count();
        if n > 0 {
            per.push(format!("{n} {}", p.label()));
        }
    }
    lines.push(Line::from(vec![
        Span::styled(format!("{} threads", selected.len()), Style::new().bold()),
        Span::styled(format!("  ({})", per.join(", ")), Style::new().fg(DIM)),
    ]));
    lines.push(kv(
        "messages",
        app.max_messages.map(|n| format!("newest {n} per thread")).unwrap_or_else(|| "complete history".into()),
    ));
    let off: Vec<&str> = Provider::ALL
        .into_iter()
        .filter(|&p| app.provider_disabled(p) && selected.iter().any(|&i| app.sessions[i].provider == p))
        .map(Provider::label)
        .collect();
    let projects = app.projects_to_create();
    if projects.is_empty() {
        lines.push(kv("projects", "all already exist in T3"));
    } else {
        lines.push(kv("projects", format!("{} new:", projects.len())));
        for p in projects.iter().take(6) {
            lines.push(Line::styled(format!("          + {}", tilde(p)), Style::new().fg(DIM)));
        }
        if projects.len() > 6 {
            lines.push(Line::styled(format!("          … and {} more", projects.len() - 6), Style::new().fg(DIM)));
        }
    }
    if !off.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!(
                "{} {} turned off in T3. Their threads import with full history, but you can only continue them after enabling the provider in T3's settings.",
                off.join(" and "),
                if off.len() == 1 { "is" } else { "are" }
            ),
            Style::new().fg(Color::Yellow),
        ));
    }
    lines.push(Line::raw(""));

    match &app.guard {
        _ if app.closing => lines.push(Line::styled(
            format!("{}  Closing T3 Code…", SPINNER[app.spinner % SPINNER.len()]),
            Style::new().fg(Color::Yellow),
        )),
        None => lines.push(Line::styled(
            format!("{}  Checking the T3 database…", SPINNER[app.spinner % SPINNER.len()]),
            Style::new().fg(DIM),
        )),
        Some(Err(e)) => lines.push(Line::styled(format!("Check failed: {e}"), Style::new().fg(Color::Red))),
        Some(Ok(report)) => {
            if !report.t3_running.is_empty() {
                lines.push(Line::styled("T3 Code is still running.", Style::new().fg(Color::Yellow).bold()));
                lines.push(Line::styled(
                    "It uses the same database, so it has to be closed while the import writes.",
                    Style::new().fg(DIM),
                ));
                lines.push(Line::raw(""));
                match &app.running {
                    Some(Ok(running)) if !running.is_empty() => {
                        for r in running {
                            lines.push(Line::from(vec![
                                Span::raw(format!("  • {}", r.name())),
                                Span::styled(format!(": {}", r.plan()), Style::new().fg(DIM)),
                            ]));
                        }
                    }
                    _ => {
                        for p in &report.t3_running {
                            lines.push(Line::styled(format!("  • {}", truncate(p, 70)), Style::new().fg(DIM)));
                        }
                    }
                }
                lines.push(Line::raw(""));
                if let Some(e) = &app.close_error {
                    lines.push(Line::styled(format!("Couldn't close T3: {e}"), Style::new().fg(Color::Red)));
                }
                if app.can_close_t3() {
                    lines.push(Line::from(vec![
                        Span::raw("Press "),
                        Span::styled("c", Style::new().fg(ACCENT).bold()),
                        Span::raw(" to close it now, or close it yourself and press "),
                        Span::styled("r", Style::new().fg(ACCENT).bold()),
                        Span::raw("."),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("Close it, then press "),
                        Span::styled("r", Style::new().fg(ACCENT).bold()),
                        Span::raw(" to check again."),
                    ]));
                }
            }
            if !report.findings.is_empty() {
                lines.push(Line::styled(
                    "This version of T3 Code changed its database. Nothing will be written.",
                    Style::new().fg(Color::Red).bold(),
                ));
                for finding in report.findings.iter().take(5) {
                    lines.push(Line::styled(
                        format!("  {}: expected {}, found {}", finding.location, finding.expected, finding.actual),
                        Style::new().fg(DIM),
                    ));
                }
                if let Some(url) = &report.issue_url {
                    lines.push(Line::raw(""));
                    lines.push(Line::raw("Please report it so t3-import can be updated:"));
                    lines.push(Line::styled(url.clone(), Style::new().fg(ACCENT).underlined()));
                }
            }
            for w in &report.warnings {
                lines.push(Line::styled(format!("note: {w}"), Style::new().fg(DIM)));
            }
            if report.ok() {
                lines.push(Line::styled("T3 is closed and its database checks out.", Style::new().fg(Color::Green)));
                if !app.restart.is_empty() {
                    lines.push(Line::styled(
                        "Its background service starts again when the import ends.",
                        Style::new().fg(DIM),
                    ));
                }
                lines.push(Line::styled(
                    "A backup is taken before anything is written.",
                    Style::new().fg(DIM),
                ));
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::raw("Press "),
                    Span::styled("enter", Style::new().fg(ACCENT).bold()),
                    Span::raw(" to import."),
                ]));
            }
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_importing(f: &mut Frame, app: &App) {
    let area = modal(f, "Importing", 84, 14);
    let height = area.height.saturating_sub(2) as usize;
    let mut lines = vec![Line::styled(
        format!("{}  Working — keep T3 Code closed.", SPINNER[app.spinner % SPINNER.len()]),
        Style::new().bold(),
    )];
    lines.push(Line::raw(""));
    let start = app.log.len().saturating_sub(height);
    lines.extend(app.log[start..].iter().map(|l| Line::styled(truncate(l, 78), Style::new().fg(DIM))));
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_done(f: &mut Frame, app: &App) {
    let area = modal(f, "Done", 84, 22);
    let mut lines = Vec::new();
    match &app.report {
        Some(Ok(r)) => {
            lines.push(Line::styled(
                format!("Imported {} threads.", r.imported.len()),
                Style::new().fg(Color::Green).bold(),
            ));
            if !r.projects_created.is_empty() {
                lines.push(kv("projects", format!("{} created", r.projects_created.len())));
            }
            if !r.skipped.is_empty() {
                lines.push(kv("skipped", r.skipped.len().to_string()));
                for (id, why) in r.skipped.iter().take(5) {
                    lines.push(Line::styled(format!("  {}: {why}", truncate(id, 40)), Style::new().fg(DIM)));
                }
            }
            for w in &r.warnings {
                lines.push(Line::styled(format!("note: {w}"), Style::new().fg(Color::Yellow)));
            }
            if let Some(b) = &r.backup {
                lines.push(Line::raw(""));
                lines.push(kv("backup", tilde(b)));
                lines.push(Line::styled(
                    "To undo, close T3 and copy the backup over statev2.sqlite.",
                    Style::new().fg(DIM),
                ));
            }
            restarted_lines(app, &mut lines);
            lines.push(Line::raw(""));
            lines.push(Line::raw("Open T3 Code to see them."));
        }
        Some(Err(e)) => {
            lines.push(Line::styled("Import failed. No threads were written.", Style::new().fg(Color::Red).bold()));
            lines.push(Line::raw(""));
            lines.push(Line::raw(e.clone()));
            restarted_lines(app, &mut lines);
        }
        None => {}
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn restarted_lines(app: &App, lines: &mut Vec<Line<'static>>) {
    if app.restarted.is_empty() {
        return;
    }
    lines.push(Line::raw(""));
    for r in &app.restarted {
        lines.push(match r {
            Ok(done) => Line::styled(done.clone(), Style::new().fg(Color::Green)),
            Err(failed) => Line::styled(failed.clone(), Style::new().fg(Color::Red)),
        });
    }
}

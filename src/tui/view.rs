use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph, Wrap};

use crate::model::Provider;
use crate::t3db;

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

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(f.area());
    draw_header(f, app, header);
    match app.screen {
        Screen::Loading => draw_loading(f, app, body),
        _ => draw_picker(f, app, body),
    }
    draw_footer(f, app, footer);
    match app.screen {
        Screen::Confirm => draw_confirm(f, app),
        Screen::Importing => draw_importing(f, app),
        Screen::Done => draw_done(f, app),
        _ => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(" t3-import ", Style::new().fg(Color::Black).bg(ACCENT).bold()), Span::raw("  ")];
    match (&app.home, &app.snapshot_error) {
        (Some(home), _) => {
            spans.push(Span::styled("T3 ", Style::new().fg(DIM)));
            spans.push(Span::raw(tilde(&home.base)));
            if let Some(snap) = &app.snapshot {
                spans.push(Span::styled(
                    format!("  ·  {} projects, {} threads", snap.projects.len(), snap.thread_ids.len()),
                    Style::new().fg(DIM),
                ));
            }
        }
        (None, Some(err)) => spans.push(Span::styled(format!("T3 Code not found: {err}"), Style::new().fg(Color::Red))),
        (None, None) => spans.push(Span::styled("reading T3 Code…", Style::new().fg(DIM))),
    }
    f.render_widget(Line::from(spans), area);
}

fn draw_loading(f: &mut Frame, app: &App, area: Rect) {
    let text = format!("{}  Scanning Claude, Codex, Grok and OpenCode sessions…", SPINNER[app.spinner % SPINNER.len()]);
    let [_, mid, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(text).centered().fg(DIM), mid);
}

fn draw_picker(f: &mut Frame, app: &mut App, area: Rect) {
    let wide = area.width >= 110;
    let (list_area, detail_area) = if wide {
        let [l, d] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(44)]).areas(area);
        (l, d)
    } else {
        let [l, d] = Layout::vertical([Constraint::Fill(1), Constraint::Length(9)]).areas(area);
        (l, d)
    };

    let title_width = list_area.width.saturating_sub(44) as usize;
    let items: Vec<ListItem> = app.rows.iter().map(|&row| ListItem::new(row_line(app, row, title_width))).collect();
    let empty = items.is_empty();
    let list = List::new(items)
        .block(Block::new().borders(Borders::RIGHT).border_style(Style::new().fg(DIM)))
        .highlight_style(Style::new().bg(Color::Rgb(45, 45, 52)).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(list, list_area, &mut app.list);
    if empty {
        let msg = if app.sessions.is_empty() { "No sessions found." } else { "Nothing matches the filters." };
        f.render_widget(Paragraph::new(msg).fg(DIM).centered(), list_area);
    }

    draw_detail(f, app, detail_area);
}

fn row_line(app: &App, row: Row, title_width: usize) -> Line<'static> {
    match row {
        Row::Group(g) => {
            let group = &app.groups[g];
            let (chosen, selectable) = app.group_selection(g);
            let mut providers: Vec<Provider> = group.sessions.iter().map(|&i| app.sessions[i].provider).collect();
            providers.sort();
            providers.dedup();
            let mut spans = vec![
                Span::styled(if group.expanded { "▾ " } else { "▸ " }, Style::new().fg(DIM)),
                Span::styled(tilde(&group.cwd), Style::new().bold()),
                Span::styled(format!("  {} ", group.sessions.len()), Style::new().fg(DIM)),
            ];
            for p in providers {
                spans.push(Span::styled("● ", Style::new().fg(provider_color(p))));
            }
            if chosen > 0 {
                spans.push(Span::styled(format!(" {chosen}/{selectable} selected"), Style::new().fg(ACCENT)));
            }
            Line::from(spans)
        }
        Row::Session(i) => {
            let s = &app.sessions[i];
            let in_t3 = app.in_t3(i);
            let selectable = app.selectable(i);
            let check = if in_t3 {
                Span::styled("  ✓ ", Style::new().fg(Color::Green))
            } else if !selectable {
                Span::styled("  – ", Style::new().fg(DIM))
            } else if app.selected.contains(&i) {
                Span::styled("  ◉ ", Style::new().fg(ACCENT).bold())
            } else {
                Span::styled("  ○ ", Style::new().fg(DIM))
            };
            let base = if selectable { Style::new() } else { Style::new().fg(DIM) };
            let msgs = s.message_count.map(|n| format!("{n:>5} msg")).unwrap_or_else(|| "         ".into());
            let mut spans = vec![
                check,
                Span::styled(format!("{:<9}", s.provider.label()), Style::new().fg(provider_color(s.provider))),
                Span::styled(format!("{}  ", s.updated_at.format("%Y-%m-%d")), Style::new().fg(DIM)),
                Span::styled(format!("{msgs}  "), Style::new().fg(DIM)),
                Span::styled(truncate(&s.title, title_width.max(20)), base),
            ];
            if in_t3 {
                spans.push(Span::styled("  in T3", Style::new().fg(Color::Green)));
            } else if !s.resumable {
                spans.push(Span::styled("  not resumable", Style::new().fg(DIM)));
            } else if app.provider_disabled(s.provider) {
                spans.push(Span::styled(format!("  {} off in T3", s.provider.label()), Style::new().fg(Color::Yellow)));
            }
            Line::from(spans)
        }
    }
}

fn kv(key: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{key:<10}"), Style::new().fg(DIM)), Span::raw(value.into())])
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::new().padding(Padding::horizontal(1));
    let lines: Vec<Line> = match app.current() {
        Some(Row::Session(i)) => {
            let s = &app.sessions[i];
            let placement = t3db::resolve_placement(&s.cwd);
            let mut lines = vec![
                Line::from(Span::styled(s.title.clone(), Style::new().bold())),
                Line::raw(""),
                kv("provider", s.provider.label()),
                kv("session", s.session_id.clone()),
                kv("updated", s.updated_at.format("%Y-%m-%d %H:%M").to_string()),
                kv("created", s.created_at.format("%Y-%m-%d %H:%M").to_string()),
                kv("messages", s.message_count.map(|n| n.to_string()).unwrap_or_else(|| "?".into())),
            ];
            if let Some(model) = &s.model {
                lines.push(kv("model", model.clone()));
            }
            lines.push(kv("project", tilde(&placement.workspace_root)));
            if let Some(wt) = &placement.worktree_path {
                let branch = placement.branch.clone().unwrap_or_else(|| "detached".into());
                lines.push(kv("worktree", format!("{} ({branch})", tilde(wt))));
                if placement.worktree_missing {
                    lines.push(Line::styled(
                        "worktree is gone: resuming runs in the project root",
                        Style::new().fg(Color::Yellow),
                    ));
                }
            }
            lines
        }
        Some(Row::Group(g)) => {
            let group = &app.groups[g];
            let placement = t3db::resolve_placement(&group.cwd);
            let exists = app
                .snapshot
                .as_ref()
                .is_some_and(|s| s.projects.iter().any(|p| p.workspace_root == placement.workspace_root));
            let mut lines = vec![
                Line::from(Span::styled(tilde(&group.cwd), Style::new().bold())),
                Line::raw(""),
                kv("sessions", group.sessions.len().to_string()),
            ];
            for p in Provider::ALL {
                let n = group.sessions.iter().filter(|&&i| app.sessions[i].provider == p).count();
                if n > 0 {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {:<8}", p.label()), Style::new().fg(provider_color(p))),
                        Span::raw(n.to_string()),
                    ]));
                }
            }
            lines.push(Line::raw(""));
            lines.push(kv("project", tilde(&placement.workspace_root)));
            lines.push(if exists {
                Line::styled("already a T3 project", Style::new().fg(Color::Green))
            } else {
                Line::styled("will be added as a T3 project", Style::new().fg(DIM))
            });
            lines
        }
        None => Vec::new(),
    };
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: true }), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let [status, keys] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let mut spans = vec![Span::raw(" ")];
    for (n, p) in Provider::ALL.into_iter().enumerate() {
        let on = app.providers.contains(&p);
        let style = if on { Style::new().fg(provider_color(p)) } else { Style::new().fg(DIM).crossed_out() };
        spans.push(Span::styled(format!("{} {}", n + 1, p.label()), style));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled("messages: ", Style::new().fg(DIM)));
    spans.push(Span::raw(app.max_messages.map(|n| format!("newest {n}")).unwrap_or_else(|| "all".into())));
    if !app.search.is_empty() {
        spans.push(Span::styled("   search: ", Style::new().fg(DIM)));
        spans.push(Span::raw(app.search.clone()));
    }
    let total = app.selected.len();
    if total > 0 {
        spans.push(Span::styled(format!("   {total} selected"), Style::new().fg(ACCENT).bold()));
    }
    f.render_widget(Line::from(spans), status);

    let hint = match (app.input, &app.screen) {
        (Input::Search, _) => format!(" search: {}▏  enter keep · esc clear", app.input_buf),
        (Input::MaxMessages, _) => {
            format!(" newest messages to keep per thread (empty = all): {}▏  enter ok · esc cancel", app.input_buf)
        }
        (_, Screen::Picker) => {
            " space select · a all · enter/→ open · ← close · e expand all · / search · 1-4 providers · m messages · i import · q quit".into()
        }
        (_, Screen::Confirm) => " enter import · r check again · esc back".into(),
        (_, Screen::Done) => " enter quit".into(),
        _ => String::new(),
    };
    f.render_widget(Line::styled(hint, Style::new().fg(DIM)), keys);
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
        None => lines.push(Line::styled(
            format!("{}  Checking the T3 database…", SPINNER[app.spinner % SPINNER.len()]),
            Style::new().fg(DIM),
        )),
        Some(Err(e)) => lines.push(Line::styled(format!("Check failed: {e}"), Style::new().fg(Color::Red))),
        Some(Ok(report)) => {
            if !report.t3_running.is_empty() {
                lines.push(Line::styled("Close T3 Code first.", Style::new().fg(Color::Yellow).bold()));
                lines.push(Line::styled(
                    "The import writes to its database, so the app, the server and the background service must all be stopped.",
                    Style::new().fg(DIM),
                ));
                for p in &report.t3_running {
                    lines.push(Line::styled(format!("  running: {}", truncate(p, 70)), Style::new().fg(DIM)));
                }
                lines.push(Line::styled("Press r to check again.", Style::new().fg(DIM)));
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
            lines.push(Line::raw(""));
            lines.push(Line::raw("Open T3 Code to see them."));
        }
        Some(Err(e)) => {
            lines.push(Line::styled("Import failed. No threads were written.", Style::new().fg(Color::Red).bold()));
            lines.push(Line::raw(""));
            lines.push(Line::raw(e.clone()));
        }
        None => {}
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

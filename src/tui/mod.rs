//! Interactive picker: scan, choose sessions, confirm, import.

mod app;
mod view;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::scan::Roots;

use app::{App, Msg};

pub fn run(roots: Roots, t3_home: Option<PathBuf>) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut app = App::new(roots, t3_home, tx);
    app.start_scan();

    let mut terminal = ratatui::init();
    let result = (|| -> anyhow::Result<()> {
        loop {
            while let Ok(msg) = rx.try_recv() {
                app.on_msg(msg);
            }
            terminal.draw(|f| view::draw(f, &mut app))?;
            if app.should_quit {
                return Ok(());
            }
            if event::poll(Duration::from_millis(80))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key);
                    }
                }
            }
            app.tick();
        }
    })();
    ratatui::restore();
    result
}

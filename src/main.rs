pub use t3_import::*;
mod gui;

fn main() -> anyhow::Result<()> {
    t3_import::run_cli(Some(gui::run))
}

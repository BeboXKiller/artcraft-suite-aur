//! artcraft-suite-aur: TUI to install, update and launch the Craft App Suite from source.

mod app;
mod engine;
mod ui;

use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("artcraft-suite-aur {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--status") {
        for a in engine::scan() {
            println!(
                "{}|{}|{}|{}|{}",
                a.repo,
                if a.installed { "installed" } else if a.cloned { "cloned" } else { "missing" },
                a.pkg.as_deref().unwrap_or("-"),
                a.version.as_deref().unwrap_or("-"),
                a.dir.display(),
            );
        }
        return;
    }
    if let Err(e) = run() {
        eprintln!("artcraft-suite-aur: {e}");
        std::process::exit(1);
    }
}

fn run() -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut app = app::App::new(tx);
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut app::App,
    rx: &mpsc::Receiver<engine::Ev>,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        app.drain(rx);
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    app.on_key(k.code);
                }
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}

//! artcraft-suite-aur: TUI to install, update and launch the Craft App Suite from source.

mod app;
mod engine;
mod theme;
mod ui;

use std::io::IsTerminal;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

const HELP: &str = "\
artcraft-suite-aur — install, update and launch the Craft App Suite from source

Usage:
  artcraft-suite-aur             Start the interactive UI (requires a terminal)
  artcraft-suite-aur --status    One line per app: repo|state|pkg|version|dir
  artcraft-suite-aur --version   Print the version and exit
  artcraft-suite-aur --help      Show this help

Inside the UI press ? for the full key reference.

Requirements: a TTY, UTF-8, at least 60x15 columns.
Exit codes: 0 ok, 1 runtime error, 2 bad usage.";

const KNOWN_ARGS: [&str; 5] = ["--help", "-h", "--version", "-V", "--status"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(bad) = args.iter().find(|a| !KNOWN_ARGS.contains(&a.as_str())) {
        eprintln!("artcraft-suite-aur: unknown argument '{bad}'");
        eprintln!("Try 'artcraft-suite-aur --help'");
        std::process::exit(2);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_line(HELP);
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        print_line(&format!("artcraft-suite-aur {}", env!("CARGO_PKG_VERSION")));
        return;
    }
    if args.iter().any(|a| a == "--status") {
        print_status();
        return;
    }
    // Never force the fullscreen UI into a pipe, CI log or a dumb terminal:
    // degrade to plain output instead.
    let dumb = std::env::var("TERM").is_ok_and(|t| t == "dumb");
    if dumb || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        print_status();
        if dumb {
            eprintln!("artcraft-suite-aur: TERM=dumb cannot host the full-screen UI.");
        } else {
            eprintln!("artcraft-suite-aur: no terminal detected — printed status instead.");
        }
        eprintln!("Run it inside a terminal for the interactive UI.");
        return;
    }
    if let Err(e) = run() {
        eprintln!("artcraft-suite-aur: {e}");
        std::process::exit(1);
    }
}

fn print_status() {
    for a in engine::scan() {
        let state = if a.installed {
            "installed"
        } else if a.cloned {
            "cloned"
        } else {
            "missing"
        };
        print_line(&format!(
            "{}|{state}|{}|{}|{}",
            a.repo,
            a.pkg.as_deref().unwrap_or("-"),
            a.version.as_deref().unwrap_or("-"),
            a.dir.display(),
        ));
    }
}

/// Write to stdout ignoring errors: `--help | head` closes the pipe early, and
/// `println!` panics on that instead of exiting quietly.
fn print_line(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
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
        app.drain(rx);
        app.refresh_log();
        terminal.draw(|f| ui::draw(f, app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
        {
            app.on_key(k);
        }
        if app.quit {
            return Ok(());
        }
    }
}

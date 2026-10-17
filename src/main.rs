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
  artcraft-suite-aur -p N        Start with N parallel builds (default 2)
  artcraft-suite-aur --releases  Track release tags, not every commit
  artcraft-suite-aur --status    One line per app: repo|state|pkg|version|dir
  artcraft-suite-aur --version   Print the version and exit
  artcraft-suite-aur --help      Show this help

-p / --parallel N (or ARTCRAFT_PARALLEL) sets how many apps compile at once,
1 to 16 — low-spec machines should stick to 1 or 2.

--releases (or ARTCRAFT_TRACK=releases) measures \"behind\" in release tags
instead of commits, and updates land on the newest release — for people who
want stable releases only. Inside the UI press t to toggle.

Inside the UI press ? for the full key reference.

Requirements: a TTY, UTF-8, at least 60x15 columns.
Exit codes: 0 ok, 1 runtime error, 2 bad usage.";

/// How many apps may compile at once by default (override with `-p`).
const DEFAULT_PARALLEL: usize = app::DEFAULT_MAX_PARALLEL;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parallel = match parallel_from(&args, std::env::var("ARTCRAFT_PARALLEL").ok()) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("Try 'artcraft-suite-aur --help'");
            std::process::exit(2);
        }
    };
    let track = match track_from(&args, std::env::var("ARTCRAFT_TRACK").ok()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("Try 'artcraft-suite-aur --help'");
            std::process::exit(2);
        }
    };
    if let Some(bad) = unknown_arg(&args) {
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
    if let Err(e) = run(parallel, track) {
        eprintln!("artcraft-suite-aur: {e}");
        std::process::exit(1);
    }
}

/// `-p N`, `--parallel N` or `--parallel=N`; else the `ARTCRAFT_PARALLEL`
/// value; else the default. The flag always wins over the environment.
fn parallel_from(args: &[String], env: Option<String>) -> Result<usize, String> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "-p" || a == "--parallel" {
            let Some(v) = args.get(i + 1) else {
                return Err(format!(
                    "artcraft-suite-aur: {a} needs a value ({}-{})",
                    app::MIN_JOBS,
                    app::MAX_JOBS
                ));
            };
            return parse_parallel(v);
        }
        if let Some(v) = a.strip_prefix("--parallel=") {
            return parse_parallel(v);
        }
        i += 1;
    }
    match env {
        Some(v) if !v.is_empty() => parse_parallel(&v),
        _ => Ok(DEFAULT_PARALLEL),
    }
}

fn parse_parallel(v: &str) -> Result<usize, String> {
    let n: usize = v.trim().parse().map_err(|_| {
        format!(
            "artcraft-suite-aur: bad parallel value '{v}' (want {}-{})",
            app::MIN_JOBS,
            app::MAX_JOBS
        )
    })?;
    if !(app::MIN_JOBS..=app::MAX_JOBS).contains(&n) {
        return Err(format!(
            "artcraft-suite-aur: parallel value {n} out of range ({}-{})",
            app::MIN_JOBS,
            app::MAX_JOBS
        ));
    }
    Ok(n)
}

/// `--releases`; else the `ARTCRAFT_TRACK` value; else commits. The flag
/// always wins over the environment, like `-p`.
fn track_from(args: &[String], env: Option<String>) -> Result<engine::Track, String> {
    if args.iter().any(|a| a == "--releases") {
        return Ok(engine::Track::Releases);
    }
    match env {
        Some(v) if !v.is_empty() => parse_track(&v),
        _ => Ok(engine::Track::Commits),
    }
}

fn parse_track(v: &str) -> Result<engine::Track, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "releases" | "release" | "tags" => Ok(engine::Track::Releases),
        "commits" | "commit" => Ok(engine::Track::Commits),
        _ => Err(format!(
            "artcraft-suite-aur: bad ARTCRAFT_TRACK value '{v}' (want releases or commits)"
        )),
    }
}

/// The first argument nothing knows about, if any (`-p`/`--parallel` values
/// are skipped — they are validated by [`parallel_from`] first).
fn unknown_arg(args: &[String]) -> Option<&str> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--help" | "-h" | "--version" | "-V" | "--status" | "--releases" => i += 1,
            "-p" | "--parallel" => i += 2,
            _ if a.starts_with("--parallel=") => i += 1,
            _ => return Some(a),
        }
    }
    None
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

fn run(parallel: usize, track: engine::Track) -> std::io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut app = app::App::new(tx, parallel, track);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parallel_flag_short_long_and_equals() {
        assert_eq!(parallel_from(&a(&["-p", "4"]), None).unwrap(), 4);
        assert_eq!(parallel_from(&a(&["--parallel", "8"]), None).unwrap(), 8);
        assert_eq!(parallel_from(&a(&["--parallel=3"]), None).unwrap(), 3);
        assert_eq!(
            parallel_from(&a(&["--status", "-p", "5"]), None).unwrap(),
            5
        );
    }

    #[test]
    fn parallel_flag_beats_the_environment() {
        assert_eq!(
            parallel_from(&a(&["-p", "1"]), Some("9".into())).unwrap(),
            1
        );
        assert_eq!(parallel_from(&a(&[]), Some("3".into())).unwrap(), 3);
        assert_eq!(parallel_from(&a(&[]), None).unwrap(), DEFAULT_PARALLEL);
        assert_eq!(
            parallel_from(&a(&[]), Some("".into())).unwrap(),
            DEFAULT_PARALLEL
        );
    }

    #[test]
    fn parallel_rejects_missing_and_bad_values() {
        assert!(parallel_from(&a(&["-p"]), None).is_err());
        assert!(parallel_from(&a(&["-p", "0"]), None).is_err());
        assert!(parallel_from(&a(&["-p", "17"]), None).is_err());
        assert!(parallel_from(&a(&["-p", "lots"]), None).is_err());
        assert!(parallel_from(&a(&["--parallel="]), None).is_err());
    }

    #[test]
    fn unknown_arg_still_finds_stragglers_and_skips_values() {
        assert_eq!(unknown_arg(&a(&["-p", "4"])), None);
        assert_eq!(unknown_arg(&a(&["--parallel=4", "--status"])), None);
        assert_eq!(unknown_arg(&a(&["--releases", "--status"])), None);
        assert_eq!(unknown_arg(&a(&["--bogus"])), Some("--bogus"));
    }

    #[test]
    fn releases_flag_beats_the_track_environment() {
        assert_eq!(
            track_from(&a(&["--releases"]), Some("commits".into())).unwrap(),
            engine::Track::Releases
        );
        assert_eq!(
            track_from(&a(&[]), Some("releases".into())).unwrap(),
            engine::Track::Releases
        );
        assert_eq!(
            track_from(&a(&[]), Some("commits".into())).unwrap(),
            engine::Track::Commits
        );
        assert_eq!(track_from(&a(&[]), None).unwrap(), engine::Track::Commits);
        assert_eq!(
            track_from(&a(&[]), Some("".into())).unwrap(),
            engine::Track::Commits
        );
    }

    #[test]
    fn track_env_rejects_junk() {
        assert!(track_from(&a(&[]), Some("stable".into())).is_err());
    }
}

//! Application state: selection, background tasks, key handling.

use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(test)]
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::engine::{self, Action, AppInfo, Ev};
use crate::theme::Palette;

const MAX_PARALLEL: usize = 2;
/// Rows a PageUp/PageDown jumps; close to a typical list viewport height.
const PAGE: usize = 10;
/// Bytes of build log cached for the tail pane.
const LOG_BYTES: u64 = 4096;

/// The status line's tone, derived from the message prefix (`✓` / `✗`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Bad,
    Busy,
}

pub struct App {
    pub apps: Vec<AppInfo>,
    /// highlighted row — the fallback target when nothing is picked
    pub sel: usize,
    /// explicitly picked rows (`Space` / `Ctrl+Space`); empty means "use `sel`"
    pub picks: HashSet<usize>,
    pub busy: HashMap<String, String>,
    pub pending: VecDeque<(String, Action)>,
    /// repo → error of the last failed task, until a retry clears it
    pub failed: HashMap<String, String>,
    /// `o`: show only apps that need attention
    pub filter_attention: bool,
    pub msg: String,
    pub quit: bool,
    /// `?` overlay
    pub show_help: bool,
    /// scroll offset inside the help overlay (it is longer than a small screen)
    pub help_scroll: usize,
    /// brand palette for this terminal (see [`crate::theme`])
    pub palette: Palette,
    /// second `q` required while tasks are running
    confirm_quit: bool,
    /// Tail of the selected repo's log, refreshed by [`Self::refresh_log`].
    pub log: Vec<String>,
    log_repo: &'static str,
    log_len: u64,
    tx: Sender<Ev>,
}

impl App {
    pub fn new(tx: Sender<Ev>) -> Self {
        let mut app = Self {
            apps: engine::scan(),
            sel: 0,
            picks: HashSet::new(),
            busy: HashMap::new(),
            pending: VecDeque::new(),
            failed: HashMap::new(),
            filter_attention: false,
            msg: "ready".into(),
            quit: false,
            show_help: false,
            help_scroll: 0,
            palette: Palette::detect(),
            confirm_quit: false,
            log: Vec::new(),
            log_repo: "",
            log_len: 0,
            tx,
        };
        app.refresh_log();
        app
    }

    pub fn drain(&mut self, rx: &Receiver<Ev>) {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Ev::Done {
                    repo,
                    action,
                    result,
                } => {
                    self.busy.remove(&repo);
                    let text = match result {
                        Ok(m) => {
                            self.failed.remove(&repo);
                            format!("✓ {m}")
                        }
                        Err(e) => {
                            self.failed.insert(repo.clone(), e.clone());
                            format!("✗ {repo} · {}: {e}", action.label())
                        }
                    };
                    self.msg = text;
                    if let Some(idx) = self.apps.iter().position(|a| a.repo == repo) {
                        self.rescan_at(idx);
                    }
                    self.pump();
                }
                Ev::Scanned { index, info } => {
                    // the suite order is fixed, but guard against a stale index anyway
                    if let Some(slot) = self.apps.get_mut(index)
                        && slot.repo == info.repo
                    {
                        *slot = info;
                    }
                }
                Ev::Refreshed => self.msg = "refreshed".into(),
            }
        }
        // a rescan can take the highlighted row out of the attention filter
        self.clamp_selection();
    }

    /// Cache the log tail for the selected repo.
    ///
    /// Called once per loop: a `stat` per tick, re-read only when the file or the
    /// selection changed, so rendering stays a pure function of state.
    pub fn refresh_log(&mut self) {
        let repo = self.apps.get(self.sel).map_or("", |a| a.repo);
        let len = std::fs::metadata(engine::log_file(repo))
            .map(|m| m.len())
            .unwrap_or(0);
        if repo == self.log_repo && len == self.log_len {
            return;
        }
        self.log_repo = repo;
        self.log_len = len;
        self.log = engine::log_tail(repo, LOG_BYTES)
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default();
    }

    /// What the status row says, and how loud it should look.
    pub fn status_line(&self) -> (String, Tone) {
        let mut parts = Vec::new();
        if let Some(s) = engine::busy_summary(&self.busy) {
            parts.push(s);
        }
        if !self.pending.is_empty() {
            parts.push(format!("{} queued", self.pending.len()));
        }
        if !parts.is_empty() {
            return (parts.join(" · "), Tone::Busy);
        }
        let tone = if self.msg.starts_with('✗') {
            Tone::Bad
        } else if self.msg.starts_with('✓') {
            Tone::Good
        } else {
            Tone::Info
        };
        (self.msg.clone(), tone)
    }

    fn try_start(&mut self, repo: String, action: Action) {
        if self.busy.contains_key(&repo) || self.pending.iter().any(|(r, _)| *r == repo) {
            return;
        }
        if self.busy.len() >= MAX_PARALLEL {
            self.pending.push_back((repo, action));
            return;
        }
        self.failed.remove(&repo);
        self.busy.insert(repo.clone(), action.label().into());
        engine::spawn(repo, action, self.tx.clone());
    }

    fn pump(&mut self) {
        while let Some((repo, action)) = self.pending.pop_front() {
            if self.busy.contains_key(&repo) || self.busy.len() >= MAX_PARALLEL {
                self.pending.push_front((repo, action));
                break;
            }
            self.failed.remove(&repo);
            self.busy.insert(repo.clone(), action.label().into());
            engine::spawn(repo, action, self.tx.clone());
        }
    }

    /// Re-read one entry from disk on a worker thread.
    fn rescan_at(&self, index: usize) {
        let Some(cur) = self.apps.get(index) else {
            return;
        };
        let mut info = cur.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            engine::rescan_one(&mut info);
            let _ = tx.send(Ev::Scanned { index, info });
        });
    }

    /// Re-read every entry from disk on one worker thread, reporting progressively.
    fn rescan_all(&self) {
        let mut infos = self.apps.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for (index, info) in infos.iter_mut().enumerate() {
                engine::rescan_one(info);
                let _ = tx.send(Ev::Scanned {
                    index,
                    info: info.clone(),
                });
            }
            let _ = tx.send(Ev::Refreshed);
        });
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        use KeyCode::*;
        // Ctrl chords are never aliases for their bare letters: Ctrl+C quits,
        // the rest are left to the terminal.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                Char('c') | Char('d') => self.quit = true,
                // terminals send Ctrl+Space as NUL, decoded as Char(' ') + CONTROL
                Char(' ') => self.pick_toggle(),
                _ => {}
            }
            self.confirm_quit = false;
            return;
        }
        if key.code == Char('?') {
            self.show_help = !self.show_help;
            if self.show_help {
                self.help_scroll = 0;
            }
            self.confirm_quit = false;
            return;
        }
        if self.show_help {
            match key.code {
                Esc | Char('q') | Enter => self.show_help = false,
                Up | Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                Down | Char('j') => self.help_scroll = self.help_scroll.saturating_add(1),
                PageUp => self.help_scroll = self.help_scroll.saturating_sub(PAGE),
                PageDown => self.help_scroll = self.help_scroll.saturating_add(PAGE),
                Home | Char('g') => self.help_scroll = 0,
                _ => {}
            }
            return;
        }
        if !matches!(key.code, Char('q') | Esc) {
            self.confirm_quit = false;
        }
        match key.code {
            Char('q') | Esc => self.request_quit(),
            Up | Char('k') => self.move_sel(-1),
            Down | Char('j') => self.move_sel(1),
            PageUp => self.move_sel(-(PAGE as isize)),
            PageDown => self.move_sel(PAGE as isize),
            Home | Char('g') => {
                self.sel = self.visible_rows().first().copied().unwrap_or(0);
            }
            End | Char('G') => {
                self.sel = self.visible_rows().last().copied().unwrap_or(0);
            }
            Enter => self.enter(),
            Char(' ') => self.pick_one(),
            Char('u') => self.start_action(Action::Update),
            Char('i') => self.start_action(Action::Install),
            Char('f') => self.start_action(Action::Fetch),
            Char('x') | Delete => self.cancel_selected(),
            Char('U') => {
                let repos: Vec<String> = self
                    .apps
                    .iter()
                    .filter(|a| a.cloned)
                    .map(|a| a.repo.to_string())
                    .collect();
                let before = self.busy.len() + self.pending.len();
                for repo in repos {
                    self.try_start(repo, Action::Update);
                }
                self.msg = if self.busy.len() + self.pending.len() > before {
                    "update queued".into()
                } else {
                    "nothing to update".into()
                };
            }
            Char('a') => {
                let repos: Vec<String> = self
                    .apps
                    .iter()
                    .filter(|a| !a.installed)
                    .map(|a| a.repo.to_string())
                    .collect();
                let before = self.busy.len() + self.pending.len();
                for repo in repos {
                    self.try_start(repo, Action::Install);
                }
                self.msg = if self.busy.len() + self.pending.len() > before {
                    "install queued".into()
                } else {
                    "nothing to install".into()
                };
            }
            Char('r') => {
                self.rescan_all();
                self.msg = "refreshing…".into();
            }
            Char('o') => {
                self.filter_attention = !self.filter_attention;
                self.clamp_selection();
                self.msg = if !self.filter_attention {
                    "filter cleared".into()
                } else {
                    let shown = self.visible_rows().len();
                    if shown == 0 {
                        "✓ nothing needs attention".into()
                    } else {
                        format!("filter: needs attention ({shown} shown)")
                    }
                };
            }
            _ => {}
        }
    }

    /// Rows the list currently shows: all of them, or just the ones needing attention.
    pub fn visible_rows(&self) -> Vec<usize> {
        if !self.filter_attention {
            return (0..self.apps.len()).collect();
        }
        self.apps
            .iter()
            .enumerate()
            .filter(|(_, a)| self.needs_attention(a))
            .map(|(i, _)| i)
            .collect()
    }

    /// Missing, behind upstream, or failed — the rows `o` keeps.
    pub fn needs_attention(&self, a: &AppInfo) -> bool {
        !a.installed || a.behind.is_some_and(|n| n > 0) || self.failed.contains_key(a.repo)
    }

    /// Step the highlight by `step` rows *within what is visible*.
    fn move_sel(&mut self, step: isize) {
        let vis = self.visible_rows();
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|&i| i == self.sel).unwrap_or(0) as isize;
        let next = (pos + step).clamp(0, vis.len() as isize - 1) as usize;
        self.sel = vis[next];
    }

    /// Keep the highlight on a row the list is actually showing.
    fn clamp_selection(&mut self) {
        let vis = self.visible_rows();
        if vis.is_empty() {
            self.filter_attention = false;
            self.sel = self.sel.min(self.apps.len().saturating_sub(1));
            return;
        }
        if !vis.contains(&self.sel) {
            self.sel = vis
                .iter()
                .copied()
                .find(|&i| i >= self.sel)
                .unwrap_or_else(|| *vis.last().unwrap());
        }
    }

    fn request_quit(&mut self) {
        let running = self.busy.len() + self.pending.len();
        if running > 0 && !self.confirm_quit {
            self.confirm_quit = true;
            self.msg = format!(
                "{running} task(s) still running — press q again to quit (they keep running)"
            );
        } else {
            self.quit = true;
        }
    }

    /// `Space`: pick just this row; pressing it again on the only pick clears it.
    fn pick_one(&mut self) {
        if self.picks.len() == 1 && self.picks.contains(&self.sel) {
            self.picks.clear();
            self.msg = "picks cleared".into();
        } else {
            self.picks.clear();
            self.picks.insert(self.sel);
            self.msg = format!("picked {}", self.row_label(self.sel));
        }
    }

    /// `Ctrl+Space`: add or remove this row from the picked set.
    fn pick_toggle(&mut self) {
        let label = self.row_label(self.sel);
        if self.picks.remove(&self.sel) {
            self.msg = format!("unpicked {label} ({} left)", self.picks.len());
        } else {
            self.picks.insert(self.sel);
            self.msg = format!("picked {label} ({} total)", self.picks.len());
        }
    }

    fn row_label(&self, idx: usize) -> String {
        self.apps
            .get(idx)
            .map(|a| a.repo.to_string())
            .unwrap_or_else(|| "?".into())
    }

    /// Rows an operation applies to: the picked set, else the highlighted row.
    fn targets(&self) -> Vec<usize> {
        if self.picks.is_empty() {
            vec![self.sel]
        } else {
            let mut v: Vec<usize> = self.picks.iter().copied().collect();
            v.sort_unstable();
            v
        }
    }

    fn is_staged(&self, repo: &str) -> bool {
        self.busy.contains_key(repo) || self.pending.iter().any(|(r, _)| r == repo)
    }

    fn cancel_selected(&mut self) {
        if self.apps.is_empty() {
            return;
        }
        let targets = self.targets();
        let mut cancelling = 0usize;
        let mut dequeued = 0usize;
        let mut finished = 0usize;
        let mut idle = 0usize;
        for i in &targets {
            let Some(repo) = self.apps.get(*i).map(|a| a.repo) else {
                continue;
            };
            if self.busy.contains_key(repo) {
                if engine::cancel(repo) {
                    cancelling += 1;
                } else {
                    finished += 1;
                }
            } else if let Some(pos) = self.pending.iter().position(|(r, _)| r == repo) {
                self.pending.remove(pos);
                dequeued += 1;
            } else {
                idle += 1;
            }
        }
        self.msg = if targets.len() == 1 {
            let repo = self.row_label(targets[0]);
            if cancelling == 1 {
                format!("cancelling {repo}")
            } else if dequeued == 1 {
                format!("{repo} dequeued")
            } else if finished == 1 {
                format!("{repo} already finished")
            } else {
                format!("{repo} is idle — nothing to cancel")
            }
        } else if cancelling + dequeued + finished == 0 {
            format!("nothing to cancel ({idle} idle)")
        } else {
            let mut parts = Vec::new();
            if cancelling > 0 {
                parts.push(format!("{cancelling} cancelling"));
            }
            if dequeued > 0 {
                parts.push(format!("{dequeued} dequeued"));
            }
            if finished > 0 {
                parts.push(format!("{finished} already finished"));
            }
            parts.join(" · ")
        };
    }

    fn enter(&mut self) {
        if self.apps.is_empty() {
            return;
        }
        let targets = self.targets();
        if targets.len() > 1 {
            self.enter_many(&targets);
            return;
        }
        let Some(a) = self.apps.get(targets[0]) else {
            return;
        };
        let repo = a.repo;
        let installed = a.installed;
        let pkg = a.pkg.clone();
        if self.busy.contains_key(repo) {
            self.msg = format!("{} already running for {repo}", self.busy[repo]);
            return;
        }
        if self.pending.iter().any(|(r, _)| r == repo) {
            self.msg = format!("{repo} is queued — wait, or press x to dequeue");
            return;
        }
        if installed {
            let pkg = pkg.unwrap_or_else(|| repo.to_string());
            self.msg = match engine::launch(&pkg) {
                Ok(()) => format!("launched {pkg}"),
                Err(e) => e,
            };
        } else {
            self.start_action(Action::Install);
        }
    }

    /// Enter on a multi-pick: install what's missing, skip what's installed.
    /// Launching a pile of GUI apps would surprise the user and stall the UI.
    fn enter_many(&mut self, targets: &[usize]) {
        let before = self.busy.len() + self.pending.len();
        let mut installed = 0usize;
        for i in targets {
            let Some((is_installed, repo)) =
                self.apps.get(*i).map(|a| (a.installed, a.repo.to_string()))
            else {
                continue;
            };
            if is_installed {
                installed += 1;
                continue;
            }
            if self.is_staged(&repo) {
                continue;
            }
            self.try_start(repo, Action::Install);
        }
        let queued = (self.busy.len() + self.pending.len()).saturating_sub(before);
        self.msg = match (queued, installed) {
            (0, n) if n == targets.len() => format!("all {n} already installed"),
            (0, _) => "already queued or installed".into(),
            (q, n) if n > 0 => format!("{q} install queued · {n} already installed"),
            (q, _) => format!("{q} install queued"),
        };
    }

    /// Run `action` against the picked set, or the highlighted row when nothing
    /// is picked. Busy/queued rows are left alone by `try_start`.
    fn start_action(&mut self, action: Action) {
        if self.apps.is_empty() {
            self.msg = "no apps".into();
            return;
        }
        let targets = self.targets();
        let before = self.busy.len() + self.pending.len();
        // a single target that was already running before we touched it
        let was_running = (targets.len() == 1)
            .then(|| self.row_label(targets[0]))
            .is_some_and(|repo| self.busy.contains_key(&repo));
        for i in &targets {
            let Some(repo) = self.apps.get(*i).map(|a| a.repo.to_string()) else {
                continue;
            };
            self.try_start(repo, action);
        }
        let added = (self.busy.len() + self.pending.len()).saturating_sub(before);
        self.msg = if targets.len() == 1 {
            let repo = self.row_label(targets[0]);
            if self.busy.contains_key(&repo) && !was_running {
                format!("{} {repo}", action.label())
            } else if self.busy.contains_key(&repo) {
                format!("{} already running for {repo}", self.busy[&repo])
            } else {
                format!("{repo} queued")
            }
        } else {
            match (targets.len(), added) {
                (n, 0) => format!("all {n} already running or queued"),
                (n, a) if a == n => format!("{a} {} queued", action.verb()),
                (n, a) => format!("{a}/{n} {} queued", action.verb()),
            }
        };
    }
}

#[cfg(test)]
impl App {
    /// Build an app over a synthetic suite, without touching the real filesystem.
    ///
    /// The sender is disconnected, so state-only tests can run freely — but any
    /// key that starts a task would still spawn real work, so don't press those.
    pub fn with_apps(apps: Vec<AppInfo>) -> Self {
        let (tx, _rx) = mpsc::channel();
        Self {
            apps,
            sel: 0,
            picks: HashSet::new(),
            busy: HashMap::new(),
            pending: VecDeque::new(),
            failed: HashMap::new(),
            filter_attention: false,
            msg: "ready".into(),
            quit: false,
            show_help: false,
            help_scroll: 0,
            palette: Palette::of(crate::theme::Depth::TrueColor),
            confirm_quit: false,
            log: Vec::new(),
            log_repo: "",
            log_len: 0,
            tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn apps(n: usize) -> Vec<AppInfo> {
        (0..n)
            .map(|i| AppInfo {
                repo: TEST_REPOS[i],
                dir: std::path::PathBuf::from("/nonexistent"),
                pkg: None,
                cloned: false,
                installed: false,
                version: None,
                behind: None,
                commits: Vec::new(),
            })
            .collect()
    }

    /// One app, described by hand: `(repo, cloned, installed, behind)`.
    fn one(repo: &str, cloned: bool, installed: bool, behind: Option<u64>) -> AppInfo {
        AppInfo {
            repo: Box::leak(repo.to_string().into_boxed_str()),
            dir: std::path::PathBuf::from("/nonexistent"),
            pkg: None,
            cloned,
            installed,
            version: None,
            behind,
            commits: Vec::new(),
        }
    }

    static TEST_REPOS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn selection_is_clamped() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.sel, 0);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.sel, 2);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.sel, 2);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.sel, 0);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.sel, 2, "PAGE jump clamps to the last row");
    }

    #[test]
    fn ctrl_c_quits_and_ctrl_letters_do_not_trigger_actions() {
        let mut app = App::with_apps(apps(3));
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(!app.quit, "Ctrl+A must not install everything");
        assert!(app.pending.is_empty() && app.busy.is_empty());
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit, "Ctrl+C must quit");
    }

    #[test]
    fn help_overlay_swallows_keys_until_closed() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.sel, 0, "navigation must not pass through the overlay");
        app.on_key(key(KeyCode::Esc));
        assert!(!app.show_help);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.sel, 1);
    }

    #[test]
    fn help_scrolls_with_j_and_k_without_moving_selection() {
        let mut app = App::with_apps(apps(3));
        app.sel = 2;
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.help_scroll, 0, "opening starts at the top");
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.help_scroll, 2);
        assert_eq!(
            app.sel, 2,
            "the app selection must not move under the overlay"
        );
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.help_scroll, 1);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.help_scroll, 0);
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.help_scroll, 0, "reopening starts at the top again");
    }

    #[test]
    fn quit_asks_for_confirmation_while_tasks_run() {
        let mut app = App::with_apps(apps(3));
        app.busy.insert("a".into(), "installing".into());
        app.on_key(key(KeyCode::Char('q')));
        assert!(!app.quit, "first q must only warn");
        assert!(app.msg.contains("still running"));
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit, "second q must quit");
    }

    #[test]
    fn queued_task_can_be_dequeued() {
        let mut app = App::with_apps(apps(3));
        app.pending.push_back(("a".into(), Action::Install));
        app.on_key(key(KeyCode::Char('x')));
        assert!(app.pending.is_empty());
        assert!(app.msg.contains("dequeued"));
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    /// Fill both runner slots so no key press can spawn real work.
    fn saturate(app: &mut App) {
        app.busy.insert("z1".into(), "installing".into());
        app.busy.insert("z2".into(), "installing".into());
        assert_eq!(app.busy.len(), MAX_PARALLEL);
    }

    #[test]
    fn space_picks_one_row_and_ctrl_space_toggles() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.picks, [0usize].into_iter().collect());
        assert!(app.msg.contains("picked a"));

        app.on_key(key(KeyCode::Down));
        app.on_key(ctrl(KeyCode::Char(' ')));
        assert_eq!(app.picks, [0usize, 1].into_iter().collect());

        app.on_key(key(KeyCode::Down));
        app.on_key(ctrl(KeyCode::Char(' ')));
        assert_eq!(app.picks, [0usize, 1, 2].into_iter().collect());
        app.on_key(ctrl(KeyCode::Char(' ')));
        assert_eq!(app.picks, [0usize, 1].into_iter().collect());

        // Space on a fresh row replaces the whole set, then clears it
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.picks, [2usize].into_iter().collect());
        app.on_key(key(KeyCode::Char(' ')));
        assert!(app.picks.is_empty());
    }

    #[test]
    fn actions_run_on_the_picked_rows_not_the_cursor() {
        let mut app = App::with_apps(apps(3));
        saturate(&mut app);
        app.picks = [1, 2].into_iter().collect();
        app.on_key(key(KeyCode::Char('u')));
        assert_eq!(
            app.pending
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"],
            "cursor row must be skipped while picks are active"
        );
        assert_eq!(app.msg, "2 update queued");
    }

    #[test]
    fn a_single_picks_wins_over_the_highlighted_row() {
        let mut app = App::with_apps(apps(3));
        saturate(&mut app);
        app.sel = 0;
        app.picks = [2].into_iter().collect();
        app.on_key(key(KeyCode::Char('i')));
        assert_eq!(
            app.pending
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert_eq!(app.msg, "c queued");
    }

    #[test]
    fn cancel_only_touches_picked_rows() {
        let mut app = App::with_apps(apps(3));
        app.pending.push_back(("a".into(), Action::Install));
        app.pending.push_back(("b".into(), Action::Install));
        app.pending.push_back(("c".into(), Action::Install));
        app.picks = [1, 2].into_iter().collect();
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(
            app.pending
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            vec!["a"],
            "the unpicked cursor row must stay queued"
        );
        assert_eq!(app.msg, "2 dequeued");
    }

    #[test]
    fn enter_on_a_pick_installs_missing_and_skips_installed() {
        let mut v = apps(3);
        v[1].installed = true;
        let mut app = App::with_apps(v);
        saturate(&mut app);
        app.picks = [0, 1].into_iter().collect();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.pending
                .iter()
                .map(|(r, _)| r.as_str())
                .collect::<Vec<_>>(),
            vec!["a"],
            "the installed row must not be queued"
        );
        assert_eq!(app.msg, "1 install queued · 1 already installed");
    }

    #[test]
    fn attention_filter_hides_current_rows_and_clamps_the_highlight() {
        let mut app = App::with_apps(vec![
            one("a", true, true, Some(0)),
            one("b", true, true, Some(2)),
            one("c", false, false, None),
        ]);
        app.on_key(key(KeyCode::Char('o')));
        assert!(app.filter_attention);
        assert_eq!(app.visible_rows(), vec![1, 2]);
        assert_eq!(app.sel, 1, "highlight must move onto a visible row");
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.sel, 2);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.sel, 2, "the highlight stops at the last visible row");
        app.on_key(key(KeyCode::Char('o')));
        assert!(!app.filter_attention);
        assert_eq!(app.visible_rows(), vec![0, 1, 2]);
    }

    #[test]
    fn failed_tasks_count_as_needing_attention() {
        let mut app = App::with_apps(vec![one("a", true, true, Some(0))]);
        assert!(!app.needs_attention(&app.apps[0]));
        app.failed.insert("a".into(), "cargo build failed".into());
        assert!(app.needs_attention(&app.apps[0]));
        app.on_key(key(KeyCode::Char('o')));
        assert_eq!(app.visible_rows(), vec![0]);
    }

    #[test]
    fn status_tone_follows_the_message_prefix() {
        let mut app = App::with_apps(apps(1));
        assert_eq!(app.status_line().1, Tone::Info);
        app.msg = "✓ installed photocraft".into();
        assert_eq!(app.status_line().1, Tone::Good);
        app.msg = "✗ photocraft · installing: boom".into();
        assert_eq!(app.status_line().1, Tone::Bad);
        app.pending.push_back(("a".into(), Action::Update));
        assert_eq!(app.status_line().1, Tone::Busy);
    }
}

//! Application state: selection, background tasks, key handling.

#[cfg(test)]
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::components::{
    Component, ComponentEvent, HelpOverlayComponent, JobsPopupComponent, ListComponent,
    UninstallConfirmComponent,
};
use crate::engine::{self, Action, AppInfo, Ev, Track};
use crate::task::TaskManager;
use crate::theme::Palette;

pub use crate::task::{DEFAULT_MAX_PARALLEL, MAX_JOBS, MIN_JOBS};

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

/// Coarse, `~`-prefixed estimate — seconds rounded up to the unit it names.
///
/// The `~` is load-bearing: crate compile times vary wildly, so this is an
/// extrapolation from crates-per-second, never a countdown to trust.
pub fn coarse_eta(secs: u64) -> String {
    if secs < 60 {
        format!("~{secs}s")
    } else if secs < 3600 {
        format!("~{}m", secs.div_ceil(60))
    } else {
        format!("~{}h", secs.div_ceil(3600))
    }
}

/// One running task's compact suffix: elapsed while the pace is still warming
/// in, then `~ETA · 54%`. `None` before any real crate finishes (nothing to
/// estimate from yet).
fn compact_progress(p: &engine::Progress) -> Option<String> {
    let elapsed = coarse_eta(p.elapsed_secs());
    match (p.eta_secs(), p.pct()) {
        (Some(eta), Some(pct)) => Some(format!("· {} · {pct}%", coarse_eta(eta))),
        _ => Some(format!("· {elapsed}")),
    }
}

pub struct App {
    /// suite rows, highlight, picks and the attention filter
    pub list: ListComponent,
    /// running/queued/failed tasks and their live progress
    pub tasks: TaskManager,
    /// commits (default) or releases — what "behind" counts; `t` toggles
    pub track: Track,
    pub msg: String,
    pub quit: bool,
    /// `?` overlay: open flag + scroll offset
    pub help: HelpOverlayComponent,
    /// brand palette for this terminal (see [`crate::theme`])
    pub palette: Palette,
    /// second `q` required while tasks are running
    confirm_quit: bool,
    /// pending `d` uninstall: target indices awaiting a `y` confirmation
    pub uninstall: UninstallConfirmComponent,
    /// `p` popup: editor state for the parallel-build limit
    pub jobs: JobsPopupComponent,
    /// Tail of the selected repo's log, refreshed by [`Self::refresh_log`].
    pub log: Vec<String>,
    log_repo: &'static str,
    log_len: u64,
    /// true until the first background scan delivers its rows
    scanning: bool,
    tx: Sender<Ev>,
}

impl App {
    pub fn new(tx: Sender<Ev>, max_parallel: usize, track: Track) -> Self {
        let mut app = Self {
            list: ListComponent::default(),
            tasks: TaskManager::new(max_parallel),
            track,
            msg: "scanning…".into(),
            quit: false,
            help: HelpOverlayComponent::default(),
            palette: Palette::detect(),
            confirm_quit: false,
            uninstall: UninstallConfirmComponent::default(),
            jobs: JobsPopupComponent::default(),
            log: Vec::new(),
            log_repo: "",
            log_len: 0,
            scanning: true,
            tx,
        };
        // the UI opens immediately; rows arrive with Ev::Loaded and the git
        // state settles in with Ev::Checked (see scan_pipeline)
        let tx = app.tx.clone();
        let track = app.track;
        std::thread::spawn(move || scan_pipeline(tx, track));
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
                    self.msg = self.tasks.finish(&repo, action.label(), &result);
                    if let Some(idx) = self.list.apps.iter().position(|a| a.repo == repo) {
                        self.rescan_at(idx);
                    }
                    self.pump();
                }
                Ev::Progress { repo, progress } => {
                    self.tasks.progress.insert(repo, progress);
                }
                Ev::Scanned { index, info } => {
                    // the suite order is fixed, but guard against a stale index anyway
                    if let Some(slot) = self.list.apps.get_mut(index)
                        && slot.repo == info.repo
                    {
                        *slot = info;
                    }
                }
                Ev::Loaded(apps) => {
                    self.list.apps = apps;
                    self.clamp_selection();
                    // rows are local-only now; the status line admits how long
                    // until Checked delivers real git state
                    let clones = self.list.apps.iter().filter(|a| a.cloned).count();
                    if clones > 0 && engine::fetch_enabled() {
                        let word = if clones == 1 { "repo" } else { "repos" };
                        self.msg = format!("checking {clones} {word} for updates…");
                    }
                }
                Ev::Checked => {
                    self.msg = if self.scanning {
                        self.scanning = false;
                        "ready".into()
                    } else {
                        "refreshed".into()
                    };
                }
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
        let repo = self.list.apps.get(self.list.sel).map_or("", |a| a.repo);
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
    /// The highlighted row's live build progress, if it's building.
    pub fn selected_progress(&self) -> Option<&engine::Progress> {
        let repo = self.list.apps.get(self.list.sel)?.repo;
        self.tasks.progress.get(repo)
    }

    pub fn status_line(&self) -> (String, Tone) {
        let mut parts = Vec::new();
        if let Some(s) = engine::busy_summary(&self.tasks.busy) {
            let mut s = s;
            // A single running task earns a compact "~2m · 54%"; parallel
            // builds would crowd the row, so they keep the plain verb list.
            if self.tasks.busy.len() == 1
                && let Some(extra) = self.selected_progress().and_then(compact_progress)
            {
                s.push_str(&format!(" {extra}"));
            }
            parts.push(s);
        }
        if !self.tasks.pending.is_empty() {
            parts.push(format!("{} queued", self.tasks.pending.len()));
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
        if self.tasks.is_staged(&repo) {
            return;
        }
        if !self.tasks.has_slot() {
            self.tasks.pending.push_back((repo, action));
            return;
        }
        self.tasks.failed.remove(&repo);
        self.start_now(repo, action);
    }

    /// Spawn `action` against `repo`'s discovered clone location.
    fn start_now(&mut self, repo: String, action: Action) {
        let Some(dir) = self
            .list
            .apps
            .iter()
            .find(|a| a.repo == repo)
            .map(|a| a.dir.clone())
        else {
            return;
        };
        self.tasks.busy.insert(repo.clone(), action.label().into());
        engine::spawn(repo, dir, action, self.track, self.tx.clone());
    }

    fn pump(&mut self) {
        while let Some((repo, action)) = self.tasks.pending.pop_front() {
            if self.tasks.busy.contains_key(&repo) || !self.tasks.has_slot() {
                self.tasks.pending.push_front((repo, action));
                break;
            }
            self.tasks.failed.remove(&repo);
            self.start_now(repo, action);
        }
    }

    /// Re-read one entry from disk on a worker thread.
    fn rescan_at(&self, index: usize) {
        let Some(cur) = self.list.apps.get(index) else {
            return;
        };
        let mut info = cur.clone();
        let tx = self.tx.clone();
        let track = self.track;
        std::thread::spawn(move || {
            engine::rescan_one(&mut info, track);
            let _ = tx.send(Ev::Scanned { index, info });
        });
    }

    /// Re-read every entry from disk on one worker thread — a full rescan,
    /// discovery of the clone locations and the upstream check included (`r`).
    fn rescan_all(&self) {
        let tx = self.tx.clone();
        let track = self.track;
        std::thread::spawn(move || scan_pipeline(tx, track));
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
            self.help.toggle();
            self.confirm_quit = false;
            return;
        }
        if self.help.open {
            self.help.handle_key(key);
            return;
        }
        // The parallel-builds popup: adjust the scratch value, apply or cancel.
        // Not destructive, so other keys are simply ignored.
        if self.jobs.open {
            match self.jobs.handle_key(key) {
                ComponentEvent::JobsApply => self.jobs_apply(),
                ComponentEvent::Msg(m) => self.msg = m,
                _ => {}
            }
            return;
        }
        // A pending uninstall wants a plain `y`; anything else changes its mind.
        if self.uninstall.pending() {
            // `y` produces Ignored and falls through to the normal-mode match.
            if let ComponentEvent::Msg(m) = self.uninstall.handle_key(key) {
                self.msg = m;
                self.confirm_quit = false;
                return;
            }
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
                self.list.sel = self.visible_rows().first().copied().unwrap_or(0);
            }
            End | Char('G') => {
                self.list.sel = self.visible_rows().last().copied().unwrap_or(0);
            }
            Right | Char('l') => {
                self.list.expanded.insert(self.list.sel);
            }
            Left | Char('h') => {
                self.list.expanded.remove(&self.list.sel);
            }
            Enter => self.enter(),
            Char(' ') => self.pick_one(),
            Char('u') => self.start_action(Action::Update),
            Char('i') => self.start_action(Action::Install),
            Char('f') => self.start_action(Action::Fetch),
            Char('x') | Delete => self.cancel_selected(),
            Char('d') => self.uninstall_selected(),
            Char('p') => {
                self.jobs.open_with(self.tasks.max_parallel);
            }
            Char('y') => {
                if let Some(targets) = self.uninstall.targets.take() {
                    self.run_uninstall(targets);
                }
            }
            Char('U') => {
                let repos: Vec<String> = self
                    .list
                    .apps
                    .iter()
                    .filter(|a| a.cloned)
                    .map(|a| a.repo.to_string())
                    .collect();
                let before = self.tasks.busy.len() + self.tasks.pending.len();
                for repo in repos {
                    self.try_start(repo, Action::Update);
                }
                self.msg = if self.tasks.busy.len() + self.tasks.pending.len() > before {
                    "update queued".into()
                } else {
                    "nothing to update".into()
                };
            }
            Char('a') => {
                let repos: Vec<String> = self
                    .list
                    .apps
                    .iter()
                    .filter(|a| !a.installed)
                    .map(|a| a.repo.to_string())
                    .collect();
                let before = self.tasks.busy.len() + self.tasks.pending.len();
                for repo in repos {
                    self.try_start(repo, Action::Install);
                }
                self.msg = if self.tasks.busy.len() + self.tasks.pending.len() > before {
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
                self.list.filter_attention = !self.list.filter_attention;
                self.clamp_selection();
                self.msg = if !self.list.filter_attention {
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
            Char('t') => {
                self.toggle_track();
                self.rescan_all();
            }
            _ => {}
        }
    }

    /// Flip between commits and releases and say so. The caller re-checks
    /// every row (the counts mean different things under each track).
    fn toggle_track(&mut self) {
        self.track = match self.track {
            Track::Commits => Track::Releases,
            Track::Releases => Track::Commits,
        };
        self.msg = match self.track {
            Track::Releases => "tracking releases — re-checking…".into(),
            Track::Commits => "tracking commits — re-checking…".into(),
        };
    }

    /// Rows the list currently shows — see [`ListComponent::visible_rows`].
    pub fn visible_rows(&self) -> Vec<usize> {
        self.list.visible_rows(&self.tasks.failed)
    }

    /// Step the highlight by `step` rows *within what is visible*.
    fn move_sel(&mut self, step: isize) {
        self.list.move_sel(step, &self.tasks.failed);
    }

    /// Keep the highlight on a row the list is actually showing.
    fn clamp_selection(&mut self) {
        self.list.clamp_selection(&self.tasks.failed);
    }

    fn request_quit(&mut self) {
        let running = self.tasks.busy.len() + self.tasks.pending.len();
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
        if let ComponentEvent::Msg(m) = self.list.pick_one() {
            self.msg = m;
        }
    }

    /// `Ctrl+Space`: add or remove this row from the picked set.
    fn pick_toggle(&mut self) {
        if let ComponentEvent::Msg(m) = self.list.pick_toggle() {
            self.msg = m;
        }
    }

    fn row_label(&self, idx: usize) -> String {
        self.list.row_label(idx)
    }

    /// Rows an operation applies to: the picked set, else the highlighted row.
    fn targets(&self) -> Vec<usize> {
        self.list.targets()
    }

    fn is_staged(&self, repo: &str) -> bool {
        self.tasks.is_staged(repo)
    }

    fn cancel_selected(&mut self) {
        if self.list.apps.is_empty() {
            return;
        }
        let targets = self.targets();
        let mut cancelling = 0usize;
        let mut dequeued = 0usize;
        let mut finished = 0usize;
        let mut idle = 0usize;
        for i in &targets {
            let Some(repo) = self.list.apps.get(*i).map(|a| a.repo) else {
                continue;
            };
            if self.tasks.busy.contains_key(repo) {
                if engine::cancel(repo) {
                    cancelling += 1;
                } else {
                    finished += 1;
                }
            } else if let Some(pos) = self.tasks.pending.iter().position(|(r, _)| r == repo) {
                self.tasks.pending.remove(pos);
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

    /// `Enter` in the `p` popup: adopt the new limit and start any queued
    /// work the raised cap now allows. The popup already closed itself.
    fn jobs_apply(&mut self) {
        self.tasks.max_parallel = self.jobs.limit();
        self.msg = format!("parallel builds: {}", self.tasks.max_parallel);
        self.pump();
    }

    /// `d`: offer to uninstall the picked (or highlighted) rows — binary,
    /// desktop entry, icons and the clone. `y` confirms; any other key cancels.
    fn uninstall_selected(&mut self) {
        if self.list.apps.is_empty() {
            return;
        }
        let targets = self.targets();
        let eligible: Vec<usize> = targets
            .iter()
            .copied()
            .filter(|i| {
                self.list
                    .apps
                    .get(*i)
                    .is_some_and(|a| a.installed && !self.is_staged(a.repo))
            })
            .collect();
        let skipped = targets.len() - eligible.len();
        if eligible.is_empty() {
            self.msg = format!("nothing to uninstall ({skipped} skipped)");
            return;
        }
        let what = if eligible.len() == 1 {
            format!("uninstall {}?", self.row_label(eligible[0]))
        } else {
            format!("uninstall {} apps?", eligible.len())
        };
        self.msg = format!("{what} y = binary + menu entry + icons + clone; Esc cancels");
        self.uninstall.open(eligible);
    }

    /// `y`: carry out the pending uninstall.
    fn run_uninstall(&mut self, targets: Vec<usize>) {
        let mut removed = 0usize;
        let mut skipped = 0usize;
        let mut last_msg = String::new();
        let mut last_err = None;
        for i in targets {
            let Some(a) = self.list.apps.get(i) else {
                continue;
            };
            let repo = a.repo.to_string();
            let dir = a.dir.clone();
            if self.is_staged(&repo) || !a.installed {
                skipped += 1;
                continue;
            }
            match engine::uninstall(&repo, &dir) {
                Ok(msg) => {
                    removed += 1;
                    last_msg = msg;
                    self.tasks.failed.remove(&repo);
                    self.rescan_at(i);
                }
                Err(e) => {
                    skipped += 1;
                    last_err = Some((repo, e));
                }
            }
        }
        self.msg = if removed == 1 && last_err.is_none() {
            last_msg
        } else if removed == 0 {
            match last_err {
                Some((repo, e)) => {
                    self.tasks.failed.insert(repo.clone(), e.clone());
                    format!("{repo} · {e}")
                }
                None => format!("nothing to uninstall ({skipped} skipped)"),
            }
        } else {
            let mut parts = vec![format!("{removed} uninstalled")];
            if skipped > 0 {
                parts.push(format!("{skipped} skipped"));
            }
            if let Some((repo, e)) = last_err {
                self.tasks.failed.insert(repo.clone(), e.clone());
                parts.push(format!("{repo} · {e}"));
            }
            parts.join(" · ")
        };
        self.clamp_selection();
    }

    fn enter(&mut self) {
        if self.list.apps.is_empty() {
            return;
        }
        let targets = self.targets();
        if targets.len() > 1 {
            self.enter_many(&targets);
            return;
        }
        let Some(a) = self.list.apps.get(targets[0]) else {
            return;
        };
        let repo = a.repo;
        let installed = a.installed;
        let pkg = a.pkg.clone();
        if self.tasks.busy.contains_key(repo) {
            self.msg = format!("{} already running for {repo}", self.tasks.busy[repo]);
            return;
        }
        if self.tasks.pending.iter().any(|(r, _)| r == repo) {
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
        let before = self.tasks.busy.len() + self.tasks.pending.len();
        let mut installed = 0usize;
        for i in targets {
            let Some((is_installed, repo)) = self
                .list
                .apps
                .get(*i)
                .map(|a| (a.installed, a.repo.to_string()))
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
        let queued = (self.tasks.busy.len() + self.tasks.pending.len()).saturating_sub(before);
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
        if self.list.apps.is_empty() {
            self.msg = "no apps".into();
            return;
        }
        let targets = self.targets();
        let before = self.tasks.busy.len() + self.tasks.pending.len();
        // a single target that was already running before we touched it
        let was_running = (targets.len() == 1)
            .then(|| self.row_label(targets[0]))
            .is_some_and(|repo| self.tasks.busy.contains_key(&repo));
        for i in &targets {
            let Some(repo) = self.list.apps.get(*i).map(|a| a.repo.to_string()) else {
                continue;
            };
            self.try_start(repo, action);
        }
        let added = (self.tasks.busy.len() + self.tasks.pending.len()).saturating_sub(before);
        self.msg = if targets.len() == 1 {
            let repo = self.row_label(targets[0]);
            if self.tasks.busy.contains_key(&repo) && !was_running {
                format!("{} {repo}", action.label())
            } else if self.tasks.busy.contains_key(&repo) {
                format!("{} already running for {repo}", self.tasks.busy[&repo])
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

/// The background scan: local rows first so the table fills instantly, then a
/// bounded fetch per clone so "up to date" is measured against the remote —
/// the cached refs a plain scan sees are only refreshed by fetch (or pull),
/// and happily report "up to date" a day after new commits land.
///
/// A failed fetch (offline, timeout) leaves that row honestly "not checked"
/// instead of trusting the stale refs. Always ends with [`Ev::Checked`], even
/// when the check is disabled or there is nothing to fetch.
fn scan_pipeline(tx: Sender<Ev>, track: Track) {
    let apps = engine::scan();
    let seeds: Vec<(usize, AppInfo)> = apps
        .iter()
        .enumerate()
        .filter(|(_, a)| a.cloned)
        .map(|(i, a)| (i, a.clone()))
        .collect();
    let _ = tx.send(Ev::Loaded(apps));
    if engine::fetch_enabled() {
        for (index, mut info) in seeds {
            if engine::git_fetch_bounded(&info.dir).is_ok() {
                engine::rescan_one(&mut info, track);
            } else {
                engine::rescan_local(&mut info);
            }
            let _ = tx.send(Ev::Scanned { index, info });
        }
    }
    let _ = tx.send(Ev::Checked);
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
            list: ListComponent {
                apps,
                ..ListComponent::default()
            },
            tasks: TaskManager::new(DEFAULT_MAX_PARALLEL),
            track: Track::Commits,
            msg: "ready".into(),
            quit: false,
            help: HelpOverlayComponent::default(),
            palette: Palette::of(crate::theme::Depth::TrueColor),
            confirm_quit: false,
            uninstall: UninstallConfirmComponent::default(),
            jobs: JobsPopupComponent {
                open: false,
                scratch: DEFAULT_MAX_PARALLEL,
            },
            log: Vec::new(),
            log_repo: "",
            log_len: 0,
            scanning: false,
            tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::time::{Duration, Instant};

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
                menu: false,
                stale: false,
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
            menu: false,
            stale: false,
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
        assert_eq!(app.list.sel, 0);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.list.sel, 2);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.list.sel, 2);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.list.sel, 0);
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.list.sel, 2, "PAGE jump clamps to the last row");
    }

    #[test]
    fn ctrl_c_quits_and_ctrl_letters_do_not_trigger_actions() {
        let mut app = App::with_apps(apps(3));
        app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(!app.quit, "Ctrl+A must not install everything");
        assert!(app.tasks.pending.is_empty() && app.tasks.busy.is_empty());
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.quit, "Ctrl+C must quit");
    }

    #[test]
    fn right_and_left_toggle_the_description_row() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Right));
        assert!(
            app.list.expanded.contains(&0),
            "Right expands the highlighted app"
        );
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Char('l')));
        assert!(app.list.expanded.contains(&1), "l expands too");
        assert!(
            app.list.expanded.contains(&0),
            "expansions stay per app while moving"
        );
        app.on_key(key(KeyCode::Char('h')));
        assert!(
            !app.list.expanded.contains(&1),
            "h collapses the highlighted row"
        );
        assert!(
            app.list.expanded.contains(&0),
            "other expansions are untouched"
        );
        app.on_key(key(KeyCode::Left));
        assert!(
            app.list.expanded.contains(&0),
            "Left only collapses the highlighted app"
        );
        app.on_key(key(KeyCode::Home));
        app.on_key(key(KeyCode::Left));
        assert!(
            app.list.expanded.is_empty(),
            "Left collapses the app under the cursor"
        );
    }

    #[test]
    fn quit_asks_for_confirmation_while_tasks_run() {
        let mut app = App::with_apps(apps(3));
        app.tasks.busy.insert("a".into(), "installing".into());
        app.on_key(key(KeyCode::Char('q')));
        assert!(!app.quit, "first q must only warn");
        assert!(app.msg.contains("still running"));
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.quit, "second q must quit");
    }

    #[test]
    fn queued_task_can_be_dequeued() {
        let mut app = App::with_apps(apps(3));
        app.tasks.pending.push_back(("a".into(), Action::Install));
        app.on_key(key(KeyCode::Char('x')));
        assert!(app.tasks.pending.is_empty());
        assert!(app.msg.contains("dequeued"));
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    /// Fill every runner slot so no key press can spawn real work.
    fn saturate(app: &mut App) {
        for i in 0..app.tasks.max_parallel {
            app.tasks.busy.insert(format!("z{i}"), "installing".into());
        }
        assert_eq!(app.tasks.busy.len(), app.tasks.max_parallel);
    }

    #[test]
    fn space_picks_one_row_and_ctrl_space_toggles() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.list.picks, [0usize].into_iter().collect());
        assert!(app.msg.contains("picked a"));

        app.on_key(key(KeyCode::Down));
        app.on_key(ctrl(KeyCode::Char(' ')));
        assert_eq!(app.list.picks, [0usize, 1].into_iter().collect());

        app.on_key(key(KeyCode::Down));
        app.on_key(ctrl(KeyCode::Char(' ')));
        assert_eq!(app.list.picks, [0usize, 1, 2].into_iter().collect());
        app.on_key(ctrl(KeyCode::Char(' ')));
        assert_eq!(app.list.picks, [0usize, 1].into_iter().collect());

        // Space on a fresh row replaces the whole set, then clears it
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.list.picks, [2usize].into_iter().collect());
        app.on_key(key(KeyCode::Char(' ')));
        assert!(app.list.picks.is_empty());
    }

    #[test]
    fn actions_run_on_the_picked_rows_not_the_cursor() {
        let mut app = App::with_apps(apps(3));
        saturate(&mut app);
        app.list.picks = [1, 2].into_iter().collect();
        app.on_key(key(KeyCode::Char('u')));
        assert_eq!(
            app.tasks
                .pending
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
        app.list.sel = 0;
        app.list.picks = [2].into_iter().collect();
        app.on_key(key(KeyCode::Char('i')));
        assert_eq!(
            app.tasks
                .pending
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
        app.tasks.pending.push_back(("a".into(), Action::Install));
        app.tasks.pending.push_back(("b".into(), Action::Install));
        app.tasks.pending.push_back(("c".into(), Action::Install));
        app.list.picks = [1, 2].into_iter().collect();
        app.on_key(key(KeyCode::Char('x')));
        assert_eq!(
            app.tasks
                .pending
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
        app.list.picks = [0, 1].into_iter().collect();
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.tasks
                .pending
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
        assert!(app.list.filter_attention);
        assert_eq!(app.visible_rows(), vec![1, 2]);
        assert_eq!(app.list.sel, 1, "highlight must move onto a visible row");
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.list.sel, 2);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.list.sel, 2,
            "the highlight stops at the last visible row"
        );
        app.on_key(key(KeyCode::Char('o')));
        assert!(!app.list.filter_attention);
        assert_eq!(app.visible_rows(), vec![0, 1, 2]);
    }

    #[test]
    fn failed_tasks_count_as_needing_attention() {
        let mut app = App::with_apps(vec![one("a", true, true, Some(0))]);
        assert!(
            !app.list
                .needs_attention(&app.list.apps[0], &app.tasks.failed)
        );
        app.tasks
            .failed
            .insert("a".into(), "cargo build failed".into());
        assert!(
            app.list
                .needs_attention(&app.list.apps[0], &app.tasks.failed)
        );
        app.on_key(key(KeyCode::Char('o')));
        assert_eq!(app.visible_rows(), vec![0]);
    }

    /// `t` flips the mode and announces it; the real key also re-checks every
    /// row (a background rescan, so the toggle itself is tested directly).
    #[test]
    fn toggling_the_track_switches_modes_and_says_so() {
        let mut app = App::with_apps(apps(2));
        assert_eq!(app.track, Track::Commits);

        app.toggle_track();
        assert_eq!(app.track, Track::Releases);
        assert!(app.msg.contains("tracking releases"), "msg: {}", app.msg);

        app.toggle_track();
        assert_eq!(app.track, Track::Commits);
        assert!(app.msg.contains("tracking commits"), "msg: {}", app.msg);
    }

    #[test]
    fn status_tone_follows_the_message_prefix() {
        let mut app = App::with_apps(apps(1));
        assert_eq!(app.status_line().1, Tone::Info);
        app.msg = "✓ installed photocraft".into();
        assert_eq!(app.status_line().1, Tone::Good);
        app.msg = "✗ photocraft · installing: boom".into();
        assert_eq!(app.status_line().1, Tone::Bad);
        app.tasks.pending.push_back(("a".into(), Action::Update));
        assert_eq!(app.status_line().1, Tone::Busy);
    }

    #[test]
    fn loaded_rows_replace_the_list_and_clamp_the_highlight() {
        let (tx, rx) = mpsc::channel();
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::End));
        assert_eq!(app.list.sel, 2);

        // the background scan finishing: fewer rows than before the highlight
        tx.send(Ev::Loaded(apps(1))).expect("send");
        app.drain(&rx);
        assert_eq!(
            app.list.apps.len(),
            1,
            "the delivered rows replace the old ones"
        );
        assert_eq!(
            app.list.sel, 0,
            "the highlight is clamped onto the new rows"
        );
        // rows alone don't finish a rescan — the upstream check does
        tx.send(Ev::Checked).expect("send");
        app.drain(&rx);
        assert_eq!(app.msg, "refreshed");
    }

    #[test]
    fn clones_announce_the_upstream_check_until_checked_arrives() {
        let (tx, rx) = mpsc::channel();
        let mut app = App::with_apps(apps(1));
        app.scanning = true;
        app.msg = "scanning…".into();
        let mut cloned = apps(1);
        cloned[0].cloned = true;
        tx.send(Ev::Loaded(cloned)).expect("send");
        app.drain(&rx);
        assert_eq!(app.msg, "checking 1 repo for updates…");
        assert!(app.scanning, "the scan isn't done until Checked");
        tx.send(Ev::Checked).expect("send");
        app.drain(&rx);
        assert_eq!(app.msg, "ready");
        assert!(!app.scanning);
    }

    #[test]
    fn a_scan_with_nothing_to_fetch_settles_on_checked_alone() {
        let (tx, rx) = mpsc::channel();
        let mut app = App::with_apps(apps(1));
        app.scanning = true;
        app.msg = "scanning…".into();
        // no cloned rows: Loaded must not promise a check that isn't coming
        tx.send(Ev::Loaded(apps(1))).expect("send");
        app.drain(&rx);
        assert_eq!(app.msg, "scanning…");
        tx.send(Ev::Checked).expect("send");
        app.drain(&rx);
        assert_eq!(app.msg, "ready");
    }

    #[test]
    fn progress_events_fill_the_map_and_done_clears_it() {
        let (tx, rx) = mpsc::channel();
        let mut app = App::with_apps(apps(1));
        let started = Instant::now() - Duration::from_secs(20);
        let p = engine::Progress {
            done: 10,
            seen: 10,
            total: 40,
            started,
            pace_secs: None,
            last_real: None,
        };
        tx.send(Ev::Progress {
            repo: "a".into(),
            progress: p.clone(),
        })
        .expect("send");
        app.drain(&rx);
        assert_eq!(
            app.tasks.progress.get("a").map(|p| (p.done, p.total)),
            Some((10, 40))
        );
        assert_eq!(app.selected_progress().map(|p| p.total), Some(40));

        // The task finishing must not leave a stale percentage on screen.
        app.tasks.busy.insert("a".into(), "building".into());
        tx.send(Ev::Done {
            repo: "a".into(),
            action: Action::Install,
            result: Ok("installed".into()),
        })
        .expect("send");
        app.drain(&rx);
        assert!(
            !app.tasks.progress.contains_key("a"),
            "progress must clear on Done"
        );
    }

    #[test]
    fn a_single_running_task_gets_a_compact_eta_on_the_status_row() {
        let mut app = App::with_apps(apps(1));
        app.tasks.busy.insert("a".into(), "building".into());
        app.tasks.progress.insert(
            "a".into(),
            engine::Progress {
                done: 10,
                seen: 10,
                total: 40,
                started: Instant::now() - Duration::from_secs(20),
                // 30 unresolved × 10s per crate → ~5m, 10/40 → 25%.
                pace_secs: Some(10.0),
                last_real: Some(Instant::now()),
            },
        );
        let (line, tone) = app.status_line();
        assert_eq!(tone, Tone::Busy);
        assert!(line.contains("building a"), "{line:?}");
        assert!(
            line.contains('~'),
            "the ETA must read as an estimate: {line:?}"
        );
        assert!(line.contains("25%"), "wrong percentage: {line:?}");
    }
}

//! Suite registry and source-build engine: clone, build, install, update, launch.
//! The same workflow as update-crafts.sh, self-contained so the package has no script dependency.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// The Craft App Suite as listed on getartcraft.com/apps, plus the launcher.
pub const SUITE: &[&str] = &[
    "cadcraft",
    "craft-launcher",
    "deckcraft",
    "designcraft",
    "effectcraft",
    "figcraft",
    "filmcraft",
    "gridcraft",
    "lightcraft",
    "pdfcraft",
    "photocraft",
    "soundcraft",
    "vectorcraft",
    "wordcraft",
];

/// One-line product descriptions, as published on getartcraft.com/apps.
pub fn description_of(repo: &str) -> &'static str {
    match repo {
        "cadcraft" => "Fast, open-source computer-aided design and drafting in pure Rust.",
        "craft-launcher" => "The Craft App Launcher: installs every app and keeps them up to date.",
        "deckcraft" => "Build and deliver dynamic slide presentations, in pure Rust.",
        "designcraft" => "Open-source page layout and publishing, rebuilt in pure Rust.",
        "effectcraft" => "Open-source motion graphics and visual effects, built in pure Rust.",
        "figcraft" => "Open-source interface design and prototyping, rebuilt in pure Rust.",
        "filmcraft" => "Professional, open-source video editing, rebuilt from scratch in Rust.",
        "gridcraft" => "Fast, open-source spreadsheets and data analysis, rebuilt in pure Rust.",
        "lightcraft" => "An open-source photo library and raw developer that runs on your machine.",
        "pdfcraft" => "The open-source PDF workbench: read, organize, combine, split and secure.",
        "photocraft" => "The open-source image editor you already know how to use.",
        "soundcraft" => "Professional audio recording, editing and mixing in pure Rust.",
        "vectorcraft" => "Fast, open-source vector illustration, reimagined in pure Rust.",
        "wordcraft" => "Fast, open-source word processing, rebuilt in pure Rust.",
        _ => "A craft app from the ArtCraft suite.",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Install,
    Update,
    Fetch,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Install => "installing",
            Action::Update => "updating",
            Action::Fetch => "fetching",
        }
    }

    /// Imperative form, for messages about a batch of picked rows.
    pub fn verb(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Update => "update",
            Action::Fetch => "fetch",
        }
    }
}

/// What "behind" is measured in: the branch's commits (the default), or the
/// upstream's release tags — for people who only want stable releases.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Track {
    Commits,
    Releases,
}

#[derive(Clone, Debug)]
pub struct AppInfo {
    pub repo: &'static str,
    pub dir: PathBuf,
    pub pkg: Option<String>,
    pub cloned: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub behind: Option<u64>,
    /// Upstream commits waiting to be pulled (`git log --oneline HEAD..@{u}`),
    /// newest last — what the update actually contains. In releases mode the
    /// missing release tag names, newest first.
    pub commits: Vec<String>,
    /// A `.desktop` entry for this app exists in the user's (or system's)
    /// applications directory — registered by this tool, the launcher or an
    /// AUR package alike.
    pub menu: bool,
    /// The installed binary was built from an older revision than the
    /// clone's current HEAD: a pull moved the source on while a cancelled or
    /// failed build left the previous binary in place. The clone is current,
    /// the install is not — `u` rebuilds it.
    pub stale: bool,
}

/// Measured build progress, from cargo's own `--message-format=json` output:
/// one `compiler-artifact` event per crate cargo produces (fresh or not).
///
/// `total` is a real denominator computed *before* the build starts (see
/// [`estimate_units`]): the exact unit count from the previous successful
/// build, or a `cargo tree` estimate when there isn't one. It stays fixed for
/// the whole build. Three counters keep the two numbers honest about each
/// other: `done` counts the crates that actually compile (non-`fresh`),
/// `seen` counts every artifact cargo emits (fresh included), and `total` is
/// the expected unit count — so the percentage is `done` over the real work
/// left (`done + total − seen`), starting at zero and reaching 99% exactly
/// when the last unresolved unit resolves. When the denominator couldn't be
/// obtained `total` is `0` and [`Self::pct`] returns `None` rather than
/// reporting a wrong percentage.
#[derive(Clone, Debug)]
pub struct Progress {
    /// Crates compiled so far (non-`fresh` artifacts).
    pub done: u64,
    /// Every artifact cargo emitted, fresh and compiled alike; the gap
    /// `total − seen` is the work still unresolved (fresh crates resolve it
    /// for free the moment they appear).
    pub seen: u64,
    /// Crates the build is expected to produce; fixed for the build, `0` when
    /// unknown (in which case the UI shows elapsed time, not a bar).
    pub total: u64,
    /// When the build phase started, for the elapsed display.
    pub started: Instant,
    /// Smoothed seconds per real work event (EWMA over non-`fresh` artifact
    /// gaps and build-script runs) — `None` until two samples exist, so the
    /// dead pre-build phase (index update, downloads) never seeds a guess.
    /// Set only through [`Self::observe`].
    pub pace_secs: Option<f64>,
    /// When the previous real work event happened, for the next pace gap.
    pub last_real: Option<Instant>,
}

/// EWMA weight of the newest gap: 0.3 keeps the estimate responsive to a
/// slow crate without letting one outlier own the whole window.
const PACE_ALPHA: f64 = 0.3;

impl Progress {
    /// Record a real-work event at `at` and fold its gap into the pace.
    ///
    /// Fresh artifacts deliberately never call this: they arrive in
    /// millisecond bursts and would collapse the pace to "everything is
    /// instant", which is exactly the lie this model exists to avoid.
    pub fn observe(&mut self, at: Instant) {
        if let Some(prev) = self.last_real {
            let gap = at.saturating_duration_since(prev).as_secs_f64();
            self.pace_secs = Some(match self.pace_secs {
                Some(prev_pace) => PACE_ALPHA * gap + (1.0 - PACE_ALPHA) * prev_pace,
                None => gap,
            });
        }
        self.last_real = Some(at);
    }

    /// Units cargo has not resolved yet — the real work left.
    pub fn unresolved(&self) -> u64 {
        self.total.saturating_sub(self.seen)
    }

    /// Percentage complete, or `None` while the total is still unknown.
    ///
    /// Linear on real crates, not on time: `done / (done + unresolved)`.
    /// Fresh crates shrink `unresolved` the instant they stream by, so a warm
    /// build's bar starts at 0% and advances only for work actually done.
    /// If the seed was too low and the counters exhaust it mid-build, the
    /// bar holds at 99% — never 100% before the build reports `Done`
    /// (`save_units` fixes the denominator on the next successful build).
    pub fn pct(&self) -> Option<u8> {
        if self.total == 0 {
            return None;
        }
        let left = self.unresolved();
        if left == 0 {
            return Some(if self.done == 0 { 0 } else { 99 });
        }
        let p = self.done * 100 / (self.done + left);
        Some(p.min(99) as u8)
    }

    /// Seconds left: unresolved units × the recent per-unit pace. `None`
    /// until a pace exists (two real samples) or nothing is left — before
    /// that there's nothing honest to extrapolate from, and the UI shows
    /// elapsed time instead.
    ///
    /// The value only changes when a crate lands (events update the pace),
    /// not on every tick, so it can't inflate through a slow crate and snap
    /// back down afterwards.
    pub fn eta_secs(&self) -> Option<u64> {
        let pace = self.pace_secs?;
        if self.total == 0 || self.unresolved() == 0 {
            return None;
        }
        Some((self.unresolved() as f64 * pace).ceil() as u64)
    }

    /// Seconds already spent, for the pre-percentage warm-up line.
    pub fn elapsed_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

#[derive(Debug)]
pub enum Ev {
    Done {
        repo: String,
        action: Action,
        result: Result<String, String>,
    },
    /// A build reported live progress; carries the latest [`Progress`].
    Progress { repo: String, progress: Progress },
    /// Fresh AppInfo for `apps[index]`, computed on a worker thread.
    Scanned { index: usize, info: AppInfo },
    /// A full scan finished — discovery, menu entries and every row in it.
    Loaded(Vec<AppInfo>),
    /// The upstream check is done: every clone was fetched (or gave up) and
    /// its row now holds real git state, or honestly admits it has none.
    Checked,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

fn bin_dir() -> PathBuf {
    home().join(".local/bin")
}

fn desk_dir() -> PathBuf {
    home().join(".local/share/applications")
}

fn icon_dir() -> PathBuf {
    home().join(".local/share/icons/hicolor")
}

pub fn log_dir() -> PathBuf {
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(".cache"),
    };
    base.join("artcraft-suite-aur")
}

pub fn log_file(repo: &str) -> PathBuf {
    log_dir().join(format!("{repo}.log"))
}

/// Last successful build's exact unit count for a repo — the seed for the next
/// build's progress denominator. Written by `build_install`, read by
/// [`estimate_units`].
fn units_file(repo: &str) -> PathBuf {
    log_dir().join(format!("{repo}.units"))
}

/// The HEAD revision an install was last built from — what makes a stale
/// binary detectable after the clone moves on. Written by `build_install`,
/// read by [`install_is_stale`].
fn installed_rev_file(repo: &str) -> PathBuf {
    log_dir().join(format!("{repo}.rev"))
}

fn installed_rev(repo: &str) -> Option<String> {
    std::fs::read_to_string(installed_rev_file(repo))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn save_installed_rev(repo: &str, rev: &str) {
    let _ = std::fs::create_dir_all(log_dir());
    let _ = std::fs::write(installed_rev_file(repo), rev);
}

/// Where a clone would land if nothing is discovered (`~/<repo>`).
pub fn default_dir(repo: &str) -> PathBuf {
    home().join(repo)
}

/// `$HOME`, as the rest of the world sees it (for display paths).
pub fn home_dir() -> PathBuf {
    home()
}

/// Roots searched for suite clones: `$HOME`, plus the user's extra roots from
/// `ARTCRAFT_REPO_ROOTS` (colon-separated).
fn repo_roots() -> Vec<PathBuf> {
    let mut roots = vec![home()];
    if let Ok(extra) = std::env::var("ARTCRAFT_REPO_ROOTS") {
        roots.extend(
            extra
                .split(':')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        );
    }
    roots
}

/// Locate every suite repo on disk: a directory of a suite name up to three
/// levels deep under any root (so `~/photocraft`, `~/src/photocraft` and
/// `~/Documents/GitHub/photocraft` all count), or a clone whose git remote is
/// `github.com/<org>/<suite>` (renamed clones and forks included). Pure
/// directory/file reads — cheap enough to run on every launch and every `r`.
pub fn discover() -> HashMap<&'static str, PathBuf> {
    discover_in(&repo_roots())
}

/// [`discover`] over explicit roots, so tests can point it at a temp tree.
pub fn discover_in(roots: &[PathBuf]) -> HashMap<&'static str, PathBuf> {
    let mut found = HashMap::new();
    for depth in [1usize, 2, 3] {
        for root in roots {
            discover_walk(root, depth, &mut found);
        }
    }
    found
}

fn discover_walk(dir: &Path, depth: usize, found: &mut HashMap<&'static str, PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        if let Some(repo) = match_repo(&path, &name) {
            // a clone is never descended into, and the first hit wins
            found.entry(repo).or_insert(path);
            continue;
        }
        discover_walk(&path, depth - 1, found);
    }
}

/// Is `path` (directory named `name`) a suite clone? By directory name, or by
/// its git remote — any org, so renamed clones and forks count too.
fn match_repo(path: &Path, name: &str) -> Option<&'static str> {
    let has_source = path.join("Cargo.toml").is_file() || path.join(".git").exists();
    if has_source && let Some(repo) = SUITE.iter().copied().find(|&r| r == name) {
        return Some(repo);
    }
    // not named after its suite: ask its git remote instead
    path.join(".git")
        .exists()
        .then(|| git_config(path))
        .flatten()
        .and_then(|cfg| remote_repo(&cfg))
}

/// The `.git/config` of a clone — also for worktrees/submodules, where `.git`
/// is a file pointing at the real directory.
fn git_config(dir: &Path) -> Option<String> {
    let git = dir.join(".git");
    let cfg = if git.is_dir() {
        git.join("config")
    } else {
        let target = std::fs::read_to_string(&git).ok()?;
        let target = target.trim().strip_prefix("gitdir:")?.trim();
        let p = PathBuf::from(target);
        let p = if p.is_absolute() { p } else { dir.join(p) };
        p.join("config")
    };
    std::fs::read_to_string(cfg).ok()
}

/// Which suite repo a git config's `url =` lines point at, if any.
fn remote_repo(config: &str) -> Option<&'static str> {
    for line in config.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("url") else {
            continue;
        };
        let Some(url) = rest.trim().strip_prefix('=') else {
            continue;
        };
        let url = url.trim().trim_end_matches(".git");
        if !url.to_ascii_lowercase().contains("github.com") {
            continue;
        }
        // last two path components are org/name for https://, scp and ssh:// URLs
        let mut parts = url.rsplit(['/', ':']);
        let name = parts.next()?;
        let Some(_org) = parts.next() else {
            continue;
        };
        if let Some(repo) = SUITE
            .iter()
            .copied()
            .find(|&r| r.eq_ignore_ascii_case(name))
        {
            return Some(repo);
        }
    }
    None
}

/// One registered `.desktop` entry of a suite app.
pub struct MenuEntry {
    /// where the entry lives (`~/.local/share/applications/…` or a system dir)
    pub path: PathBuf,
    /// the executable it launches (from `Exec=`), when parseable
    pub pkg: Option<String>,
}

/// Every suite app with a desktop entry: repo → its `.desktop` file.
/// Reads the user's applications dir plus `$XDG_DATA_DIRS` (read-only), so
/// entries registered by this tool, the launcher or an AUR package all count.
pub fn menu_map() -> HashMap<&'static str, MenuEntry> {
    let mut dirs = vec![desk_dir()];
    let data_dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(
        data_dirs
            .split(':')
            .filter(|s| !s.is_empty())
            .map(|d| PathBuf::from(d).join("applications")),
    );
    menu_map_in(&dirs)
}

/// [`menu_map`] over explicit directories, so tests can point it at a temp tree.
pub fn menu_map_in(dirs: &[PathBuf]) -> HashMap<&'static str, MenuEntry> {
    let mut found = HashMap::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if desktop_hidden(&text) {
                continue;
            }
            if let Some(repo) = repo_of_desktop(&text, &path) {
                found.entry(repo).or_insert_with(|| MenuEntry {
                    pkg: desktop_exec(&text),
                    path,
                });
            }
        }
    }
    found
}

/// `Hidden=true` entries are removed from menus by the spec — don't detect them.
fn desktop_hidden(text: &str) -> bool {
    text.lines().any(|l| {
        let Some((k, v)) = l.split_once('=') else {
            return false;
        };
        k.trim() == "Hidden" && v.trim().eq_ignore_ascii_case("true")
    })
}

/// The executable a `.desktop` entry launches, as a plain name
/// (`Exec=photocraft %F` → `photocraft`, absolute paths → their file name).
fn desktop_exec(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() != "Exec" {
            continue;
        }
        let Some(token) = v.split_whitespace().next() else {
            continue;
        };
        let Some(base) = Path::new(token).file_name().and_then(|b| b.to_str()) else {
            continue;
        };
        return Some(plain_name(base).to_string());
    }
    None
}

/// Which suite app a `.desktop` file belongs to: its stem, the binary it
/// execs, or the launcher's `ai.storyteller.<repo>` icon / WM class.
fn repo_of_desktop(text: &str, path: &Path) -> Option<&'static str> {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        && let Some(repo) = suite_of(stem)
    {
        return Some(repo);
    }
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        let candidate = match k.trim() {
            "Exec" | "TryExec" => match v.split_whitespace().next() {
                Some(t) => t,
                None => continue,
            },
            "Icon" | "StartupWMClass" => v,
            _ => continue,
        };
        let base = match Path::new(candidate).file_name().and_then(|b| b.to_str()) {
            Some(b) => b,
            None => continue,
        };
        if let Some(repo) = suite_of(base) {
            return Some(repo);
        }
    }
    None
}

/// `photocraft` or `ai.storyteller.photocraft` → `photocraft`, if a suite name.
fn suite_of(name: &str) -> Option<&'static str> {
    SUITE
        .iter()
        .copied()
        .find(|&r| r == name || name.strip_prefix("ai.storyteller.") == Some(r))
}

/// Strip the launcher's `ai.storyteller.` prefix from a binary name.
fn plain_name(bin: &str) -> &str {
    bin.strip_prefix("ai.storyteller.").unwrap_or(bin)
}

/// Where an installed binary actually lives: `~/.local/bin` first (what this
/// tool installs), then cargo, then the system paths an AUR or launcher
/// install uses.
pub fn find_bin(pkg: &str) -> Option<PathBuf> {
    find_bin_in(
        &[
            bin_dir(),
            home().join(".cargo/bin"),
            "/usr/local/bin".into(),
            "/usr/bin".into(),
        ],
        pkg,
    )
}

/// [`find_bin`] over explicit directories, so tests can point it at a temp tree.
pub fn find_bin_in(dirs: &[PathBuf], pkg: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(pkg)).find(|p| p.is_file())
}

/// Send a subprocess's stdout/stderr to the repo's log file instead of the terminal.
///
/// The TUI owns the tty (raw mode + alternate screen): any child that inherits it
/// writes straight over the frame buffer and ratatui's diff never learns about it.
fn attach_log(repo: &str, cmd: &mut Command, mark: &str) -> Result<(), String> {
    use std::io::Write;
    let _ = std::fs::create_dir_all(log_dir());
    let path = log_file(repo);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "\n$ {mark}");
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    let out = opts.open(&path).map_err(|e| format!("log: {e}"))?;
    let err = out.try_clone().map_err(|e| format!("log: {e}"))?;
    cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err));
    Ok(())
}

fn child_env_path() -> String {
    let mut path = std::env::var("PATH").unwrap_or_default();
    let cargo_bin = home().join(".cargo/bin");
    if cargo_bin.is_dir() {
        path = format!("{}:{}", cargo_bin.display(), path);
    }
    path
}

fn pid_registry() -> &'static Arc<Mutex<HashMap<String, u32>>> {
    static REG: OnceLock<Arc<Mutex<HashMap<String, u32>>>> = OnceLock::new();
    REG.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn run_registered(repo: &str, cmd: &mut Command) -> Result<std::process::ExitStatus, String> {
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let pid = child.id();
    if let Ok(mut reg) = pid_registry().lock() {
        reg.insert(repo.to_string(), pid);
    }
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    if let Ok(mut reg) = pid_registry().lock() {
        reg.remove(repo);
    }
    Ok(status)
}

pub fn cancel(repo: &str) -> bool {
    let pid = pid_registry()
        .lock()
        .ok()
        .and_then(|r| r.get(repo).copied());
    let Some(pid) = pid else { return false };
    let _ = Command::new("kill")
        .args(["-TERM", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    true
}

/// Main desktop app package of a repo: the apps/* member that isn't a -cli or -web sibling
/// (craft-launcher hosts artcraft-launcher). Prefers the package named like the repo.
pub fn pkg_of(repo: &str, dir: &Path) -> Option<String> {
    let mut best: Option<String> = None;
    let mut cargos: Vec<PathBuf> = std::fs::read_dir(dir.join("apps"))
        .ok()?
        .filter_map(|e| Some(e.ok()?.path().join("Cargo.toml")))
        .collect();
    cargos.sort();
    for c in cargos {
        let parent = c.parent()?.file_name()?.to_string_lossy().to_string();
        if parent.ends_with("-cli") || parent.ends_with("-web") {
            continue;
        }
        let text = std::fs::read_to_string(&c).ok()?;
        let name = text
            .lines()
            .find_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))?;
        if name == repo {
            return Some(name.to_string());
        }
        if best.is_none() {
            best = Some(name.to_string());
        }
    }
    best
}

/// The clone's own version for `pkg` — the `version` line of the same
/// apps/* manifest [`pkg_of`] names, following `version.workspace = true`
/// into the root manifest's `[workspace.package]` (where every suite app
/// keeps it). The fallback signal for a stale install built before
/// revisions were recorded.
fn pkg_version_of(dir: &Path, pkg: &str) -> Option<String> {
    let mut cargos: Vec<PathBuf> = std::fs::read_dir(dir.join("apps"))
        .ok()?
        .filter_map(|e| Some(e.ok()?.path().join("Cargo.toml")))
        .collect();
    cargos.sort();
    for c in cargos {
        let Ok(text) = std::fs::read_to_string(&c) else {
            continue;
        };
        let name = text
            .lines()
            .find_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))?;
        if name != pkg {
            continue;
        }
        if let Some(v) = text
            .lines()
            .find_map(|l| l.strip_prefix("version = \"")?.strip_suffix('"'))
        {
            return Some(v.to_string());
        }
        let workspace = text
            .lines()
            .any(|l| l.starts_with("version.workspace") || l.starts_with("version = { workspace"));
        if !workspace {
            return None;
        }
        let root = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
        let mut in_section = false;
        for l in root.lines() {
            if l.starts_with('[') {
                in_section = l.trim() == "[workspace.package]";
                continue;
            }
            if in_section
                && let Some(v) = l.trim().strip_prefix("version = \"")
                && let Some(v) = v.strip_suffix('"')
            {
                return Some(v.to_string());
            }
        }
        return None;
    }
    None
}

/// How many units a build of `pkg` in `dir` will produce — the denominator
/// that lets a percentage start at zero instead of guessing.
///
/// Prefers the exact unit count persisted from the previous successful build
/// of the same repo ([`units_file`]); when there isn't one (first build) it
/// estimates: unique crates in `cargo tree`'s normal+build graph, plus the
/// units the tree cannot see — build scripts, and crates compiled for both
/// host and target — derived from `cargo metadata`'s resolve graph (see
/// [`metadata_units`]). On figcraft that turns a 404-crate tree into ~500
/// units against 484 real ones: a few over is the safe direction (the bar
/// parks near 97% instead of the tail going mute), and after the first
/// build the exact count is persisted and takes over.
///
/// Returns `0` on any failure (no lockfile, odd workspace, cargo missing) —
/// meaning "unknown", never a fabricated number, so the caller shows elapsed
/// time instead of a bar.
fn estimate_units(dir: &Path, repo: &str, pkg: &str) -> u64 {
    // Exact count from the last successful build of this repo.
    if let Ok(text) = std::fs::read_to_string(units_file(repo)) {
        if let Ok(n) = text.trim().parse::<u64>() {
            if n > 0 {
                return n;
            }
        }
    }
    // First build (or a stale/invalid cache): estimate from the dep graph.
    // `cargo tree` has no --release (it isn't a build command); the crate set
    // is the same for either profile, and the root package is already the
    // first line so it needs no +1.
    let mut cmd = Command::new("cargo");
    cmd.args([
        "tree",
        "-p",
        pkg,
        "--edges",
        "normal,build",
        "--prefix",
        "none",
    ])
    .current_dir(dir)
    .env("PATH", child_env_path())
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    let Ok(out) = cmd.output() else { return 0 };
    if !out.status.success() {
        return 0;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Normalize each line to `name vversion`: cargo decorates lines with
    // ` (proc-macro)`, a workspace path, or the ` (*)` shown-elsewhere
    // marker; shedding all of them collapses repeats to one crate and makes
    // the key match `cargo metadata`'s `name`+`version` fields.
    let mut crates: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for line in text.lines() {
        let name = tree_crate_name(line);
        if !name.is_empty() {
            crates.insert(name);
        }
    }
    // Correct the tree's blind spot: build scripts, host-side proc-macro
    // support, and crates compiled for both host and target (see
    // [`metadata_units`]). Metadata failure (old cargo, no index for the
    // lockfile) leaves the plain tree estimate — still a real number, just
    // a lower one.
    let meta = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(dir)
        .env("PATH", child_env_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let extra = match meta {
        Ok(out) if out.status.success() => {
            let meta_text = String::from_utf8_lossy(&out.stdout);
            metadata_units(&meta_text, pkg, &crates)
        }
        _ => 0,
    };
    crates.len() as u64 + extra
}

/// One `cargo tree` line normalized to `name vversion`.
///
/// Strips every trailing parenthesized decoration — the `(*)` shown-elsewhere
/// marker, ` (proc-macro)`, and a workspace member's path — so the same
/// crate always keys the same way. Crate names and versions never contain
/// parentheses, so the loop is safe.
fn tree_crate_name(line: &str) -> &str {
    let mut name = line.trim();
    while name.ends_with(')') {
        match name.rfind(" (") {
            Some(i) => name = &name[..i],
            None => break,
        }
    }
    name
}

/// Units `cargo tree` cannot show but cargo will compile, counted from
/// `cargo metadata`'s package list and resolve graph:
///
/// - one `custom-build` unit per crate with a `build.rs` — plus a second
///   when that crate is also compiled on the host side;
/// - a second lib unit for crates that are both target and host
///   dependencies (build-scripts and proc-macros drag their deps into the
///   host world, where features may resolve differently).
///
/// Only packages present in `crates` (the tree's `name vversion` set) are
/// counted — metadata also lists dev-dependencies and other platforms'
/// crates, which never build here. The model is structural, so it can land
/// a unit high when cargo merges identical host/target units; the estimate's
/// job is to never land far *below* reality, where the tail of the bar
/// would go mute.
///
/// Hand-scanned rather than parsed: in cargo's compact JSON a package object
/// is the only place `{"name":"…","version":"…","id":"…"}` appears
/// together (target and dependency objects have a name but no version), a
/// resolve node is the only `"id":"…"` pair after `"resolve":`, and each
/// edge's kinds are the exact strings `"kind":null` (normal) or
/// `"kind":"build"`.
fn metadata_units(json: &str, pkg: &str, crates: &std::collections::BTreeSet<&str>) -> u64 {
    use std::collections::{HashMap, HashSet};

    struct Pkg {
        name: String,
        version: String,
        is_pm: bool,
        has_cb: bool,
    }

    const NAME: &str = "{\"name\":\"";
    const VER: &str = "\",\"version\":\"";
    const ID: &str = "\",\"id\":\"";
    const CB: &str = "\"kind\":[\"custom-build\"]";
    const PM: &str = "\"kind\":[\"proc-macro\"]";

    // Phase A: packages, their ids, and their target kinds. Kind markers
    // attribute to the last package start; decoy name markers (dependency
    // and target objects) don't interrupt that.
    let mut pkgs: Vec<Pkg> = Vec::new();
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let mut root_name: Option<usize> = None;
    let mut root_path: Option<usize> = None;
    let mut current: Option<usize> = None;
    let mut attributed = 0usize;
    let mut i = 0usize;
    while let Some(rel) = json[i..].find(NAME) {
        let marker = i + rel;
        let region = &json[attributed..marker];
        if let Some(ci) = current {
            pkgs[ci].has_cb |= region.contains(CB);
            pkgs[ci].is_pm |= region.contains(PM);
        }
        let after = marker + NAME.len();
        let Some(nlen) = json[after..].find('"') else {
            break;
        };
        let name = json[after..after + nlen].to_string();
        let after_name = after + nlen;
        if json[after_name..].starts_with(VER) {
            let vstart = after_name + VER.len();
            let Some(vlen) = json[vstart..].find('"') else {
                break;
            };
            let version = json[vstart..vstart + vlen].to_string();
            let after_ver = vstart + vlen; // version's closing quote
            let mut id = "";
            if json[after_ver..].starts_with(ID) {
                let istart = after_ver + ID.len();
                let Some(ilen) = json[istart..].find('"') else {
                    break;
                };
                id = &json[istart..istart + ilen];
                i = istart + ilen + 1;
            } else {
                i = after_ver + 1;
            }
            let idx = pkgs.len();
            if name == pkg {
                root_name.get_or_insert(idx);
                if id.starts_with("path+") {
                    root_path.get_or_insert(idx);
                }
            }
            if !id.is_empty() {
                ids.insert(id, idx);
            }
            pkgs.push(Pkg {
                name,
                version,
                is_pm: false,
                has_cb: false,
            });
            current = Some(idx);
        } else {
            i = after_name;
        }
        attributed = i;
    }
    if let Some(ci) = current {
        let tail = &json[attributed..];
        pkgs[ci].has_cb |= tail.contains(CB);
        pkgs[ci].is_pm |= tail.contains(PM);
    }
    let Some(root_idx) = root_path.or(root_name) else {
        return 0;
    };

    // Phase B: resolve edges. Node ids are the only `"id":"…"` pairs after
    // `"resolve":`; each `{"pkg":"…","dep_kinds":[…]}` entry carries its
    // edge kinds right behind the package id.
    const NODE: &str = "\"id\":\"";
    const PKG: &str = "\"pkg\":\"";
    const KINDS: &str = "\"dep_kinds\":[";
    let mut nodes: Vec<(usize, usize)> = Vec::new(); // (package, marker offset)
    let Some(res) = json.find("\"resolve\":") else {
        return 0;
    };
    let mut j = res;
    while let Some(rel) = json[j..].find(NODE) {
        let marker = j + rel;
        let after = marker + NODE.len();
        let Some(ilen) = json[after..].find('"') else {
            break;
        };
        if let Some(&idx) = ids.get(&json[after..after + ilen])
            && nodes.last().map(|(p, _)| *p != idx).unwrap_or(true)
        {
            nodes.push((idx, marker));
        }
        j = after + ilen + 1;
    }
    let mut edges: HashMap<usize, Vec<(usize, bool, bool)>> = HashMap::new();
    for (n, &(idx, start)) in nodes.iter().enumerate() {
        let end = nodes.get(n + 1).map(|(_, s)| *s).unwrap_or(json.len());
        let span = &json[start..end];
        let mut k = 0usize;
        while let Some(pre) = span[k..].find(PKG) {
            let pmark = k + pre + PKG.len();
            let Some(plen) = span[pmark..].find('"') else {
                break;
            };
            let rest = &span[pmark + plen + 1..];
            let window_end = rest.find(PKG).unwrap_or(rest.len().min(400));
            let window = &rest[..window_end];
            k = pmark + plen + 1;
            let Some(&dep) = ids.get(&span[pmark..pmark + plen]) else {
                continue;
            };
            let (normal, build) = match window.find(KINDS) {
                Some(dk) => {
                    let ks = &window[dk + KINDS.len()..];
                    let kinds = &ks[..ks.find(']').unwrap_or(ks.len())];
                    (
                        kinds.contains("\"kind\":null"),
                        kinds.contains("\"kind\":\"build\""),
                    )
                }
                None => (true, false),
            };
            edges.entry(idx).or_default().push((dep, normal, build));
        }
    }

    // Phase C: the unit closures. Target = normal-edge reachability from the
    // root, stopping at proc-macros (their interiors are host-only). Host =
    // build-deps of the root and of every build-script package, closed over
    // normal+build edges, plus the normal deps of every target proc-macro.
    let mut target: HashSet<usize> = HashSet::new();
    let mut stack = vec![root_idx];
    while let Some(p) = stack.pop() {
        if !target.insert(p) || pkgs[p].is_pm {
            continue;
        }
        if let Some(es) = edges.get(&p) {
            stack.extend(es.iter().filter(|e| e.1).map(|e| e.0));
        }
    }
    let mut host: HashSet<usize> = HashSet::new();
    let mut stack: Vec<usize> = Vec::new();
    let seed_build = |p: usize, host: &mut HashSet<usize>, stack: &mut Vec<usize>| {
        if let Some(es) = edges.get(&p) {
            for &(dep, _, build) in es {
                if build && host.insert(dep) {
                    stack.push(dep);
                }
            }
        }
    };
    seed_build(root_idx, &mut host, &mut stack);
    for (p, pkg) in pkgs.iter().enumerate() {
        if pkg.has_cb {
            seed_build(p, &mut host, &mut stack);
        }
    }
    let drain = |host: &mut HashSet<usize>, stack: &mut Vec<usize>| {
        while let Some(p) = stack.pop() {
            if let Some(es) = edges.get(&p) {
                for &(dep, normal, build) in es {
                    if (normal || build) && host.insert(dep) {
                        stack.push(dep);
                    }
                }
            }
        }
    };
    drain(&mut host, &mut stack);
    loop {
        let before = host.len();
        for &p in &target {
            if pkgs[p].is_pm
                && let Some(es) = edges.get(&p)
            {
                for &(dep, normal, _) in es {
                    if normal && host.insert(dep) {
                        stack.push(dep);
                    }
                }
            }
        }
        drain(&mut host, &mut stack);
        if host.len() == before {
            break;
        }
    }

    let matched =
        |p: usize| crates.contains(format!("{} v{}", pkgs[p].name, pkgs[p].version).as_str());
    let mut extras = 0u64;
    for (p, pkg) in pkgs.iter().enumerate() {
        let dual = target.contains(&p) && host.contains(&p) && !pkg.is_pm && matched(p);
        extras += u64::from(dual);
        if pkg.has_cb && matched(p) {
            extras += 1;
            extras += u64::from(dual);
        }
    }
    extras
}

/// Remember a build's exact unit count so the next progress bar is precise.
fn save_units(repo: &str, units: u64) {
    if units == 0 {
        return;
    }
    let _ = std::fs::create_dir_all(log_dir());
    let _ = std::fs::write(units_file(repo), units.to_string());
}

/// Version string of an installed binary.
///
/// Bounded by [`VERSION_TIMEOUT`]: a future app that ignores `--version` and stays
/// alive would otherwise hang `scan()` and every rescan forever.
const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Per-repo deadline for the background upstream check ([`git_fetch_bounded`]).
/// A typical up-to-date fetch is well under a second; this only exists so a
/// blackholed network can't wedge the check phase. User-triggered fetches
/// (`f`, `u`) are never time-limited.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn version_of(bin: &Path) -> Option<String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + VERSION_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.as_mut()?.read_to_string(&mut out).ok()?;
    out.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Local state only: discovery, menu entries, binaries, versions — no git.
///
/// A clone's upstream state is filled in after [`git_fetch_bounded`] via
/// [`rescan_one`], so rows are never measured against the cached remote refs
/// that only a fetch (or a pull) refreshes.
pub fn scan() -> Vec<AppInfo> {
    let dirs = discover();
    let menus = menu_map();
    SUITE
        .iter()
        .map(|&repo| {
            let dir = dirs.get(repo).cloned().unwrap_or_else(|| default_dir(repo));
            info_of(repo, dir, menus.get(repo))
        })
        .collect()
}

/// One row's state, read fresh from disk: discovery result (`dir`), the clone,
/// the registered menu entry, the binary wherever it lives — and git, but only
/// when called right after a fetch ([`rescan_one`]).
fn info_of(repo: &'static str, dir: PathBuf, menu: Option<&MenuEntry>) -> AppInfo {
    let mut a = AppInfo {
        repo,
        dir,
        pkg: None,
        cloned: false,
        installed: false,
        version: None,
        behind: None,
        commits: Vec::new(),
        menu: false,
        stale: false,
    };
    refresh_local(&mut a, menu);
    a
}

/// Re-read one row after a fetch, a pull, a clone or a build: local facts and
/// git state both, where git is as fresh as the last fetch in this repo.
pub fn rescan_one(a: &mut AppInfo, track: Track) {
    // the clone's location was decided at scan time; everything else is re-read
    refresh_local(a, menu_map().get(a.repo));
    refresh_git(a, track);
}

/// Re-read one row's local facts when the fetch failed: no upstream state
/// (stale remote refs must not masquerade as current), but the install-vs-
/// clone staleness is still read — it only needs the local HEAD.
pub fn rescan_local(a: &mut AppInfo) {
    refresh_local(a, menu_map().get(a.repo));
    a.behind = None;
    a.commits = Vec::new();
}

fn refresh_local(a: &mut AppInfo, menu: Option<&MenuEntry>) {
    a.cloned = a.dir.join("Cargo.toml").is_file();
    // the clone knows its package; a menu-only install learns it from Exec=
    let mut pkg = a.cloned.then(|| pkg_of(a.repo, &a.dir)).flatten();
    if pkg.is_none() {
        pkg = menu.and_then(|m| m.pkg.clone());
    }
    let bin = pkg.as_ref().and_then(|p| find_bin(p));
    a.installed = bin.is_some();
    a.version = bin.as_ref().and_then(|b| version_of(b));
    a.pkg = pkg;
    a.menu = menu.is_some();
    // a local fact, needing no upstream: the binary against the clone's
    // current HEAD. Computed here so offline rescans and `--status` see it.
    a.stale = a.cloned && a.installed && install_is_stale(a.repo, &a.dir, a.version.as_deref());
}

fn refresh_git(a: &mut AppInfo, track: Track) {
    match track {
        Track::Commits => {
            a.behind = a.cloned.then(|| behind(&a.dir)).flatten();
            a.commits = if a.cloned {
                upstream_commits(&a.dir)
            } else {
                Vec::new()
            };
        }
        Track::Releases => {
            let state = a.cloned.then(|| releases_state(&a.dir)).flatten();
            a.behind = state.as_ref().map(|(n, _)| *n);
            a.commits = state.map(|(_, tags)| tags).unwrap_or_default();
        }
    }
}

/// The clone's current HEAD, as a full revision string.
fn head_rev(dir: &Path) -> Option<String> {
    run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"])
}

/// Whether the installed binary predates the clone's current HEAD — the
/// aftermath of a pull that moved the source on while the build was
/// cancelled or failed, leaving the previous binary in place. A recorded
/// build revision ([`installed_rev`]) is compared directly; installs from
/// before revisions were recorded fall back to the binary's `--version`
/// against the clone's package version — as a whole token, since the probe
/// prints "FigCraft 0.6.0" while the manifest says "0.6.0".
pub fn install_is_stale(repo: &str, dir: &Path, bin_version: Option<&str>) -> bool {
    if let (Some(recorded), Some(head)) = (installed_rev(repo), head_rev(dir)) {
        return recorded != head;
    }
    let Some(pkg) = pkg_of(repo, dir) else {
        return false;
    };
    let Some(src_version) = pkg_version_of(dir, &pkg) else {
        return false;
    };
    bin_version.is_some_and(|v| !v.split_whitespace().any(|t| t == src_version))
}

/// Whether the scan pipeline checks upstream on launch and `r`.
/// `ARTCRAFT_FETCH=never` (or `0`/`no`/`off`) keeps git calls user-triggered.
pub fn fetch_enabled() -> bool {
    match std::env::var("ARTCRAFT_FETCH") {
        Ok(v) => !matches!(
            v.to_ascii_lowercase().as_str(),
            "never" | "0" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Whether install/update completions raise a desktop notification.
/// `ARTCRAFT_NOTIFY=never` (or `0`/`no`/`off`) silences them.
pub fn notify_enabled() -> bool {
    notify_enabled_from(std::env::var("ARTCRAFT_NOTIFY").ok().as_deref())
}

/// The parsing half of [`notify_enabled`], split out so it's testable without
/// mutating the process environment (which would race under parallel tests).
fn notify_enabled_from(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => !matches!(
            v.to_ascii_lowercase().as_str(),
            "never" | "0" | "no" | "off"
        ),
        None => true,
    }
}

/// Raise a desktop notification via `notify-send` (libnotify → D-Bus).
///
/// Fire-and-forget on the caller's (already background) thread: the child is
/// spawned and dropped without waiting, so a slow or hung session bus can never
/// delay the completion path or the UI. A missing `notify-send` or absent
/// session bus is a silent no-op — the in-app status row already tells the same
/// story, so a failed notification is never an error.
fn notify(title: &str, body: &str) {
    if !notify_enabled() {
        return;
    }
    let _ = Command::new("notify-send")
        .args(["-a", "ArtCraft Suite", title, body])
        .env("PATH", child_env_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Serializes every git ref mutation (background fetch, `f`, `u`) so the
/// housekeeping check can never race a user action on the same repo's locks.
fn git_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Background housekeeping fetch: bounded so a dead network can't wedge the
/// check phase, quiet because the details pane belongs to user-triggered
/// tasks. On error the caller must leave the row "not checked" — stale refs
/// are exactly the lie this exists to remove.
pub fn git_fetch_bounded(dir: &Path) -> Result<(), String> {
    let _guard = git_lock();
    let mut cmd = Command::new("git");
    // --tags: a plain fetch can miss tags in a --depth 1 clone, and releases
    // mode reads them.
    cmd.args(["-C", &dir.to_string_lossy(), "fetch", "--tags"])
        .env("PATH", child_env_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let deadline = std::time::Instant::now() + FETCH_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return status
                    .success()
                    .then_some(())
                    .ok_or_else(|| "fetch failed".to_string());
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = Command::new("kill")
                    .args(["-TERM", &format!("-{}", child.id())])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.wait();
                return Err("fetch timed out".into());
            }
        }
    }
}

fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .env("PATH", child_env_path())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The ref to measure against: the branch's upstream (`@{u}`) when it
/// resolves, else the remote's default branch (`origin/HEAD`) — the fallback
/// a detached release checkout needs, since `@{u}` requires being on a branch.
fn upstream_ref(dir: &Path) -> Option<String> {
    if let Some(u) = run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "rev-parse",
            "--abbrev-ref",
            "@{u}",
        ],
    ) {
        if !u.is_empty() {
            return Some(u);
        }
    }
    run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "symbolic-ref",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )
}

/// Commits `HEAD` is behind its upstream, from the *cached* remote-tracking
/// ref — as fresh as the last fetch in this repo, never fresher. Callers are
/// expected to have fetched first ([`git_fetch_bounded`], `f`, or `u`).
fn behind(dir: &Path) -> Option<u64> {
    let up = upstream_ref(dir)?;
    let range = format!("HEAD..{up}");
    run_capture(
        "git",
        &["-C", &dir.to_string_lossy(), "rev-list", "--count", &range],
    )?
    .parse()
    .ok()
}

/// Upstream commits the next pull would bring, from the same cached refs as
/// [`behind`] — no network, so it is as fresh (or as stale) as the count.
fn upstream_commits(dir: &Path) -> Vec<String> {
    let Some(up) = upstream_ref(dir) else {
        return Vec::new();
    };
    let range = format!("HEAD..{up}");
    let out = run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "log",
            "--oneline",
            "-8",
            &range,
        ],
    );
    out.filter(|s| !s.is_empty())
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Release tags on the upstream that `HEAD` does not contain, newest first —
/// the releases-mode twin of [`behind`]. Tags must have been fetched
/// (`--tags` on every fetch path); `None` when there is no upstream to ask.
fn releases_state(dir: &Path) -> Option<(u64, Vec<String>)> {
    let up = upstream_ref(dir)?;
    let tags = run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "tag",
            "--merged",
            &up,
            "--sort=-v:refname",
        ],
    )?;
    let have: std::collections::HashSet<String> = run_capture(
        "git",
        &["-C", &dir.to_string_lossy(), "tag", "--merged", "HEAD"],
    )?
    .lines()
    .filter(|l| !l.is_empty())
    .map(str::to_string)
    .collect();
    let missing: Vec<String> = tags
        .lines()
        .filter(|t| !t.is_empty() && !have.contains(*t))
        .map(str::to_string)
        .collect();
    Some((missing.len() as u64, missing))
}

/// The newest release tag on the upstream, or `None` when there is no
/// upstream or no tag yet.
fn latest_release(dir: &Path) -> Option<String> {
    let up = upstream_ref(dir)?;
    let out = run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "tag",
            "--merged",
            &up,
            "--sort=-v:refname",
        ],
    )?;
    out.lines().find(|l| !l.is_empty()).map(str::to_string)
}

/// Whether `HEAD` already is `rev` (a tag or commit); `None` when either rev
/// fails to resolve.
fn head_is(dir: &Path, rev: &str) -> Option<bool> {
    let head = run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"])?;
    let other = run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", rev])?;
    Some(head == other)
}

fn git_pull(repo: &str, dir: &Path) -> Result<(), String> {
    // Detached (e.g. after a releases-mode checkout): return to the remote's
    // default branch first, so switching back to commits mode lands on the
    // branch instead of a cryptic pull failure.
    if run_capture(
        "git",
        &["-C", &dir.to_string_lossy(), "symbolic-ref", "-q", "HEAD"],
    )
    .is_none()
        && let Some(branch) = default_branch(dir)
    {
        git_switch(repo, dir, &branch)?;
    }
    let _guard = git_lock();
    let mut cmd = Command::new("git");
    cmd.args(["-C", &dir.to_string_lossy(), "pull", "--ff-only"])
        .env("PATH", child_env_path());
    attach_log(repo, &mut cmd, "git pull --ff-only")?;
    let s = run_registered(repo, &mut cmd)?;
    if s.signal().is_some() {
        return Err("cancelled".into());
    }
    s.success()
        .then_some(())
        .ok_or_else(|| "git pull failed (local changes?)".into())
}

/// The short name of the remote's default branch (`origin/main` → `main`).
fn default_branch(dir: &Path) -> Option<String> {
    let r = run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "symbolic-ref",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )?;
    Some(r.strip_prefix("origin/")?.to_string())
}

fn git_switch(repo: &str, dir: &Path, branch: &str) -> Result<(), String> {
    let _guard = git_lock();
    let mut cmd = Command::new("git");
    cmd.args(["-C", &dir.to_string_lossy(), "switch", branch])
        .env("PATH", child_env_path());
    attach_log(repo, &mut cmd, &format!("git switch {branch}"))?;
    let s = run_registered(repo, &mut cmd)?;
    if s.signal().is_some() {
        return Err("cancelled".into());
    }
    s.success()
        .then_some(())
        .ok_or_else(|| format!("git switch {branch} failed"))
}

/// Move `HEAD` onto a release tag (detached). The branch pointer is untouched
/// — nothing is lost and commits mode returns to the branch — and a dirty
/// tree fails loudly, the same contract as an ff-only pull.
fn git_checkout_release(repo: &str, dir: &Path, tag: &str) -> Result<(), String> {
    let _guard = git_lock();
    let mut cmd = Command::new("git");
    cmd.args(["-C", &dir.to_string_lossy(), "checkout", tag])
        .env("PATH", child_env_path());
    attach_log(repo, &mut cmd, &format!("git checkout {tag}"))?;
    let s = run_registered(repo, &mut cmd)?;
    if s.signal().is_some() {
        return Err("cancelled".into());
    }
    s.success()
        .then_some(())
        .ok_or_else(|| "git checkout failed (local changes?)".into())
}

fn git_org() -> String {
    std::env::var("ARTCRAFT_GIT_ORG").unwrap_or_else(|_| "storytold".into())
}

fn git_clone(repo: &str, dir: &Path) -> Result<(), String> {
    let existed = dir.exists();
    let mut cmd = Command::new("git");
    cmd.args([
        "clone",
        "--depth",
        "1",
        &format!("https://github.com/{}/{}", git_org(), repo),
        &dir.to_string_lossy(),
    ])
    .env("PATH", child_env_path());
    // a clone starts a fresh build cycle: drop the previous run's log
    let _ = std::fs::remove_file(log_file(repo));
    attach_log(repo, &mut cmd, "git clone")?;
    let s = run_registered(repo, &mut cmd)?;
    if s.signal().is_some() || !s.success() {
        if !existed {
            let _ = std::fs::remove_dir_all(dir);
        }
        return Err(if s.signal().is_some() {
            "cancelled".into()
        } else {
            format!("clone of {repo} failed")
        });
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &t)?;
        } else {
            std::fs::copy(e.path(), t)?;
        }
    }
    Ok(())
}

/// Best-effort refresh of the desktop entry and icon caches.
fn refresh_desktop_caches() {
    let _ = Command::new("update-desktop-database")
        .arg(desk_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = Command::new("gtk-update-icon-cache")
        .args(["-f", "-t"])
        .arg(icon_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Remove every file under `from`'s tree at the same relative path under `to`,
/// counting successful deletions (mirrors `copy_tree`).
fn remove_tree_files(from: &Path, to: &Path, removed: &mut usize) {
    if let Ok(rd) = std::fs::read_dir(from) {
        for e in rd.flatten() {
            let t = to.join(e.file_name());
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_tree_files(&e.path(), &t, removed);
            } else if std::fs::remove_file(&t).is_ok() {
                *removed += 1;
            }
        }
    }
}

/// Register the app with the desktop: its `.desktop` entry and hicolor icons
/// into the user's applications/icons directories, then refresh both caches.
/// Returns whether an entry was actually copied.
fn install_menu_entry(dir: &Path) -> bool {
    let mut registered = false;
    let desk = dir.join("packaging/linux");
    if let Ok(rd) = std::fs::read_dir(&desk) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "desktop") {
                let _ = std::fs::create_dir_all(desk_dir());
                if let Some(name) = p.file_name() {
                    let _ = std::fs::copy(&p, desk_dir().join(name));
                    registered = true;
                }
                break;
            }
        }
    }
    let icons = dir.join("assets/app-icon/hicolor");
    if icons.is_dir() {
        let _ = copy_tree(&icons, &icon_dir());
    }
    refresh_desktop_caches();
    registered
}

/// Remove what an install puts on the system: the binary in `~/.local/bin`,
/// the desktop entry and the hicolor icons — and the clone, when there is one.
/// The clone goes last, so a failed removal of the rest never costs the source.
pub fn uninstall(repo: &str, dir: &Path) -> Result<String, String> {
    let menus = menu_map();
    let pkg = pkg_of(repo, dir)
        .or_else(|| menus.get(repo).and_then(|m| m.pkg.clone()))
        .ok_or("no clone and no menu entry - nothing to uninstall")?;
    let bin = bin_dir().join(&pkg);
    let had_bin = bin.is_file();
    if had_bin {
        std::fs::remove_file(&bin).map_err(|e| format!("remove {bin:?}: {e}"))?;
    }
    // the registered entry, wherever this tool or the launcher put it — but
    // only the user's own copy; a system entry belongs to its package manager
    let mut desk_removed = false;
    if let Some(entry) = menus.get(repo)
        && entry.path.starts_with(desk_dir())
    {
        desk_removed |= std::fs::remove_file(&entry.path).is_ok();
    }
    let desk = dir.join("packaging/linux");
    if let Ok(rd) = std::fs::read_dir(&desk) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "desktop") {
                if let Some(name) = p.file_name() {
                    desk_removed |= std::fs::remove_file(desk_dir().join(name)).is_ok();
                }
                break;
            }
        }
    }
    let mut icons_removed = 0usize;
    remove_tree_files(
        &dir.join("assets/app-icon/hicolor"),
        &icon_dir(),
        &mut icons_removed,
    );
    refresh_desktop_caches();
    if !had_bin && !desk_removed && icons_removed == 0 {
        return Err("nothing installed to remove".into());
    }
    // The remembered unit count describes this install's build; with the app
    // gone, drop the seed so a future install re-estimates instead of
    // trusting a count from an uninstall that may never come back the same.
    let _ = std::fs::remove_file(units_file(repo));
    // likewise the build revision: a future install starts unjudged, not
    // stale against a clone that may have moved on in the meantime.
    let _ = std::fs::remove_file(installed_rev_file(repo));
    // only a real clone is deleted: a menu-only install has no source here,
    // and `dir` then just points at where a future clone would land
    let had_clone = dir.join("Cargo.toml").is_file();
    if had_clone {
        std::fs::remove_dir_all(dir).map_err(|e| format!("remove clone {dir:?}: {e}"))?;
    }
    Ok(if had_clone {
        format!("removed {pkg} + clone")
    } else {
        format!("removed {pkg}")
    })
}

/// Read a build's output line by line, tee every line to the shared log, and
/// — on the JSON (stdout) side — count cargo's artifacts into the live
/// [`Progress`].
///
/// `seed` is the pre-computed denominator ([`estimate_units`]); it's sent as a
/// `done: 0` event before the first crate so the bar renders empty from the
/// start. Every `compiler-artifact` (fresh or not) bumps `seen`, only a
/// non-`fresh` one bumps `done` and samples the pace, and
/// `build-script-executed` lines sample the pace alone; a `Progress` event
/// goes out on each of those. The function returns how many
/// `compiler-artifact` events it saw in total — the exact unit count, worth
/// persisting so the next build's denominator is precise rather than a tree
/// estimate.
///
/// The counting is what turns "still building" into a real percentage: cargo
/// prints one `compiler-artifact` object per produced crate, and `fresh: true`
/// marks crates it skipped from cache (so they aren't work in progress).
fn pump_build_lines(
    stream: Option<impl Read>,
    repo: &str,
    log_handle: &Arc<Mutex<File>>,
    tx: &Sender<Ev>,
    count: bool,
    seed: u64,
) -> u64 {
    let Some(stream) = stream else { return 0 };
    let mut progress = Progress {
        done: 0,
        seen: 0,
        total: seed,
        started: Instant::now(),
        pace_secs: None,
        last_real: None,
    };
    if count && seed > 0 {
        // Show the empty bar immediately, before the first crate finishes.
        let _ = tx.send(Ev::Progress {
            repo: repo.to_string(),
            progress: progress.clone(),
        });
    }
    let mut units = 0;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if let Ok(mut f) = log_handle.lock() {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
        if count && is_artifact(&line) {
            // Every artifact event (fresh or not) is one unit cargo produced.
            units += 1;
            progress.seen += 1;
            // Only a non-fresh artifact is real progress toward done — and
            // only real work informs the pace (fresh bursts would lie).
            if artifact_progress(&line).is_some() {
                progress.done += 1;
                progress.observe(Instant::now());
            }
            let _ = tx.send(Ev::Progress {
                repo: repo.to_string(),
                progress: progress.clone(),
            });
        } else if count && is_build_script(&line) {
            // Build scripts are among the slowest units but emit no artifact
            // while they run — sample the pace anyway so the ETA keeps
            // moving during a script-heavy stretch.
            progress.observe(Instant::now());
            let _ = tx.send(Ev::Progress {
                repo: repo.to_string(),
                progress: progress.clone(),
            });
        }
    }
    units
}

/// True for any `compiler-artifact` event — cargo's per-crate "I produced
/// this" line, whether it just compiled it or served it fresh from cache.
fn is_artifact(line: &str) -> bool {
    line.contains("\"compiler-artifact\"")
}

/// True for cargo's `build-script-executed` event — a `build.rs` run, real
/// compile time that produces no artifact of its own. Used only to sample
/// the pace; it never counts toward `done`/`seen`.
fn is_build_script(line: &str) -> bool {
    line.contains("\"reason\":\"build-script-executed\"")
}

/// Parse one `--message-format=json` line: `Some(1)` when it's a freshly
/// compiled artifact, else `None`.
///
/// Cargo never emits a total — it prints one `compiler-artifact` object per
/// crate as it compiles, so there is no denominator to read here. The caller
/// supplies one up front ([`estimate_units`]) and treats each non-`fresh`
/// artifact as one crate of real work done.
///
/// Deliberately a hand-parse rather than a JSON dependency: the two fields we
/// need (`reason`, `fresh`) are flat top-level scalars, so a scan is honest and
/// keeps the package dependency-free.
fn artifact_progress(line: &str) -> Option<u64> {
    if !is_artifact(line) {
        return None;
    }
    // A cached crate is `fresh: true` — cargo skipped it, so it isn't progress.
    if line.contains("\"fresh\":true") {
        return None;
    }
    Some(1)
}

fn build_install(dir: &Path, repo: &str, tx: &Sender<Ev>) -> Result<String, String> {
    let pkg = pkg_of(repo, dir).ok_or("no desktop app package found under apps/")?;
    // Real denominator before the build starts: exact count from the last
    // successful build, or a cargo-tree estimate on the first one. `0` means
    // "unknown" and degrades to an elapsed-time line, never a lying full bar.
    let seed = estimate_units(dir, repo, &pkg);
    let _ = std::fs::create_dir_all(log_dir());
    let log = log_file(repo);
    let mut cmd = Command::new("cargo");
    // `--message-format=json` makes cargo emit one `compiler-artifact` JSON
    // line per produced crate — the only honest signal for a percentage.
    cmd.args(["build", "--release", "-p", &pkg, "--message-format=json"])
        .current_dir(dir)
        .env("PATH", child_env_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let pid = child.id();
    if let Ok(mut reg) = pid_registry().lock() {
        reg.insert(repo.to_string(), pid);
    }
    // Both streams tee to the same log (the details pane and log_tail read it).
    // Each reader runs on its own thread so neither pipe can fill and deadlock
    // the other; a Mutex<File> serializes their appends. Only stdout carries
    // cargo's JSON, so only that side counts artifacts and emits progress.
    let log_handle = Arc::new(Mutex::new(
        File::create(&log).map_err(|e| format!("log: {e}"))?,
    ));
    let out = child.stdout.take();
    let err = child.stderr.take();
    let repo_owned = repo.to_string();
    let out_handle = {
        let log_handle = Arc::clone(&log_handle);
        let repo = repo_owned.clone();
        let tx = tx.clone();
        std::thread::spawn(move || pump_build_lines(out, &repo, &log_handle, &tx, true, seed))
    };
    let err_handle = {
        let log_handle = Arc::clone(&log_handle);
        let repo = repo_owned.clone();
        let tx = tx.clone();
        std::thread::spawn(move || pump_build_lines(err, &repo, &log_handle, &tx, false, 0))
    };
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    if let Ok(mut reg) = pid_registry().lock() {
        reg.remove(repo);
    }
    // Total units cargo produced (fresh + compiled) — the exact denominator,
    // persisted so the next build's bar is precise instead of tree-estimated.
    let units = out_handle.join().unwrap_or(0);
    let _ = err_handle.join();
    if status.signal().is_some() {
        return Err("cancelled".into());
    }
    if !status.success() {
        return Err(format!("build failed - see {}", log.display()));
    }
    save_units(repo, units);
    let src = dir.join("target/release").join(&pkg);
    let dst = bin_dir().join(&pkg);
    std::fs::create_dir_all(bin_dir()).map_err(|e| e.to_string())?;
    std::fs::copy(&src, &dst).map_err(|e| format!("install {dst:?}: {e}"))?;
    // The binary on disk came from this exact revision — remember it, so a
    // later pull can tell a rebuilt install from a stale one.
    if let Some(head) = head_rev(dir) {
        save_installed_rev(repo, &head);
    }
    let menu = install_menu_entry(dir);
    let v = version_of(&dst).unwrap_or_else(|| "ok".into());
    Ok(if menu {
        format!("installed {pkg} ({v}) · added to app menu")
    } else {
        format!("installed {pkg} ({v})")
    })
}

/// User-triggered fetch: logged to the repo's cache log and cancellable with
/// `x`. Tags come along in both modes — releases mode reads them.
fn git_fetch_logged(repo: &str, dir: &Path) -> Result<(), String> {
    let _guard = git_lock();
    let mut cmd = Command::new("git");
    cmd.args(["-C", &dir.to_string_lossy(), "fetch", "--tags"])
        .env("PATH", child_env_path());
    attach_log(repo, &mut cmd, "git fetch --tags")?;
    let s = run_registered(repo, &mut cmd)?;
    if s.signal().is_some() {
        return Err("cancelled".into());
    }
    s.success()
        .then_some(())
        .ok_or_else(|| "git fetch failed".into())
}

/// Fetch tags, then land `HEAD` on the newest release tag. Already there is
/// the no-op; an older tag (or a clone that ran ahead on the branch) moves to
/// the release — forward or back — without touching the branch pointer.
fn update_to_release(repo: &str, dir: &Path) -> Result<(), String> {
    git_fetch_logged(repo, dir)?;
    let tag = latest_release(dir).ok_or_else(|| "no release tags found".to_string())?;
    if head_is(dir, &tag) == Some(true) {
        return Ok(());
    }
    git_checkout_release(repo, dir, &tag)
}

pub fn run_action(
    repo: &str,
    dir: &Path,
    action: Action,
    track: Track,
    tx: &Sender<Ev>,
) -> Result<String, String> {
    match action {
        Action::Install => {
            if !dir.join("Cargo.toml").is_file() {
                git_clone(repo, dir)?;
                // A pristine, tool-owned clone may be moved onto the release
                // safely; an existing clone's HEAD is never touched by `i`.
                if track == Track::Releases
                    && let Some(tag) = latest_release(dir)
                    && head_is(dir, &tag) == Some(false)
                {
                    git_checkout_release(repo, dir, &tag)?;
                }
            }
            build_install(dir, repo, tx)
        }
        Action::Update => {
            if !dir.join("Cargo.toml").is_file() {
                return Err("not cloned - install it first".into());
            }
            let before = head_rev(dir);
            match track {
                Track::Commits => git_pull(repo, dir)?,
                Track::Releases => update_to_release(repo, dir)?,
            }
            let after = head_rev(dir);
            let pkg = pkg_of(repo, dir);
            let bin = pkg.as_ref().and_then(|p| find_bin(p));
            // "up to date" must mean the *install* is current too: a clone
            // that already moved on (a previous pull whose build never
            // finished) carries a stale binary that `u` has to rebuild,
            // not skip.
            let stale = bin
                .as_ref()
                .is_some_and(|b| install_is_stale(repo, dir, version_of(b).as_deref()));
            if before == after && bin.is_some() && !stale {
                return Ok(format!("{repo}: up to date"));
            }
            build_install(dir, repo, tx)
        }
        Action::Fetch => {
            if !dir.join("Cargo.toml").is_file() {
                return Err("not cloned".into());
            }
            git_fetch_logged(repo, dir)?;
            Ok(match track {
                Track::Commits => match behind(dir) {
                    Some(n) => format!("{repo}: behind {n} commit(s)"),
                    None => format!("{repo}: fetched (no upstream?)"),
                },
                Track::Releases => match releases_state(dir) {
                    Some((n, _)) => format!("{repo}: behind {n} release(s)"),
                    None => format!("{repo}: fetched (no upstream?)"),
                },
            })
        }
    }
}

pub fn spawn(repo: String, dir: PathBuf, action: Action, track: Track, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let result = run_action(&repo, &dir, action, track, &tx);
        // Desktop notification for the outcomes the user walked away from:
        // success, failure and the "up to date" no-op all count. Fetch is
        // quick and user-triggered, so it stays quiet.
        if matches!(action, Action::Install | Action::Update) {
            let body = match &result {
                Ok(m) => m.clone(),
                Err(e) => format!("{repo} · {}: {e}", action.label()),
            };
            notify(&repo, &body);
        }
        let _ = tx.send(Ev::Done {
            repo,
            action,
            result,
        });
    });
}

pub fn launch(pkg: &str) -> Result<(), String> {
    let bin = find_bin(pkg).ok_or_else(|| format!("{pkg} is not installed"))?;
    let mut child = Command::new(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("PATH", child_env_path())
        .spawn()
        .map_err(|e| format!("launch: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(150));
    match child.try_wait() {
        Ok(Some(status)) if !status.success() => {
            Err(format!("{pkg} exited immediately ({status})"))
        }
        _ => Ok(()),
    }
}

/// Last `want` bytes of a log file for the tail pane.
///
/// Decoded lossily: a build log can contain invalid UTF-8, and a seek into the
/// middle of a multi-byte character would make `read_to_string` fail and blank
/// the whole pane.
pub fn log_tail(repo: &str, want: u64) -> Option<String> {
    let mut f = File::open(log_file(repo)).ok()?;
    let len = f.metadata().ok()?.len();
    if len > want {
        f.seek(SeekFrom::End(-(want as i64))).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

pub fn busy_summary(busy: &HashMap<String, String>) -> Option<String> {
    if busy.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = busy.iter().map(|(k, v)| format!("{v} {k}")).collect();
    parts.sort();
    Some(parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A `Progress` whose clock reads `elapsed` seconds ago. `seen` counts
    /// every artifact emitted so far, `done` the non-fresh subset of them.
    fn progress_at(done: u64, seen: u64, total: u64, elapsed: u64) -> Progress {
        Progress {
            done,
            seen,
            total,
            started: Instant::now() - Duration::from_secs(elapsed),
            pace_secs: None,
            last_real: None,
        }
    }

    #[test]
    fn progress_pct_weights_done_against_real_work_left() {
        // No total means no honest percentage — the UI shows elapsed time.
        assert_eq!(
            progress_at(0, 0, 0, 0).pct(),
            None,
            "no total, no percentage"
        );
        // A seeded build starts at zero with an empty bar.
        assert_eq!(progress_at(0, 0, 80, 0).pct(), Some(0));
        // Cold build: nothing was fresh, so 43 of 80 crates is 53% — the
        // classic done-over-total reading.
        assert_eq!(progress_at(43, 43, 80, 0).pct(), Some(53));
        // Warm-build regression: 400 fresh crates streamed by, only 10 real
        // ones compiled of 449 expected. done/total would read 2%; the bar
        // instead weighs done against the work actually left
        // (10 / (10 + 449 − 410) ≈ 20%).
        assert_eq!(progress_at(10, 410, 449, 0).pct(), Some(20));
        // The seed ran out mid-build (cargo-tree undercounts): hold at 99%,
        // never claim completion before the build reports Done.
        assert_eq!(progress_at(90, 80, 80, 0).pct(), Some(99));
        // Everything fresh and nothing compiled (a no-op rebuild): no
        // evidence of real work, so no over-claim either.
        assert_eq!(progress_at(0, 80, 80, 0).pct(), Some(0));
    }

    #[test]
    fn progress_eta_is_unresolved_units_times_recent_pace() {
        // All pace gaps are measured from one anchor so the arithmetic is
        // exact (two separate now() calls would add stray nanoseconds).
        let t0 = Instant::now();

        // Cold build: 10 compiled, none fresh, 30 left at 10s/crate → 300s.
        let mut cold = progress_at(10, 10, 40, 0);
        cold.observe(t0);
        cold.observe(t0 + Duration::from_secs(10));
        assert_eq!(cold.pace_secs, Some(10.0));
        assert_eq!(cold.eta_secs(), Some(300));

        // Warm-build regression: 400 fresh streamed by, 10 real compiled at
        // a steady 15s/crate. The old elapsed-rate formula claimed ~2h here
        // (it extrapolated 439 "left" at the rate of 10 crates); unresolved
        // units × pace says ~10 min.
        let mut warm = progress_at(10, 410, 449, 0);
        warm.observe(t0);
        warm.observe(t0 + Duration::from_secs(15));
        assert_eq!(warm.eta_secs(), Some(39 * 15));

        // The EWMA keeps recent crates dominant: gaps 10s then 20s →
        // 0.3·20 + 0.7·10 = 13s per unit, 30 left → 390s.
        let mut ewma = progress_at(10, 10, 40, 0);
        ewma.observe(t0);
        ewma.observe(t0 + Duration::from_secs(10));
        ewma.observe(t0 + Duration::from_secs(30));
        assert_eq!(ewma.pace_secs, Some(13.0));
        assert_eq!(ewma.eta_secs(), Some(30 * 13));

        // No pace yet (fewer than two real samples) — the pre-build phase
        // shows elapsed time, never a guess.
        assert_eq!(
            progress_at(10, 10, 40, 0).eta_secs(),
            None,
            "no pace, no estimate"
        );
        // Nothing unresolved: the build is over, not "0s left".
        let mut exhausted = progress_at(10, 40, 40, 0);
        exhausted.observe(t0);
        exhausted.observe(t0 + Duration::from_secs(5));
        assert_eq!(exhausted.eta_secs(), None, "nothing left");
        // A pace without a denominator is equally untrustworthy.
        let mut blind = progress_at(10, 10, 0, 0);
        blind.observe(t0);
        blind.observe(t0 + Duration::from_secs(5));
        assert_eq!(blind.eta_secs(), None, "no total");
    }

    #[test]
    fn artifact_progress_skips_fresh_and_non_artifact_lines() {
        assert_eq!(
            artifact_progress(r#"{"reason":"compiler-artifact","fresh":false}"#),
            Some(1)
        );
        assert_eq!(
            artifact_progress(r#"{"reason":"compiler-artifact","fresh":true}"#),
            None,
            "a cached crate is not progress"
        );
        assert_eq!(artifact_progress(r#"{"reason":"compiler-message"}"#), None);
        assert_eq!(artifact_progress("not json at all"), None);
    }

    #[test]
    fn is_artifact_matches_any_compiler_artifact() {
        // Every artifact event (fresh or not) is one unit cargo produced, so
        // `is_artifact` is the wider net; `artifact_progress` is the subset
        // that counts as real work done.
        assert!(is_artifact(
            r#"{"reason":"compiler-artifact","fresh":false}"#
        ));
        assert!(is_artifact(
            r#"{"reason":"compiler-artifact","fresh":true}"#
        ));
        assert!(!is_artifact(r#"{"reason":"compiler-message"}"#));
        assert!(!is_artifact("not json at all"));
    }

    #[test]
    fn build_script_events_match_and_nothing_else_does() {
        // A build.rs run is real compile time with no artifact of its own —
        // it samples the pace but never counts as done or seen.
        assert!(is_build_script(
            r#"{"reason":"build-script-executed","package_id":"foo","linked_libs":[]}"#
        ));
        assert!(!is_build_script(
            r#"{"reason":"compiler-artifact","fresh":false}"#
        ));
        assert!(!is_build_script(r#"{"reason":"compiler-message"}"#));
        assert!(!is_build_script("not json at all"));
    }

    #[test]
    fn pump_counts_every_artifact_and_paces_only_from_real_work() {
        // Cargo-shaped feed: a build-script run, a cached crate, and two
        // compiled ones. Fresh events must be counted *and* reported (they
        // resolve units for free); only `done` and the pace stay untouched.
        let feed = concat!(
            r#"{"reason":"build-script-executed"}"#,
            "\n",
            r#"{"reason":"compiler-artifact","fresh":true}"#,
            "\n",
            r#"{"reason":"compiler-artifact","fresh":false}"#,
            "\n",
            r#"{"reason":"compiler-artifact","fresh":false}"#,
            "\n",
        );
        let repo = "engine-test-pump";
        let log_path = log_dir().join(format!("{repo}.log"));
        let log = Arc::new(Mutex::new(File::create(&log_path).expect("log")));
        let (tx, rx) = std::sync::mpsc::channel();
        let units = pump_build_lines(
            Some(std::io::Cursor::new(feed.as_bytes())),
            repo,
            &log,
            &tx,
            true,
            40,
        );
        drop(tx);
        let mut events = Vec::new();
        while let Ok(ev) = rx.recv() {
            if let Ev::Progress { repo: r, progress } = ev {
                assert_eq!(r, repo);
                events.push(progress);
            }
        }
        assert_eq!(units, 3, "only compiler-artifact lines count as units");
        assert_eq!(
            events.len(),
            5,
            "seed + build-script + 3 artifacts must all report"
        );
        let last = events.last().expect("progress events");
        assert_eq!(
            (last.done, last.seen, last.total),
            (2, 3, 40),
            "two compiled, three seen, seed unchanged"
        );
        assert!(
            last.pace_secs.is_some(),
            "a script run plus two crates must seed a pace"
        );
        assert!(
            events.iter().any(|p| p.seen == 1 && p.done == 0),
            "the fresh crate must be reported as an event of its own"
        );
        cleanup(&log_path);
    }

    #[test]
    fn a_persisted_unit_count_seeds_the_next_build() {
        // After a successful build the exact unit count is remembered, and the
        // next build reads it back instead of re-estimating from cargo tree.
        // The persisted file short-circuits before cargo tree runs, so a bogus
        // directory proves no tree call is needed.
        let repo = "engine-test-units";
        save_units(repo, 536);
        assert_eq!(
            estimate_units(Path::new("/nonexistent"), repo, "pkg"),
            536,
            "the persisted exact count must win over a tree estimate"
        );
        cleanup(&units_file(repo));
    }

    #[test]
    fn an_unknown_denominator_is_zero_not_a_guess() {
        // No persisted count and no cargo tree (bogus dir): report 0 — the
        // caller shows elapsed time rather than a bar built on a fabrication.
        let repo = "engine-test-units-unknown";
        let _ = std::fs::remove_file(units_file(repo));
        assert_eq!(estimate_units(Path::new("/nonexistent"), repo, "pkg"), 0);
    }

    #[test]
    fn tree_lines_shed_every_cargo_decoration() {
        // Same crate, four decorations: the key must be `name vversion` so
        // repeats collapse and metadata can match it.
        assert_eq!(tree_crate_name("serde v1.0.219"), "serde v1.0.219");
        assert_eq!(tree_crate_name("serde v1.0.219 (*)"), "serde v1.0.219");
        assert_eq!(
            tree_crate_name("bytemuck_derive v1.12.1 (proc-macro)"),
            "bytemuck_derive v1.12.1"
        );
        assert_eq!(
            tree_crate_name("figcraft v0.6.0 (/home/u/figcraft/apps/figcraft)"),
            "figcraft v0.6.0"
        );
        assert_eq!(
            tree_crate_name("  ahash v0.8.12 (*)  "),
            "ahash v0.8.12",
            "whitespace and the marker go together"
        );
    }

    #[test]
    fn a_first_build_estimate_never_lands_under_the_real_unit_count() {
        // Live check against a real suite clone (skipped when it isn't
        // there): figcraft's first build produced 484 cargo units, and the
        // tree+metadata estimate must reach at least that — landing under
        // is what made the tail of the bar go mute. The persisted exact
        // count is set aside so the estimator actually runs.
        let dir = home().join("figcraft");
        if !dir.join("Cargo.toml").is_file() {
            return;
        }
        let _ = std::fs::rename(units_file("figcraft"), units_file("figcraft.bak"));
        let n = estimate_units(&dir, "figcraft", "figcraft");
        let _ = std::fs::rename(units_file("figcraft.bak"), units_file("figcraft"));
        assert!(
            (484..=520).contains(&n),
            "estimate must cover the 484 real units without running away: {n}"
        );
    }

    #[test]
    fn metadata_units_counts_build_scripts_and_dual_compiled_crates() {
        // Cargo-shaped compact JSON with a resolve graph: `dual` and `mixed`
        // are both target and host deps (second lib unit); `inner`'s build
        // script runs for its proc-macro consumer; `solo` is a build-dep
        // only (one host unit); `ghost` is a dev-dep and must not count.
        let json = concat!(
            r#"{"packages":["#,
            r#"{"name":"app","version":"0.1.0","id":"path+file:///app#app@0.1.0","#,
            r#""targets":[{"kind":["bin"],"name":"app"}],"dependencies":[]},"#,
            r#"{"name":"dual","version":"1.0.0","id":"registry+d#dual@1.0.0","#,
            r#""targets":[{"kind":["lib"],"name":"dual"}]},"#,
            r#"{"name":"pm","version":"1.0.0","id":"registry+p#pm@1.0.0","#,
            r#""targets":[{"kind":["proc-macro"],"name":"pm"}]},"#,
            r#"{"name":"inner","version":"1.0.0","id":"registry+i#inner@1.0.0","#,
            r#""targets":[{"kind":["custom-build"],"name":"build-script-build"},#,
            r#"{"kind":["lib"],"name":"inner"}]},"#,
            r#"{"name":"solo","version":"1.0.0","id":"registry+s#solo@1.0.0","#,
            r#""targets":[{"kind":["custom-build"],"name":"build-script-build"},#,
            r#"{"kind":["lib"],"name":"solo"}]},"#,
            r#"{"name":"mixed","version":"1.0.0","id":"registry+m#mixed@1.0.0","#,
            r#""targets":[{"kind":["custom-build"],"name":"build-script-build"},#,
            r#"{"kind":["lib"],"name":"mixed"}]},"#,
            r#"{"name":"ghost","version":"9.9.9","id":"registry+g#ghost@9.9.9","#,
            r#""targets":[{"kind":["custom-build"],"name":"build-script-build"}]}],"#,
            r#""resolve":{"root":null,"nodes":["#,
            r#"{"id":"path+file:///app#app@0.1.0","dependencies":[],"deps":["#,
            r#"{"pkg":"registry+d#dual@1.0.0","dep_kinds":[{"kind":null,"target":null},#,
            r#"{"kind":"build","target":null}]},"#,
            r#"{"pkg":"registry+p#pm@1.0.0","dep_kinds":[{"kind":null,"target":null}]},"#,
            r#"{"pkg":"registry+s#solo@1.0.0","dep_kinds":[{"kind":"build","target":null}]},"#,
            r#"{"pkg":"registry+m#mixed@1.0.0","dep_kinds":[{"kind":null,"target":null},#,
            r#"{"kind":"build","target":null}]},"#,
            r#"{"pkg":"registry+g#ghost@9.9.9","dep_kinds":[{"kind":"dev","target":null}]}]},"#,
            r#"{"id":"registry+d#dual@1.0.0","dependencies":[],"deps":[]},"#,
            r#"{"id":"registry+p#pm@1.0.0","dependencies":[],"deps":["#,
            r#"{"pkg":"registry+i#inner@1.0.0","dep_kinds":[{"kind":null,"target":null}]}]},"#,
            r#"{"id":"registry+i#inner@1.0.0","dependencies":[],"deps":[]},"#,
            r#"{"id":"registry+s#solo@1.0.0","dependencies":[],"deps":[]},"#,
            r#"{"id":"registry+m#mixed@1.0.0","dependencies":[],"deps":[]},"#,
            r#"{"id":"registry+g#ghost@9.9.9","dependencies":[],"deps":[]}],"#,
            r#""workspace_members":[]}"#,
        );
        let set: std::collections::BTreeSet<&str> = [
            "app v0.1.0",
            "dual v1.0.0",
            "pm v1.0.0",
            "inner v1.0.0",
            "solo v1.0.0",
            "mixed v1.0.0",
        ]
        .into_iter()
        .collect();
        // dual + mixed (2 second units) + inner/solo/mixed build scripts (3)
        // + mixed's second build-script run (1) = 6; ghost is dev-only.
        assert_eq!(metadata_units(json, "app", &set), 6);
        // No tree set: nothing matches, however rich the graph.
        let empty: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        assert_eq!(metadata_units(json, "app", &empty), 0);
    }

    #[test]
    fn notify_opt_out_parses_the_documented_values() {
        // On by default; the documented opt-out words (and a stray case) silence
        // it; anything else keeps it on. Split from notify_enabled() so the test
        // never mutates the process env.
        assert!(notify_enabled_from(None), "unset must mean on");
        for off in ["never", "0", "no", "off", "NEVER", "Off"] {
            assert!(
                !notify_enabled_from(Some(off)),
                "'{off}' must silence notifications"
            );
        }
        for on in ["always", "yes", "1", ""] {
            assert!(notify_enabled_from(Some(on)), "'{on}' must stay on");
        }
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn children_are_wired_to_the_log_file_not_the_tty() {
        let repo = "engine-test-stdio";
        let path = log_file(repo);
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "readlink /proc/self/fd/1; readlink /proc/self/fd/2"]);
        attach_log(repo, &mut cmd, "test marker").expect("attach_log");
        let status = cmd.status().expect("spawn");
        assert!(status.success());

        let log = std::fs::read_to_string(&path).expect("log written");
        let want = path.to_string_lossy().to_string();
        assert!(
            log.contains(&want) && log.contains("$ test marker"),
            "child stdio must point at the log, got:\n{log}"
        );
        cleanup(&path);
    }

    #[test]
    fn log_tail_tolerates_invalid_utf8() {
        let repo = "engine-test-utf8";
        let path = log_file(repo);
        let _ = std::fs::create_dir_all(log_dir());
        std::fs::write(&path, b"good line\n\xff\xfe broken\n").expect("write");

        let tail = log_tail(repo, 4096).expect("tail");
        assert!(tail.contains("good line"), "{tail:?}");
        assert!(
            tail.contains('\u{FFFD}'),
            "invalid bytes must be replaced, not dropped: {tail:?}"
        );
        cleanup(&path);
    }

    #[test]
    fn version_probe_gives_up_instead_of_hanging() {
        let dir = std::env::temp_dir().join("artcraft-suite-aur-test");
        let _ = std::fs::create_dir_all(&dir);
        let bin = dir.join("sleeper");
        std::fs::write(&bin, "#!/bin/sh\nsleep 30\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        let started = Instant::now();
        assert!(version_of(&bin).is_none());
        let took = started.elapsed();
        assert!(
            took < VERSION_TIMEOUT + Duration::from_secs(2),
            "probe took {took:?}"
        );
        cleanup(&bin);
    }

    /// A fresh directory tree to discover in, removed by the caller.
    fn tmp_tree(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("artcraft-suite-aur-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmp tree");
        dir
    }

    /// What makes a directory a clone: source (or a git dir) under a suite name.
    fn make_repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).expect("mkdir .git");
        std::fs::write(path.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
    }

    /// A clone whose directory name says nothing — only its remote does.
    fn make_remote_repo(path: &Path, url: &str) {
        std::fs::create_dir_all(path.join(".git")).expect("mkdir .git");
        std::fs::write(
            path.join(".git/config"),
            format!("[remote \"origin\"]\n\turl = {url}\n"),
        )
        .expect("write");
    }

    #[test]
    fn discovery_finds_suite_repos_at_both_depths() {
        let root = tmp_tree("discover");
        make_repo(&root.join("photocraft")); // depth 1: the classic layout
        let deep = root.join("Documents/GitHub/wordcraft"); // depth 3
        make_repo(&deep);
        make_repo(&root.join("sub/photocraft")); // must lose to the depth-1 copy
        make_repo(&root.join("a/b/gridcraft")); // depth 3, no name conflict
        std::fs::create_dir_all(root.join("deckcraft")).expect("mkdir"); // no source: not a clone
        make_repo(&root.join(".cache/lightcraft")); // dot-dirs are never entered

        let found = discover_in(std::slice::from_ref(&root));
        assert_eq!(
            found.get("photocraft").map(PathBuf::as_path),
            Some(root.join("photocraft").as_path()),
            "a depth-1 clone wins over a depth-2 one"
        );
        assert_eq!(
            found.get("wordcraft").map(PathBuf::as_path),
            Some(deep.as_path())
        );
        assert_eq!(
            found.get("gridcraft").map(PathBuf::as_path),
            Some(root.join("a/b/gridcraft").as_path())
        );
        assert!(
            !found.contains_key("deckcraft"),
            "a bare directory named after a suite is not a clone"
        );
        assert!(
            !found.contains_key("lightcraft"),
            "dot-dirs must be skipped"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn discovery_matches_renamed_clones_by_their_git_remote() {
        let root = tmp_tree("remote");
        let https = root.join("docs/photo-upstream");
        make_remote_repo(&https, "https://github.com/someone/photocraft.git");
        let scp = root.join("word-upstream");
        make_remote_repo(&scp, "git@github.com:storytold/wordcraft.git");
        let unrelated = root.join("other");
        make_remote_repo(
            &unrelated,
            "https://github.com/someone/totally-different.git",
        );

        let found = discover_in(std::slice::from_ref(&root));
        assert_eq!(
            found.get("photocraft").map(PathBuf::as_path),
            Some(https.as_path())
        );
        assert_eq!(
            found.get("wordcraft").map(PathBuf::as_path),
            Some(scp.as_path())
        );
        assert_eq!(
            found.len(),
            2,
            "only suite repos are discovered, nothing else"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// Run git in `dir`, asserting success.
    fn git_in(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("spawn git");
        assert!(st.success(), "git {args:?} in {} failed", dir.display());
    }

    /// The bug the whole check phase exists for: a clone whose cached refs
    /// never see a fetch reports "up to date" while the origin has moved on.
    /// `git_fetch_bounded` is what makes `behind()` honest again — against a
    /// local file remote, so the test needs no network.
    #[test]
    fn bounded_fetch_makes_behind_honest_against_a_local_remote() {
        let root = tmp_tree("fetch");
        let origin = root.join("origin");
        let clone = root.join("clone");
        git_in(&root, &["init", "-b", "main", "origin"]);
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        git_in(&origin, &["add", "."]);
        git_in(
            &origin,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-m",
                "first",
            ],
        );
        git_in(&root, &["clone", "origin", "clone"]);

        // the origin gains a commit the clone hasn't seen
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"y\"\n").expect("write");
        git_in(&origin, &["add", "."]);
        git_in(
            &origin,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-m",
                "second",
            ],
        );

        // cached refs are stale: this is exactly the old "up to date" lie
        assert_eq!(behind(&clone), Some(0), "before any fetch, refs agree");
        assert!(upstream_commits(&clone).is_empty());

        git_fetch_bounded(&clone).expect("bounded fetch");

        assert_eq!(
            behind(&clone),
            Some(1),
            "after the fetch, the count is real"
        );
        let commits = upstream_commits(&clone);
        assert!(
            commits.iter().any(|c| c.contains("second")),
            "the pull's contents are listed: {commits:?}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn bounded_fetch_fails_loudly_when_there_is_nothing_to_fetch() {
        let root = tmp_tree("fetchfail");
        assert!(git_fetch_bounded(&root).is_err(), "not a git repo");
        std::fs::remove_dir_all(root).ok();
    }

    /// A commit with an inline identity — the release-tracking fixtures
    /// commit several times and would drown in the repeated flags.
    fn commit_in(dir: &Path, msg: &str) {
        git_in(
            dir,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=test",
                "commit",
                "-m",
                msg,
            ],
        );
    }

    /// Releases mode's core: "behind" counts the release tags `HEAD` does
    /// not contain, newest first — even from a detached tag checkout, where
    /// `@{u}` is unavailable and `origin/HEAD` must carry the answer.
    #[test]
    fn release_tracking_counts_the_tags_head_does_not_have() {
        let root = tmp_tree("releases");
        let origin = root.join("origin");
        let clone = root.join("clone");
        git_in(&root, &["init", "-b", "main", "origin"]);
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        git_in(&origin, &["add", "."]);
        commit_in(&origin, "first");
        git_in(&origin, &["tag", "v0.1.0"]);

        // a second release lands on the origin
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"y\"\n").expect("write");
        git_in(&origin, &["add", "."]);
        commit_in(&origin, "second");
        git_in(&origin, &["tag", "v0.2.0"]);

        git_in(&root, &["clone", "origin", "clone"]);
        // the clone sits on the older release (detached, like `u` leaves it)
        git_in(&clone, &["checkout", "v0.1.0"]);

        let (n, tags) = releases_state(&clone).expect("releases state");
        assert_eq!(n, 1, "one release beyond the checked-out tag");
        assert_eq!(tags, vec!["v0.2.0".to_string()], "newest missing tag first");
        assert_eq!(latest_release(&clone).as_deref(), Some("v0.2.0"));

        // landing on the newest release closes the gap
        git_in(&clone, &["checkout", "v0.2.0"]);
        let (n, tags) = releases_state(&clone).expect("releases state");
        assert_eq!((n, tags.len()), (0, 0), "on the latest release");
        std::fs::remove_dir_all(root).ok();
    }

    /// The vectorcraft regression: a pull moves the clone on, the build is
    /// cancelled, and the row used to say "0.11.0 · up to date" forever.
    /// Recorded build revisions must call the install stale until it is
    /// rebuilt — including from a detached release checkout.
    #[test]
    fn a_pull_without_a_rebuild_leaves_the_install_stale() {
        let repo = "staletest"; // unique cache key: the test owns its .rev
        let root = tmp_tree("stale");
        let clone = root.join("clone");
        git_in(&root, &["init", "-b", "main", "clone"]);
        std::fs::write(clone.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        git_in(&clone, &["add", "."]);
        commit_in(&clone, "first");
        // the install was built from this HEAD
        let built_from = head_rev(&clone).expect("head");
        save_installed_rev(repo, &built_from);
        assert!(
            !install_is_stale(repo, &clone, Some("0.11.0")),
            "an install built from the current HEAD is not stale"
        );
        // the clone moves on (the pull half of `u`), the rebuild never runs
        git_in(&clone, &["tag", "v0.1.0"]);
        std::fs::write(clone.join("Cargo.toml"), "[package]\nname = \"y\"\n").expect("write");
        git_in(&clone, &["add", "."]);
        commit_in(&clone, "second");
        assert!(
            install_is_stale(repo, &clone, Some("0.11.0")),
            "the old binary must be stale against the moved-on clone"
        );
        // a rebuild records the new HEAD: stale clears
        save_installed_rev(repo, &head_rev(&clone).expect("head"));
        assert!(!install_is_stale(repo, &clone, Some("0.12.0")));
        // a detached release checkout is judged by its commit the same way
        git_in(&clone, &["checkout", "v0.1.0"]);
        assert!(
            install_is_stale(repo, &clone, Some("0.12.0")),
            "landing on an older tag also leaves the newer binary stale"
        );
        let _ = std::fs::remove_file(installed_rev_file(repo));
        std::fs::remove_dir_all(root).ok();
    }

    /// Installs that predate recorded revisions fall back to comparing the
    /// binary's --version against the clone's package version — the exact
    /// signal that caught the vectorcraft 0.11.0-vs-0.12.0 lie. Suite apps
    /// keep that version in the root manifest's [workspace.package].
    #[test]
    fn without_a_recorded_revision_the_version_mismatch_names_the_stale_install() {
        let repo = "staleversiontest";
        let _ = std::fs::remove_file(installed_rev_file(repo));
        let root = tmp_tree("stalev");
        let clone = root.join("clone");
        std::fs::create_dir_all(clone.join("apps/photocraft")).expect("mkdir");
        std::fs::write(
            clone.join("apps/photocraft/Cargo.toml"),
            "[package]\nname = \"photocraft\"\nversion.workspace = true\n",
        )
        .expect("write");
        std::fs::write(
            clone.join("Cargo.toml"),
            "[workspace.package]\nversion = \"0.12.0\"\n",
        )
        .expect("write");
        assert!(
            install_is_stale(repo, &clone, Some("photocraft 0.11.0")),
            "an older binary against a newer manifest is stale"
        );
        assert!(
            !install_is_stale(repo, &clone, Some("photocraft 0.12.0")),
            "the probe's 'name version' line matches the manifest's bare version"
        );
        assert!(
            !install_is_stale(repo, &clone, Some("0.12.0 (dev build)")),
            "a suffixed probe line still carries the version as a token"
        );
        assert!(
            !install_is_stale(repo, &clone, None),
            "no version to compare: never claim stale"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// Version sort, not lexicographic: v10 must beat v9, or the "newest"
    /// release is silently the wrong one after the tenth minor.
    #[test]
    fn latest_release_sorts_by_version_not_letters() {
        let root = tmp_tree("relver");
        let origin = root.join("origin");
        let clone = root.join("clone");
        git_in(&root, &["init", "-b", "main", "origin"]);
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        git_in(&origin, &["add", "."]);
        commit_in(&origin, "first");
        git_in(&origin, &["tag", "v9.9.9"]);
        git_in(&origin, &["tag", "v10.0.0"]);
        git_in(&root, &["clone", "origin", "clone"]);
        assert_eq!(latest_release(&clone).as_deref(), Some("v10.0.0"));
        std::fs::remove_dir_all(root).ok();
    }

    /// A clone without tags (`--no-tags`, as a stale shallow clone may be)
    /// gains them from the bounded background fetch — every fetch path passes
    /// `--tags` so releases mode always has its data.
    #[test]
    fn bounded_fetch_brings_tags_to_a_tagless_clone() {
        let root = tmp_tree("fetchtags");
        let origin = root.join("origin");
        let clone = root.join("clone");
        git_in(&root, &["init", "-b", "main", "origin"]);
        std::fs::write(origin.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("write");
        git_in(&origin, &["add", "."]);
        commit_in(&origin, "first");
        git_in(&origin, &["tag", "v0.1.0"]);
        git_in(&root, &["clone", "--no-tags", "origin", "clone"]);

        let before =
            run_capture("git", &["-C", &clone.to_string_lossy(), "tag"]).unwrap_or_default();
        assert!(before.is_empty(), "fixture starts tagless: {before:?}");

        git_fetch_bounded(&clone).expect("bounded fetch");

        let after =
            run_capture("git", &["-C", &clone.to_string_lossy(), "tag"]).unwrap_or_default();
        assert!(
            after.lines().any(|t| t == "v0.1.0"),
            "fetch --tags brought the release: {after:?}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn desktop_entries_are_matched_by_stem_exec_and_icon() {
        let dir = tmp_tree("menu");
        // the launcher's entry: the stem and icon carry the suite name
        std::fs::write(
            dir.join("ai.storyteller.photocraft.desktop"),
            "[Desktop Entry]\nExec=photocraft %F\nIcon=ai.storyteller.photocraft\n",
        )
        .expect("write");
        // this tool's own entry: the stem carries the name
        std::fs::write(
            dir.join("wordcraft.desktop"),
            "[Desktop Entry]\nExec=wordcraft %F\n",
        )
        .expect("write");
        // craft-launcher's binary is artcraft-launcher: Exec decides the package
        std::fs::write(
            dir.join("ai.storyteller.craft-launcher.desktop"),
            "[Desktop Entry]\nExec=artcraft-launcher %F\nIcon=ai.storyteller.craft-launcher\n",
        )
        .expect("write");
        std::fs::write(
            dir.join("filmcraft.desktop"),
            "[Desktop Entry]\nHidden=true\n",
        )
        .expect("write");
        std::fs::write(
            dir.join("firefox.desktop"),
            "[Desktop Entry]\nExec=firefox\n",
        )
        .expect("write");

        let menus = menu_map_in(std::slice::from_ref(&dir));
        assert_eq!(
            menus.get("photocraft").and_then(|m| m.pkg.as_deref()),
            Some("photocraft")
        );
        assert_eq!(
            menus.get("wordcraft").and_then(|m| m.pkg.as_deref()),
            Some("wordcraft")
        );
        assert_eq!(
            menus.get("craft-launcher").and_then(|m| m.pkg.as_deref()),
            Some("artcraft-launcher"),
            "Exec= is the package, not the file name"
        );
        assert!(
            !menus.contains_key("filmcraft"),
            "Hidden=true entries are removed from menus"
        );
        assert!(!menus.contains_key("firefox"), "not a suite entry");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn find_bin_searches_the_local_bin_dir_before_the_system_paths() {
        let root = tmp_tree("findbin");
        let local = root.join("local");
        let system = root.join("system");
        std::fs::create_dir_all(&local).expect("mkdir");
        std::fs::create_dir_all(&system).expect("mkdir");
        std::fs::write(local.join("photocraft"), "local").expect("write");
        std::fs::write(system.join("wordcraft"), "system").expect("write");
        let dirs = [local.clone(), system.clone()];

        assert_eq!(
            find_bin_in(&dirs, "photocraft"),
            Some(local.join("photocraft"))
        );
        assert_eq!(
            find_bin_in(&dirs, "wordcraft"),
            Some(system.join("wordcraft")),
            "a system-wide install (AUR, launcher) still counts as installed"
        );
        assert_eq!(find_bin_in(&dirs, "deckcraft"), None);
        std::fs::remove_dir_all(root).ok();
    }
}

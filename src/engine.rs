//! Suite registry and source-build engine: clone, build, install, update, launch.
//! The same workflow as update-crafts.sh, self-contained so the package has no script dependency.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

/// The Craft App Suite as listed on getartcraft.com/apps, plus the launcher.
pub const SUITE: &[&str] = &[
    "cadcraft",
    "craft-launcher",
    "deckcraft",
    "designcraft",
    "effectcraft",
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
        "cadcraft" => "Fast, open-source computer-aided design and drafting, in pure Rust.",
        "craft-launcher" => "The Craft App Launcher: installs every app and keeps them up to date.",
        "deckcraft" => "Build and deliver dynamic slide presentations, in pure Rust.",
        "designcraft" => "Open-source page layout and publishing, rebuilt in pure Rust.",
        "effectcraft" => "Open-source motion graphics and visual effects, built in pure Rust.",
        "filmcraft" => "Professional, open-source video editing, rebuilt from scratch in Rust.",
        "gridcraft" => "Fast, open-source spreadsheets and data analysis, rebuilt in pure Rust.",
        "lightcraft" => "An open-source photo library and raw developer that runs on your machine.",
        "pdfcraft" => "The open-source PDF workbench: read, organize, combine, split and secure.",
        "photocraft" => "The open-source image editor you already know how to use.",
        "soundcraft" => "Professional audio recording, editing and mixing in pure Rust.",
        "vectorcraft" => "Fast, open-source vector illustration, reimaged in pure Rust.",
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
    /// newest last — what the update actually contains.
    pub commits: Vec<String>,
    /// A `.desktop` entry for this app exists in the user's (or system's)
    /// applications directory — registered by this tool, the launcher or an
    /// AUR package alike.
    pub menu: bool,
}

#[derive(Debug)]
pub enum Ev {
    Done {
        repo: String,
        action: Action,
        result: Result<String, String>,
    },
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
    };
    refresh_local(&mut a, menu);
    a
}

/// Re-read one row after a fetch, a pull, a clone or a build: local facts and
/// git state both, where git is as fresh as the last fetch in this repo.
pub fn rescan_one(a: &mut AppInfo) {
    // the clone's location was decided at scan time; everything else is re-read
    refresh_local(a, menu_map().get(a.repo));
    refresh_git(a);
}

/// Re-read one row's local facts without touching git — for when the fetch
/// failed and stale refs must not masquerade as current state.
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
}

fn refresh_git(a: &mut AppInfo) {
    a.behind = a.cloned.then(|| behind(&a.dir)).flatten();
    a.commits = if a.cloned {
        upstream_commits(&a.dir)
    } else {
        Vec::new()
    };
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
    cmd.args(["-C", &dir.to_string_lossy(), "fetch"])
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

/// Commits `HEAD` is behind its upstream, from the *cached* remote-tracking
/// ref — as fresh as the last fetch in this repo, never fresher. Callers are
/// expected to have fetched first ([`git_fetch_bounded`], `f`, or `u`).
fn behind(dir: &Path) -> Option<u64> {
    run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "rev-list",
            "--count",
            "HEAD..@{u}",
        ],
    )?
    .parse()
    .ok()
}

/// Upstream commits the next pull would bring, from the same cached refs as
/// [`behind`] — no network, so it is as fresh (or as stale) as the count.
fn upstream_commits(dir: &Path) -> Vec<String> {
    let out = run_capture(
        "git",
        &[
            "-C",
            &dir.to_string_lossy(),
            "log",
            "--oneline",
            "-8",
            "HEAD..@{u}",
        ],
    );
    out.filter(|s| !s.is_empty())
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn git_pull(repo: &str, dir: &Path) -> Result<(), String> {
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

fn build_install(dir: &Path, repo: &str) -> Result<String, String> {
    let pkg = pkg_of(repo, dir).ok_or("no desktop app package found under apps/")?;
    let _ = std::fs::create_dir_all(log_dir());
    let log = log_file(repo);
    let file = File::create(&log).map_err(|e| format!("log: {e}"))?;
    let err_file = file.try_clone().map_err(|e| format!("log: {e}"))?;
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--release", "-p", &pkg])
        .current_dir(dir)
        .env("PATH", child_env_path())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file));
    let s = run_registered(repo, &mut cmd)?;
    if s.signal().is_some() {
        return Err("cancelled".into());
    }
    if !s.success() {
        return Err(format!("build failed - see {}", log.display()));
    }
    let src = dir.join("target/release").join(&pkg);
    let dst = bin_dir().join(&pkg);
    std::fs::create_dir_all(bin_dir()).map_err(|e| e.to_string())?;
    std::fs::copy(&src, &dst).map_err(|e| format!("install {dst:?}: {e}"))?;
    let menu = install_menu_entry(dir);
    let v = version_of(&dst).unwrap_or_else(|| "ok".into());
    Ok(if menu {
        format!("installed {pkg} ({v}) · added to app menu")
    } else {
        format!("installed {pkg} ({v})")
    })
}

pub fn run_action(repo: &str, dir: &Path, action: Action) -> Result<String, String> {
    match action {
        Action::Install => {
            if !dir.join("Cargo.toml").is_file() {
                git_clone(repo, dir)?;
            }
            build_install(dir, repo)
        }
        Action::Update => {
            if !dir.join("Cargo.toml").is_file() {
                return Err("not cloned - install it first".into());
            }
            let before = run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"]);
            git_pull(repo, dir)?;
            let after = run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"]);
            let pkg = pkg_of(repo, dir);
            let bin_ok = pkg.as_ref().is_some_and(|p| find_bin(p).is_some());
            if before == after && bin_ok {
                return Ok(format!("{repo}: up to date"));
            }
            build_install(dir, repo)
        }
        Action::Fetch => {
            if !dir.join("Cargo.toml").is_file() {
                return Err("not cloned".into());
            }
            let _guard = git_lock();
            let mut cmd = Command::new("git");
            cmd.args(["-C", &dir.to_string_lossy(), "fetch"])
                .env("PATH", child_env_path());
            attach_log(repo, &mut cmd, "git fetch")?;
            let s = run_registered(repo, &mut cmd)?;
            if s.signal().is_some() {
                return Err("cancelled".into());
            }
            if !s.success() {
                return Err("git fetch failed".into());
            }
            let n = behind(dir);
            Ok(match n {
                Some(n) => format!("{repo}: behind {n} commit(s)"),
                None => format!("{repo}: fetched (no upstream?)"),
            })
        }
    }
}

pub fn spawn(repo: String, dir: PathBuf, action: Action, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let result = run_action(&repo, &dir, action);
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

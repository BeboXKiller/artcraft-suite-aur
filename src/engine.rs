//! Suite registry and source-build engine: clone, build, install, update, launch.
//! The same workflow as update-crafts.sh, self-contained so the package has no script dependency.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

pub const SUITE: &[&str] = &[
    "artcraft",
    "artcraftx",
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
}

pub struct AppInfo {
    pub repo: &'static str,
    pub dir: PathBuf,
    pub pkg: Option<String>,
    pub cloned: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub behind: Option<u64>,
}

#[derive(Debug)]
pub enum Ev {
    Done {
        repo: String,
        action: Action,
        result: Result<String, String>,
    },
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

fn child_env_path() -> String {
    let mut path = std::env::var("PATH").unwrap_or_default();
    let cargo_bin = home().join(".cargo/bin");
    if cargo_bin.is_dir() {
        path = format!("{}:{}", cargo_bin.display(), path);
    }
    path
}

/// Main desktop app package of a repo: the apps/* member that isn't a -cli or -web sibling
/// (craft-launcher hosts artcraft-launcher). Prefers the package named like the repo.
pub fn pkg_of(dir: &Path) -> Option<String> {
    let repo = dir.file_name()?.to_string_lossy().to_string();
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
        let name = text.lines().find_map(|l| l.strip_prefix("name = \"")?.strip_suffix('"'))?;
        if name == repo {
            return Some(name.to_string());
        }
        if best.is_none() {
            best = Some(name.to_string());
        }
    }
    best
}

fn version_of(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").stderr(Stdio::null()).output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_string())
}

pub fn scan() -> Vec<AppInfo> {
    SUITE
        .iter()
        .map(|&repo| {
            let dir = home().join(repo);
            let cloned = dir.join("Cargo.toml").is_file();
            let pkg = cloned.then(|| pkg_of(&dir)).flatten();
            let installed = pkg.as_ref().is_some_and(|p| bin_dir().join(p).is_file());
            let version = installed
                .then(|| bin_dir().join(pkg.as_ref().unwrap()))
                .and_then(|b| version_of(&b));
            AppInfo { repo, dir, pkg, cloned, installed, version, behind: None }
        })
        .collect()
}

pub fn rescan(apps: &mut [AppInfo]) {
    for a in apps {
        let pkg = a.cloned.then(|| pkg_of(&a.dir)).flatten();
        a.installed = pkg.as_ref().is_some_and(|p| bin_dir().join(p).is_file());
        a.version = a
            .installed
            .then(|| bin_dir().join(pkg.as_ref().unwrap()))
            .and_then(|b| version_of(&b));
        a.pkg = pkg;
    }
}

fn run_capture(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd)
        .args(args)
        .env("PATH", child_env_path())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn behind(dir: &Path) -> Option<u64> {
    run_capture("git", &["-C", &dir.to_string_lossy(), "rev-list", "--count", "HEAD..@{u}"])?
        .parse()
        .ok()
}

fn git_pull(dir: &Path) -> Result<(), String> {
    let s = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "pull", "--ff-only", "-q"])
        .env("PATH", child_env_path())
        .status()
        .map_err(|e| format!("git: {e}"))?;
    s.success().then_some(()).ok_or_else(|| "git pull failed (local changes?)".into())
}

fn git_clone(repo: &str, dir: &Path) -> Result<(), String> {
    let s = Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            &format!("https://github.com/storytold/{repo}"),
            &dir.to_string_lossy(),
        ])
        .env("PATH", child_env_path())
        .status()
        .map_err(|e| format!("git: {e}"))?;
    s.success().then_some(()).ok_or_else(|| format!("clone of {repo} failed"))
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

fn install_menu_entry(dir: &Path) {
    let desk = dir.join("packaging/linux");
    if let Ok(rd) = std::fs::read_dir(&desk) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "desktop") {
                let _ = std::fs::create_dir_all(desk_dir());
                if let Some(name) = p.file_name() {
                    let _ = std::fs::copy(&p, desk_dir().join(name));
                }
                break;
            }
        }
    }
    let icons = dir.join("assets/app-icon/hicolor");
    if icons.is_dir() {
        let _ = copy_tree(&icons, &icon_dir());
    }
    let _ = Command::new("update-desktop-database").arg(desk_dir()).stdout(Stdio::null()).status();
    let _ = Command::new("gtk-update-icon-cache")
        .args(["-f", "-t"])
        .arg(icon_dir())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn build_install(dir: &Path, repo: &str) -> Result<String, String> {
    let pkg = pkg_of(dir).ok_or("no desktop app package found under apps/")?;
    let _ = std::fs::create_dir_all(log_dir());
    let log = log_file(repo);
    let file = File::create(&log).map_err(|e| format!("log: {e}"))?;
    let err_file = file.try_clone().map_err(|e| format!("log: {e}"))?;
    let s = Command::new("cargo")
        .args(["build", "--release", "-p", &pkg])
        .current_dir(dir)
        .env("PATH", child_env_path())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(err_file))
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !s.success() {
        return Err(format!("build failed - see {}", log.display()));
    }
    let src = dir.join("target/release").join(&pkg);
    let dst = bin_dir().join(&pkg);
    std::fs::create_dir_all(bin_dir()).map_err(|e| e.to_string())?;
    std::fs::copy(&src, &dst).map_err(|e| format!("install {dst:?}: {e}"))?;
    install_menu_entry(dir);
    let v = version_of(&dst).unwrap_or_else(|| "ok".into());
    Ok(format!("installed {pkg} ({v})"))
}

pub fn run_action(repo: &str, action: Action) -> Result<String, String> {
    let dir = home().join(repo);
    match action {
        Action::Install => {
            if !dir.join("Cargo.toml").is_file() {
                git_clone(repo, &dir)?;
            }
            build_install(&dir, repo)
        }
        Action::Update => {
            if !dir.join("Cargo.toml").is_file() {
                return Err("not cloned - install it first".into());
            }
            let before = run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"]);
            git_pull(&dir)?;
            let after = run_capture("git", &["-C", &dir.to_string_lossy(), "rev-parse", "HEAD"]);
            let pkg = pkg_of(&dir);
            let bin_ok = pkg.as_ref().is_some_and(|p| bin_dir().join(p).is_file());
            if before == after && bin_ok {
                return Ok(format!("{repo}: up to date"));
            }
            build_install(&dir, repo)
        }
        Action::Fetch => {
            if !dir.join("Cargo.toml").is_file() {
                return Err("not cloned".into());
            }
            let s = Command::new("git")
                .args(["-C", &dir.to_string_lossy(), "fetch", "-q"])
                .env("PATH", child_env_path())
                .status()
                .map_err(|e| format!("git: {e}"))?;
            if !s.success() {
                return Err("git fetch failed".into());
            }
            let n = behind(&dir);
            Ok(match n {
                Some(n) => format!("{repo}: behind {n} commit(s)"),
                None => format!("{repo}: fetched (no upstream?)"),
            })
        }
    }
}

pub fn spawn(repo: String, action: Action, tx: Sender<Ev>) {
    std::thread::spawn(move || {
        let result = run_action(&repo, action);
        let _ = tx.send(Ev::Done { repo, action, result });
    });
}

pub fn launch(pkg: &str) -> Result<(), String> {
    let bin = bin_dir().join(pkg);
    if !bin.is_file() {
        return Err(format!("{pkg} is not installed"));
    }
    Command::new(&bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("PATH", child_env_path())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("launch: {e}"))
}

/// Last `want` bytes of a log file for the tail pane.
pub fn log_tail(repo: &str, want: u64) -> Option<String> {
    let mut f = File::open(log_file(repo)).ok()?;
    let len = f.metadata().ok()?.len();
    if len > want {
        f.seek(SeekFrom::End(-(want as i64))).ok()?;
    }
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    Some(buf)
}

pub fn busy_summary(busy: &HashMap<String, String>) -> Option<String> {
    if busy.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = busy.iter().map(|(k, v)| format!("{v} {k}")).collect();
    parts.sort();
    Some(parts.join(" · "))
}

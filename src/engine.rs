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
    /// A full rescan finished.
    Refreshed,
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
            let behind = cloned.then(|| behind(&dir)).flatten();
            AppInfo {
                repo,
                dir,
                pkg,
                cloned,
                installed,
                version,
                behind,
            }
        })
        .collect()
}

pub fn rescan_one(a: &mut AppInfo) {
    a.cloned = a.dir.join("Cargo.toml").is_file();
    let pkg = a.cloned.then(|| pkg_of(&a.dir)).flatten();
    a.installed = pkg.as_ref().is_some_and(|p| bin_dir().join(p).is_file());
    a.version = a
        .installed
        .then(|| bin_dir().join(pkg.as_ref().unwrap()))
        .and_then(|b| version_of(&b));
    a.pkg = pkg;
    a.behind = a.cloned.then(|| behind(&a.dir)).flatten();
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

fn git_pull(repo: &str, dir: &Path) -> Result<(), String> {
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

fn build_install(dir: &Path, repo: &str) -> Result<String, String> {
    let pkg = pkg_of(dir).ok_or("no desktop app package found under apps/")?;
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
            git_pull(repo, &dir)?;
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
        let _ = tx.send(Ev::Done {
            repo,
            action,
            result,
        });
    });
}

pub fn launch(pkg: &str) -> Result<(), String> {
    let bin = bin_dir().join(pkg);
    if !bin.is_file() {
        return Err(format!("{pkg} is not installed"));
    }
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
}

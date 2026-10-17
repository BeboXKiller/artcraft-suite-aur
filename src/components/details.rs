//! The bottom details pane: the live log while a task runs, the update's
//! contents otherwise — what the next `u` would actually bring.
//!
//! Step 6 of the refactor: `draw_details`/`details_for` and their helpers
//! (`progress_line`, `tail_lines`, `short_dir`) plus the cached log tail
//! (`refresh_log`) move here from `ui.rs`/`app.rs`. `App` keeps a thin
//! `refresh_log` wrapper that picks the selected repo.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::coarse_eta;
use crate::components::list::{behind_word, clip};
use crate::engine::{self, AppInfo, Track};
use crate::task::TaskManager;
use crate::theme::{Sty, Tint};

/// Detail lines kept visible in the bottom pane (the block is `Length(7)`).
pub const DETAIL_LINES: usize = 5;
/// Bytes of build log cached for the tail pane.
const LOG_BYTES: u64 = 4096;

/// Cached tail of the selected repo's build log, re-read only when the file
/// or the selection changed.
#[derive(Default)]
pub struct DetailsComponent {
    pub log: Vec<String>,
    log_repo: &'static str,
    log_len: u64,
}

impl DetailsComponent {
    /// Cache the log tail for the selected repo.
    ///
    /// Called once per loop: a `stat` per tick, re-read only when the file or
    /// the selection changed, so rendering stays a pure function of state.
    pub fn refresh_log(&mut self, repo: &'static str) {
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

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        s: Sty,
        sel: Option<&AppInfo>,
        tasks: &TaskManager,
        track: Track,
    ) {
        let width = area.width.saturating_sub(4) as usize;
        let Some(a) = sel else {
            frame.render_widget(
                Paragraph::new(" nothing to show — no apps scanned ")
                    .block(Block::bordered().title(" Details ")),
                area,
            );
            return;
        };
        let (title, lines) = self.details_for(a, s, width, tasks, track);
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(title)),
            area,
        );
    }

    fn details_for<'a>(
        &self,
        a: &AppInfo,
        s: Sty,
        width: usize,
        tasks: &TaskManager,
        track: Track,
    ) -> (String, Vec<Line<'a>>) {
        let repo = a.repo;
        if let Some(verb) = tasks.busy.get(repo) {
            let title = format!(" {verb} — {repo} ");
            let mut lines = Vec::new();
            if let Some(p) = tasks.progress.get(repo) {
                lines.push(progress_line(p, s, width));
            }
            lines.extend(tail_lines(
                &self.log,
                s.fg(Tint::Muted),
                width,
                "(waiting for output…)",
            ));
            return (title, lines);
        }
        // waiting in the queue: say so before the normal content — the row
        // already reads ◌ queued, the pane explains what happens next
        if let Some(pos) = tasks.pending.iter().position(|(r, _)| r == repo) {
            let action = tasks.pending[pos].1;
            let title = format!(" ◌ {repo} — queued ");
            let lines = vec![
                Line::from(Span::styled(
                    format!(" {} starts next", action.label()),
                    s.fg(Tint::Accent),
                )),
                Line::from(Span::styled(
                    format!(
                        " position {} of {} · x cancels",
                        pos + 1,
                        tasks.pending.len()
                    ),
                    s.fg(Tint::Faint),
                )),
            ];
            return (title, lines);
        }
        if let Some(err) = tasks.failed.get(repo) {
            let title = format!(" ✗ {repo} — build failed ");
            let mut lines = vec![Line::from(Span::styled(format!(" {err}"), s.fg(Tint::Bad)))];
            lines.extend(tail_lines(
                &self.log,
                s.fg(Tint::Muted),
                width,
                "(no log kept)",
            ));
            lines.push(Line::from(Span::styled(
                " x clears · r rescans · i retries",
                s.fg(Tint::Faint),
            )));
            return (title, lines);
        }
        let (title, mut lines) = match a.behind {
            Some(n) if n > 0 => {
                let title = format!(" What's new in {repo} ");
                let version = a.version.as_deref().unwrap_or("no version");
                let mut lines = vec![Line::from(Span::styled(
                    format!(" {version} · {n} {} to pull", behind_word(track, n)),
                    s.fg(Tint::Warn).bold(),
                ))];
                if a.commits.is_empty() {
                    lines.push(Line::from(Span::styled(
                        match track {
                            Track::Commits => " (commit list unknown — f refreshes)",
                            Track::Releases => " (release list unknown — f refreshes)",
                        },
                        s.fg(Tint::Faint),
                    )));
                }
                for c in a.commits.iter().take(DETAIL_LINES) {
                    lines.push(Line::from(Span::styled(
                        format!(" {}", clip(c, width.saturating_sub(2))),
                        s.fg(Tint::Muted),
                    )));
                }
                let more = a.commits.len().saturating_sub(DETAIL_LINES);
                let hint = if more > 0 {
                    format!("+{more} more · f refreshes · u updates")
                } else {
                    "f refreshes · u updates".to_string()
                };
                lines.push(Line::from(Span::styled(
                    format!(" {hint}"),
                    s.fg(Tint::Faint),
                )));
                (title, lines)
            }
            // the clone is current but the binary predates it: a pull whose
            // build never finished. There is nothing to pull — the update is
            // already on disk, waiting to be rebuilt.
            _ if a.installed && a.cloned && a.stale => {
                let version = a.version.as_deref().unwrap_or("no version");
                (
                    format!(" {repo} — update ready "),
                    vec![
                        Line::from(Span::styled(
                            format!(" {version} · source is newer than the install"),
                            s.fg(Tint::Warn),
                        )),
                        Line::from(Span::styled(
                            " u rebuilds it · Enter launches the installed one",
                            s.fg(Tint::Faint),
                        )),
                    ],
                )
            }
            _ if a.installed && !a.cloned => {
                let version = a.version.as_deref().unwrap_or("no version");
                (
                    format!(" {repo} — installed, no clone "),
                    vec![
                        Line::from(Span::styled(
                            format!(" {version} · binary found, source not here"),
                            s.fg(Tint::Good),
                        )),
                        Line::from(Span::styled(
                            " Enter launches · i clones and builds it",
                            s.fg(Tint::Faint),
                        )),
                    ],
                )
            }
            _ if a.installed && a.behind.is_none() => {
                let version = a.version.as_deref().unwrap_or("no version");
                (
                    format!(" {repo} — not checked "),
                    vec![
                        Line::from(Span::styled(
                            format!(" {version} · upstream not checked"),
                            s.fg(Tint::Muted),
                        )),
                        Line::from(Span::styled(
                            " f fetches · u updates · Enter launches",
                            s.fg(Tint::Faint),
                        )),
                    ],
                )
            }
            _ if a.installed => {
                let version = a.version.as_deref().unwrap_or("no version");
                (
                    format!(" {repo} — up to date "),
                    vec![
                        Line::from(Span::styled(
                            format!(" {version} · nothing to pull"),
                            s.fg(Tint::Good),
                        )),
                        Line::from(Span::styled(
                            " Enter launches · u updates · i reinstalls",
                            s.fg(Tint::Faint),
                        )),
                    ],
                )
            }
            _ if a.cloned => (
                format!(" {repo} — cloned, not built "),
                vec![Line::from(Span::styled(
                    " i builds and installs it",
                    s.fg(Tint::Faint),
                ))],
            ),
            _ => (
                format!(" {repo} — not installed "),
                vec![Line::from(Span::styled(
                    " i clones and builds it",
                    s.fg(Tint::Faint),
                ))],
            ),
        };
        // Footer: where the clone lives (auto-discovered paths differ from
        // `~/<repo>`) and whether a desktop entry is registered — the two facts a
        // bare row can't show.
        let menu = if a.menu {
            "in app menu"
        } else {
            "no menu entry"
        };
        let room = width.saturating_sub(menu.len() + 4);
        lines.push(Line::from(Span::styled(
            format!(" {} · {}", clip(&short_dir(&a.dir), room), menu),
            s.fg(Tint::Faint),
        )));
        (title, lines)
    }
}

/// The clone path with `$HOME` shortened to `~`, for the details footer.
fn short_dir(dir: &std::path::Path) -> String {
    let home = crate::engine::home_dir();
    if let Ok(rest) = dir.strip_prefix(&home)
        && !rest.as_os_str().is_empty()
    {
        return format!("~/{}", rest.display());
    }
    dir.display().to_string()
}

/// The selected build's progress bar and its numbers, above the live log.
fn progress_line<'a>(p: &engine::Progress, s: Sty, width: usize) -> Line<'a> {
    match (p.pct(), p.eta_secs()) {
        (Some(pct), eta) => {
            // Keep bar + numbers inside the pane even when it's narrow.
            let eta_str = eta.map_or_else(|| "…".into(), coarse_eta);
            let text = format!("{pct}% · {eta_str} left · {} compiled", p.done);
            let bar_w = width.saturating_sub(text.chars().count() + 3).max(4);
            let filled = bar_w * pct as usize / 100;
            let bar: String = "█".repeat(filled) + &"░".repeat(bar_w - filled);
            Line::from(vec![
                Span::styled(format!(" {bar} "), s.fg(Tint::Accent)),
                Span::styled(text, s.fg(Tint::Muted)),
            ])
        }
        // No denominator (the pre-build crate count couldn't be obtained):
        // show elapsed time rather than a bar that would be a guess.
        (None, _) => Line::from(Span::styled(
            format!(" building… {} elapsed", coarse_eta(p.elapsed_secs())),
            s.fg(Tint::Muted),
        )),
    }
}

fn tail_lines<'a>(
    log: &[String],
    style: Style,
    width: usize,
    empty: &'static str,
) -> Vec<Line<'a>> {
    if log.is_empty() {
        return vec![Line::from(Span::styled(format!(" {empty}"), style))];
    }
    let start = log.len().saturating_sub(DETAIL_LINES);
    log[start..]
        .iter()
        .map(|l| {
            Line::from(Span::styled(
                format!(" {}", clip(l, width.saturating_sub(2))),
                style,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        App::with_apps(vec![AppInfo {
            repo: "photocraft",
            dir: std::path::PathBuf::from("/nonexistent"),
            pkg: Some("photocraft".into()),
            cloned: true,
            installed: true,
            // deliberately wider than the version column on a 60-column screen
            version: Some("0.5.0 (dev build) with a very long suffix".into()),
            behind: Some(0),
            commits: Vec::new(),
            menu: true,
            stale: false,
        }])
    }

    fn app_info(repo: &str, cloned: bool, installed: bool, behind: Option<u64>) -> AppInfo {
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

    fn render(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| {
                        buf.cell((x, y))
                            .unwrap()
                            .symbol()
                            .chars()
                            .next()
                            .unwrap_or(' ')
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn details_pane_lists_release_tags() {
        let mut a = app();
        a.track = Track::Releases;
        let mut b = app_info("wordcraft", true, true, Some(2));
        b.commits = vec!["v0.14.0".into(), "v0.15.0".into()];
        a.list.apps.push(b);
        a.list.sel = 1;
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("2 releases to pull"), "{joined:?}");
        assert!(joined.contains("v0.15.0"), "{joined:?}");
    }

    #[test]
    fn details_pane_shows_what_the_update_contains() {
        let mut a = app();
        let mut b = app_info("wordcraft", true, true, Some(2));
        b.commits = vec![
            "abc1234 fix table borders".into(),
            "def5678 faster previews".into(),
        ];
        a.list.apps.push(b);
        a.list.sel = 1;
        let rows = render(&a, 80, 24);
        let joined = rows.join("\n");
        assert!(joined.contains("What's new in wordcraft"), "{joined:?}");
        assert!(joined.contains("fix table borders"), "{joined:?}");
        assert!(joined.contains("2 commits to pull"), "{joined:?}");
    }

    #[test]
    fn details_pane_shows_the_live_log_while_busy() {
        let mut a = app();
        a.tasks
            .busy
            .insert("photocraft".into(), "installing".into());
        a.details.log = vec!["   Compiling photocraft v0.5.0".into()];
        let rows = render(&a, 80, 24);
        let joined = rows.join("\n");
        assert!(joined.contains("installing — photocraft"), "{joined:?}");
        assert!(joined.contains("Compiling photocraft"), "{joined:?}");
    }

    #[test]
    fn a_queued_task_says_so_in_the_details_pane() {
        let mut a = app();
        // behind as well: without the queued branch the pane would show the
        // update card, so the negative assert proves the interception
        a.list.apps[0].behind = Some(3);
        a.tasks
            .pending
            .push_back(("photocraft".into(), engine::Action::Update));
        a.tasks
            .pending
            .push_back(("other".into(), engine::Action::Install));
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("photocraft — queued"), "{joined:?}");
        assert!(joined.contains("updating starts next"), "{joined:?}");
        assert!(joined.contains("position 1 of 2"), "{joined:?}");
        assert!(joined.contains("x cancels"), "{joined:?}");
        assert!(
            !joined.contains("What's new in"),
            "queued pane fell through to content: {joined:?}"
        );
    }

    #[test]
    fn details_footer_reports_where_the_clone_lives_and_the_menu_state() {
        // the fixture is menu-registered but lives outside $HOME
        let rows = render(&app(), 100, 30);
        let joined = rows.join("\n");
        assert!(joined.contains("in app menu"), "{joined:?}");
        assert!(
            joined.contains("/nonexistent"),
            "a discovered clone path must be visible: {joined:?}"
        );

        let a = App::with_apps(vec![app_info("cadcraft", false, false, None)]);
        let rows = render(&a, 100, 30);
        assert!(
            rows.iter().any(|r| r.contains("no menu entry")),
            "a missing menu entry must be stated: {rows:?}"
        );
    }

    #[test]
    fn a_building_row_shows_a_bar_and_a_tilde_eta() {
        // A cold build at 43 of 80 expected crates, two minutes ago: honest
        // numbers, and the ETA is prefixed with `~` so it never reads as a
        // countdown.
        let mut a = app();
        a.tasks.busy.insert("photocraft".into(), "building".into());
        a.tasks.progress.insert(
            "photocraft".into(),
            engine::Progress {
                done: 43,
                seen: 43,
                total: 80,
                started: std::time::Instant::now() - std::time::Duration::from_secs(130),
                // 37 unresolved × 2s per crate → ~1m left.
                pace_secs: Some(2.0),
                last_real: Some(std::time::Instant::now()),
            },
        );
        let rows = render(&a, 80, 24);
        let joined = rows.join("\n");
        assert!(joined.contains("building — photocraft"), "{joined:?}");
        assert!(joined.contains("█"), "no filled bar cell: {joined:?}");
        assert!(joined.contains("53%"), "wrong percentage: {joined:?}");
        assert!(
            joined.contains("43 compiled"),
            "crate count missing: {joined:?}"
        );
        assert!(
            joined.contains('~'),
            "an ETA must read as an estimate: {joined:?}"
        );
    }

    #[test]
    fn a_build_starts_at_zero_with_an_empty_bar_not_a_full_one() {
        // The reported bug: the bar was pinned at 100% from the first crate
        // because the denominator was set equal to the count. A seeded build
        // must start at 0% with an empty bar and fill in from there.
        let mut a = app();
        a.tasks.busy.insert("photocraft".into(), "building".into());
        a.tasks.progress.insert(
            "photocraft".into(),
            engine::Progress {
                done: 0,
                seen: 0,
                total: 440,
                started: std::time::Instant::now(),
                pace_secs: None,
                last_real: None,
            },
        );
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("0%"), "not at zero: {joined:?}");
        assert!(
            joined.contains("0 compiled"),
            "crate count missing: {joined:?}"
        );
        assert!(
            joined.contains('░') && !joined.contains('█'),
            "a zero-percent bar must be empty: {joined:?}"
        );
    }

    #[test]
    fn a_build_with_no_denominator_shows_elapsed_not_a_fake_percent() {
        // The denominator couldn't be obtained: there is no honest percentage,
        // so show elapsed time rather than a bar that would be a guess.
        let mut a = app();
        a.tasks.busy.insert("photocraft".into(), "building".into());
        a.tasks.progress.insert(
            "photocraft".into(),
            engine::Progress {
                done: 0,
                seen: 0,
                total: 0,
                started: std::time::Instant::now() - std::time::Duration::from_secs(12),
                pace_secs: None,
                last_real: None,
            },
        );
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("elapsed"), "{joined:?}");
        assert!(
            !joined.contains('%'),
            "no percentage without a total: {joined:?}"
        );
    }

    #[test]
    fn short_dir_shortens_home_only() {
        let home = engine::home_dir();
        let inside = home.join("src").join("cadcraft");
        assert_eq!(short_dir(&inside), "~/src/cadcraft");
        // not under $HOME: show the path as-is, no `~` invented
        assert_eq!(
            short_dir(std::path::Path::new("/nonexistent/photocraft")),
            "/nonexistent/photocraft"
        );
    }
}

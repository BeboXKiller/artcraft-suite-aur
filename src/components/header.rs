//! The top header block: title, the status line, and the right-aligned
//! suite counters.
//!
//! Step 7 of the refactor: the header text assembly and the `summary`
//! counters move here from `ui.rs`. `App` keeps `status_line` (it owns the
//! message and task state); this component only paints what it is given.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::Tone;
use crate::engine::{AppInfo, Track};
use crate::task::TaskManager;
use crate::theme::{Sty, Tint};

pub struct HeaderComponent;

/// Everything the header row needs, gathered at the call site so this
/// component stays free of `App`.
pub struct HeaderView<'a> {
    pub status: &'a str,
    pub tone: Tone,
    pub apps: &'a [AppInfo],
    pub filter_attention: bool,
    pub track: Track,
    pub tasks: &'a TaskManager,
}

impl HeaderComponent {
    pub fn render(&self, frame: &mut Frame, area: Rect, s: Sty, view: HeaderView<'_>) {
        let tone_tint = match view.tone {
            Tone::Busy => Tint::Accent,
            Tone::Good => Tint::Good,
            Tone::Bad => Tint::Bad,
            Tone::Info => Tint::Muted,
        };
        let head_text = vec![
            Line::from(vec![
                Span::styled(" ArtCraft Suite", s.fg(Tint::Accent).bold()),
                Span::styled(
                    " — install · update · launch the Craft suite",
                    s.fg(Tint::Muted),
                ),
            ]),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(view.status.to_string(), s.fg(tone_tint)),
            ]),
        ];
        let mut head_block = Block::bordered().title(" ArtCraft Suite ");
        let summary = summary(
            view.apps,
            view.filter_attention,
            view.track,
            &view.tasks.busy,
            view.tasks.max_parallel,
            s,
        );
        if !summary.is_empty() {
            head_block = head_block.title(Line::from(summary).right_aligned());
        }
        frame.render_widget(Paragraph::new(head_text).block(head_block), area);
    }
}

/// Right-aligned header counters: how much is current, behind, or missing.
fn summary(
    apps: &[AppInfo],
    filter_attention: bool,
    track: Track,
    busy: &std::collections::HashMap<String, String>,
    max_parallel: usize,
    s: Sty,
) -> Vec<Span<'static>> {
    if apps.is_empty() {
        return Vec::new();
    }
    let behind = apps
        .iter()
        .filter(|a| a.behind.is_some_and(|n| n > 0))
        .count();
    // nothing present at all: no clone *and* no installed binary — an app
    // that is installed without its source is not "missing"
    let missing = apps.iter().filter(|a| !a.cloned && !a.installed).count();
    let up = apps
        .iter()
        .filter(|a| a.installed && !a.stale && (a.behind == Some(0) || !a.cloned))
        .count();
    // clones whose upstream check hasn't landed — unknown, not "current"
    let unchecked = apps
        .iter()
        .filter(|a| a.cloned && a.behind.is_none())
        .count();
    let mut spans = vec![
        Span::styled(format!(" ✓ {up} "), s.fg(Tint::Good)),
        Span::styled(format!(" ↑ {behind} "), s.fg(Tint::Warn)),
        Span::styled(format!(" · {missing} "), s.fg(Tint::Faint)),
    ];
    if unchecked > 0 {
        spans.push(Span::styled(format!(" ? {unchecked} "), s.fg(Tint::Muted)));
    }
    if filter_attention {
        spans.push(Span::styled(" attention ", s.fg(Tint::Accent).bold()));
    }
    if track == Track::Releases {
        spans.push(Span::styled(" ⚑ releases ", s.fg(Tint::Accent)));
    }
    if !busy.is_empty() {
        spans.push(Span::styled(
            format!(" ⚙ {}/{} ", busy.len(), max_parallel),
            s.fg(Tint::Accent),
        ));
    }
    spans
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
    fn header_summarises_the_suite() {
        let mut a = app();
        a.list.apps.push(app_info("wordcraft", true, true, Some(3)));
        a.list.apps.push(app_info("cadcraft", false, false, None));
        let rows = render(&a, 80, 24);
        let head = rows[0].clone();
        assert!(head.contains("✓ 1"), "up-to-date count missing: {head:?}");
        assert!(head.contains("↑ 1"), "behind count missing: {head:?}");
        assert!(head.contains("· 1"), "missing count missing: {head:?}");
    }

    #[test]
    fn releases_badge_shows_in_the_header_only_in_releases_mode() {
        let mut a = app();
        a.list.apps.push(app_info("wordcraft", true, true, Some(0)));
        let head = render(&a, 80, 24)[0].clone();
        assert!(!head.contains("releases"), "no badge expected: {head:?}");
        a.track = Track::Releases;
        let head = render(&a, 80, 24)[0].clone();
        assert!(head.contains("⚑ releases"), "badge missing: {head:?}");
    }

    #[test]
    fn header_counts_running_builds_against_the_limit() {
        let mut a = app();
        a.tasks
            .busy
            .insert("photocraft".into(), "installing".into());
        a.tasks.max_parallel = 2;
        let head = render(&a, 80, 24).join("\n");
        assert!(head.contains("⚙ 1/2"), "no parallel badge: {head:?}");
        let b = app();
        assert!(
            !render(&b, 80, 24).join("\n").contains('⚙'),
            "idle badge leaked"
        );
    }
}

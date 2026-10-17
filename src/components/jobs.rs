//! The `p` parallel-builds popup: scratch value, key handling, centered card.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::{MAX_JOBS, MIN_JOBS};
use crate::components::{Component, ComponentEvent};
use crate::theme::{Sty, Tint};

/// Editor for the parallel-build limit. Independent from the other overlays:
/// help can sit on top of it and both stay open until a key resolves them.
#[derive(Default)]
pub struct JobsPopupComponent {
    pub open: bool,
    /// Value being edited, seeded from the live limit when `p` opens it.
    pub scratch: usize,
}

impl JobsPopupComponent {
    /// `p`: open the editor seeded with the live limit.
    pub fn open_with(&mut self, limit: usize) {
        self.open = true;
        self.scratch = limit;
    }

    /// The limit the editor is sitting on, clamped like the apply path.
    pub fn limit(&self) -> usize {
        self.scratch.clamp(MIN_JOBS, MAX_JOBS)
    }

    /// Centered editor card: the number, the honest warning for weak
    /// machines, and how to change or leave. Moved verbatim from
    /// `ui::draw_jobs_popup`.
    pub fn render(&self, frame: &mut Frame, area: Rect, s: Sty) {
        if !self.open {
            return;
        }
        let lines: Vec<Line> = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  {}", self.scratch),
                s.fg(Tint::Accent).bold(),
            )),
            Line::from(""),
            Line::from("  apps compiling at once."),
            Line::from(Span::styled(
                "  Low-spec PC? Stick to 1–2.",
                s.fg(Tint::Warn),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  ↑↓ or +/- change · Enter applies · Esc cancels",
                s.fg(Tint::Faint),
            )),
        ];
        let w = area.width.min(50);
        let box_h = (lines.len() as u16 + 2).min(area.height);
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(w)) / 2,
            area.y + (area.height.saturating_sub(box_h)) / 2,
            w,
            box_h,
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(" Parallel builds ")),
            rect,
        );
    }
}

impl Component for JobsPopupComponent {
    /// Not destructive, so unknown keys are simply ignored. `Enter` closes
    /// and asks App to adopt the limit via [`ComponentEvent::JobsApply`].
    fn handle_key(&mut self, key: KeyEvent) -> ComponentEvent {
        match key.code {
            KeyCode::Up | KeyCode::Char('+') => {
                self.scratch = (self.scratch + 1).min(MAX_JOBS);
                ComponentEvent::Ignored
            }
            KeyCode::Down | KeyCode::Char('-') => {
                self.scratch = self.scratch.saturating_sub(1).max(MIN_JOBS);
                ComponentEvent::Ignored
            }
            KeyCode::Enter => {
                self.open = false;
                ComponentEvent::JobsApply
            }
            KeyCode::Esc => {
                self.open = false;
                ComponentEvent::Msg("parallel builds unchanged".into())
            }
            _ => ComponentEvent::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, DEFAULT_MAX_PARALLEL};
    use crate::engine::{Action, AppInfo};
    use crossterm::event::KeyModifiers;

    fn apps(n: usize) -> Vec<AppInfo> {
        (0..n)
            .map(|i| AppInfo {
                repo: ["a", "b", "c"][i],
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Fill every runner slot so no key press can spawn real work.
    fn saturate(app: &mut App) {
        for i in 0..app.tasks.max_parallel {
            app.tasks.busy.insert(format!("z{i}"), "installing".into());
        }
        assert_eq!(app.tasks.busy.len(), app.tasks.max_parallel);
    }

    #[test]
    fn p_opens_the_jobs_popup_and_enter_applies() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Char('p')));
        assert!(app.jobs.open, "p must open the popup");
        assert_eq!(
            app.jobs.scratch, app.tasks.max_parallel,
            "scratch starts at the current limit"
        );
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Char('+')));
        assert_eq!(app.jobs.scratch, app.tasks.max_parallel + 2);
        app.on_key(key(KeyCode::Enter));
        assert!(!app.jobs.open, "Enter must close the popup");
        assert_eq!(app.tasks.max_parallel, 4);
        assert_eq!(app.msg, "parallel builds: 4");
    }

    #[test]
    fn raising_the_limit_promotes_queued_work_immediately() {
        let mut app = App::with_apps(apps(3));
        saturate(&mut app);
        app.tasks.pending.push_back(("c".into(), Action::Install));
        app.on_key(key(KeyCode::Char('p')));
        app.on_key(key(KeyCode::Up)); // 3
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.tasks.busy.len(),
            3,
            "the queued task must start at once"
        );
        assert!(app.tasks.pending.is_empty(), "the queue must be drained");
    }

    #[test]
    fn the_jobs_popup_clamps_and_esc_cancels() {
        let mut app = App::with_apps(apps(1));
        app.on_key(key(KeyCode::Char('p')));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.jobs.scratch, MIN_JOBS, "clamped at the floor");
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.jobs.scratch, MIN_JOBS);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.list.sel, 0,
            "other keys must not act while the popup is open"
        );
        app.on_key(key(KeyCode::Esc));
        assert!(!app.jobs.open);
        assert_eq!(
            app.tasks.max_parallel, DEFAULT_MAX_PARALLEL,
            "Esc keeps the old limit"
        );
        assert_eq!(app.msg, "parallel builds unchanged");
        app.on_key(key(KeyCode::Char('p')));
        for _ in 0..20 {
            app.on_key(key(KeyCode::Up));
        }
        assert_eq!(app.jobs.scratch, MAX_JOBS, "clamped at the ceiling");
    }

    #[test]
    fn jobs_popup_shows_the_value_and_the_low_spec_warning() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut a = App::with_apps(vec![AppInfo {
            repo: "photocraft",
            dir: std::path::PathBuf::from("/nonexistent"),
            pkg: Some("photocraft".into()),
            cloned: true,
            installed: true,
            version: None,
            behind: Some(0),
            commits: Vec::new(),
            menu: true,
            stale: false,
        }]);
        a.jobs.open = true;
        a.jobs.scratch = 5;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| a.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let joined: String = (0..24)
            .map(|y| {
                (0..80)
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
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Parallel builds"), "{joined:?}");
        assert!(joined.contains("apps compiling at once."), "{joined:?}");
        assert!(joined.contains("Low-spec PC? Stick to 1–2."), "{joined:?}");
        assert!(joined.contains("Enter applies · Esc cancels"), "{joined:?}");
        // the card is small: the key bar stays readable around it
        assert!(
            joined.contains("[q] quit"),
            "popup ate the frame: {joined:?}"
        );
        // and it never shows without the p key
        let b = App::with_apps(vec![AppInfo {
            repo: "photocraft",
            dir: std::path::PathBuf::from("/nonexistent"),
            pkg: None,
            cloned: false,
            installed: false,
            version: None,
            behind: None,
            commits: Vec::new(),
            menu: false,
            stale: false,
        }]);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| b.render(f)).unwrap();
        let buf = term.backend().buffer().clone();
        let joined: String = (0..24)
            .map(|y| {
                (0..80)
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
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !joined.contains("Parallel builds"),
            "popup leaked into a normal frame"
        );
    }
}

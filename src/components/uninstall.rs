//! The `d`-key uninstall confirmation: pending targets and the centered card.
//!
//! Independent from the other overlays — today the help can sit on top of a
//! pending confirm, and both stay armed until a key resolves the confirm.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::components::{Component, ComponentEvent};
use crate::engine::AppInfo;
use crate::theme::{Sty, Tint};

/// Pending `d` uninstall: target indices awaiting a `y` confirmation.
#[derive(Default)]
pub struct UninstallConfirmComponent {
    pub targets: Option<Vec<usize>>,
}

impl UninstallConfirmComponent {
    pub fn open(&mut self, targets: Vec<usize>) {
        self.targets = Some(targets);
    }

    pub fn pending(&self) -> bool {
        self.targets.is_some()
    }

    /// Centered confirmation card: the names, what goes, how to say yes.
    /// Takes `apps` as an argument (AppContext stays minimal) — the names
    /// come from the list's rows. Moved verbatim from `ui::draw_confirm_uninstall`.
    pub fn render(&self, frame: &mut Frame, area: Rect, s: Sty, apps: &[AppInfo]) {
        let Some(targets) = &self.targets else {
            return;
        };
        let mut lines: Vec<Line> = Vec::new();
        let names: Vec<&str> = targets
            .iter()
            .filter_map(|i| apps.get(*i).map(|a| a.repo))
            .collect();
        for n in names.iter().take(5) {
            lines.push(Line::from(Span::styled(
                format!("  {n}"),
                s.fg(Tint::Accent).bold(),
            )));
        }
        if names.len() > 5 {
            lines.push(Line::from(Span::styled(
                format!("  +{} more", names.len() - 5),
                s.fg(Tint::Faint),
            )));
        }
        lines.push(Line::from(""));
        for l in [
            "  This removes the binary, the",
            "  desktop entry, the icons and",
            "  the clone.",
        ] {
            lines.push(Line::from(Span::styled(l, s.fg(Tint::Warn))));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  y confirms · any other key cancels",
            s.fg(Tint::Faint),
        )));
        let w = area.width.min(40);
        let box_h = (lines.len() as u16 + 2).min(area.height);
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(w)) / 2,
            area.y + (area.height.saturating_sub(box_h)) / 2,
            w,
            box_h,
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().title(" Confirm uninstall ")),
            rect,
        );
    }
}

impl Component for UninstallConfirmComponent {
    /// Any key except `y` changes the mind; the `y` itself falls through to
    /// App's normal-mode match, which runs the uninstall. (`?` and Ctrl were
    /// already handled by App before this is reached.)
    fn handle_key(&mut self, key: KeyEvent) -> ComponentEvent {
        if self.targets.is_some() && key.code != KeyCode::Char('y') {
            self.targets = None;
            ComponentEvent::Msg("uninstall cancelled".into())
        } else {
            ComponentEvent::Ignored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
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

    fn render(app: &App, w: u16, h: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::draw(f, app)).unwrap();
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
    fn d_asks_before_uninstall_and_any_other_key_cancels() {
        let mut v = apps(3);
        v[0].installed = true;
        let mut app = App::with_apps(v);
        app.list.sel = 0;
        app.on_key(key(KeyCode::Char('d')));
        assert!(app.uninstall.pending(), "d must ask first");
        assert!(app.msg.contains('y'), "prompt must say y: {}", app.msg);
        app.on_key(key(KeyCode::Char('j')));
        assert!(!app.uninstall.pending(), "another key must cancel");
        assert_eq!(app.msg, "uninstall cancelled");
        // asking again and confirming: no clone on disk, so the engine errors
        // safely without touching anything real
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));
        assert!(!app.uninstall.pending(), "y must consume the confirm");
        assert!(
            app.msg.contains("no clone"),
            "engine error must surface: {}",
            app.msg
        );
        assert!(app.failed.contains_key("a"), "failed row must be marked");
    }

    #[test]
    fn d_skips_busy_and_not_installed_rows() {
        let mut v = apps(3);
        v[0].installed = true;
        let mut app = App::with_apps(v);
        app.busy.insert("a".into(), "updating".into());
        app.list.sel = 0;
        app.on_key(key(KeyCode::Char('d')));
        assert!(!app.uninstall.pending(), "nothing eligible must not ask");
        assert_eq!(app.msg, "nothing to uninstall (1 skipped)");
        assert_eq!(app.busy["a"], "updating", "busy row must stay untouched");
    }

    #[test]
    fn confirm_uninstall_is_a_centered_card_not_a_full_screen() {
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
        a.uninstall.open(vec![0]);
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("Confirm uninstall"), "{joined:?}");
        assert!(joined.contains("photocraft"), "{joined:?}");
        assert!(
            joined.contains("y confirms · any other key cancels"),
            "{joined:?}"
        );
        assert!(joined.contains("the clone."), "{joined:?}");
        // the card is small: the key bar stays readable around it
        assert!(
            joined.contains("[q] quit"),
            "modal ate the frame: {joined:?}"
        );
        // and it never shows without a pending confirm
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
        assert!(
            !render(&b, 80, 24).join("\n").contains("Confirm uninstall"),
            "modal leaked into a normal frame"
        );
    }
}

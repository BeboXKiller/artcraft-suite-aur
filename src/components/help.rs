//! The `?` help overlay: open/scroll state, key handling, and the card itself.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::components::{Component, ComponentEvent};
use crate::theme::{Sty, Tint};

/// Help lines per page — same jump the list's PageUp/PageDown uses.
const PAGE: usize = 10;

/// State of the `?` overlay. Independent from the other overlays: today the
/// help can be open over a pending uninstall confirm, and both stay armed.
#[derive(Default)]
pub struct HelpOverlayComponent {
    pub open: bool,
    /// Scroll offset inside the help (it is longer than a small screen).
    pub scroll: usize,
}

impl HelpOverlayComponent {
    /// `?`: open at the top, or close. The scroll never survives a reopen.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.scroll = 0;
        }
    }
}

impl Component for HelpOverlayComponent {
    /// The `?` key itself never reaches here — `App::on_key` toggles first.
    /// `End`/`G` are deliberately not mapped (quirk preserved from ui.rs).
    fn handle_key(&mut self, key: KeyEvent) -> ComponentEvent {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                self.open = false;
                ComponentEvent::CloseOverlay
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ComponentEvent::Ignored
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                ComponentEvent::Ignored
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(PAGE);
                ComponentEvent::Ignored
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(PAGE);
                ComponentEvent::Ignored
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.scroll = 0;
                ComponentEvent::Ignored
            }
            _ => ComponentEvent::Ignored,
        }
    }
}

/// Full-width modal help card. Kept within 58 columns so nothing wraps on the
/// 60x15 minimum; longer than a small screen, so j/k scroll it. (Moved verbatim
/// from `ui::draw_help`.)
pub fn render_help_card(frame: &mut Frame, area: Rect, s: Sty, scroll: usize) {
    let h = s.fg(Tint::Accent).bold();
    // Every hint with a one-line description of what it actually does.
    let lines: Vec<Line> = vec![
        Line::from(Span::styled("Navigation", h)),
        Line::from("  j/k or ↑↓       move the highlight"),
        Line::from("  PgUp / PgDn     jump ten rows"),
        Line::from("  g / G, Home/End first / last app"),
        Line::from("  → / l            open a description under the app"),
        Line::from("  ← / h            close it again"),
        Line::from("  o               attention only (hide apps that are fine)"),
        Line::from("  t               track releases — rows and u follow tags"),
        Line::from(""),
        Line::from(Span::styled(
            "Picking — what u / i / f / d / x / Enter act on",
            h,
        )),
        Line::from("  Space           pick the row; press again to unpick"),
        Line::from("  Ctrl+Space      add or remove a row, keep the rest"),
        Line::from("  (no picks)      actions apply to the highlighted row"),
        Line::from(""),
        Line::from(Span::styled("Acting on the picks", h)),
        Line::from("  Enter           open the app; install it if missing"),
        Line::from("                   many picks: install every missing one"),
        Line::from("  u / i / f       update / install / fetch the picks"),
        Line::from("  U / a           update everything / install everything"),
        Line::from("  d               uninstall: binary, menu, icons, clone"),
        Line::from("                   y confirms, any other key cancels"),
        Line::from("  x or Delete     cancel a task, or unqueue a queued one"),
        Line::from("  r               rescan: clone locations, menu, git state"),
        Line::from(""),
        Line::from(Span::styled("Session", h)),
        Line::from("  ?               open or close this help"),
        Line::from("  p               how many builds run at once (1-16)"),
        Line::from("  q / Esc         quit — asks again while tasks run"),
        Line::from("  Ctrl+C          quit immediately"),
        Line::from(""),
        Line::from(Span::styled("Reading a row", h)),
        Line::from("  ✓ up to date    ↑n behind   ◐ updating…   ◌ queued"),
        Line::from("  ✗ build failed  ○ cloned, not built   · missing"),
        Line::from("  ? not checked   ↻ update ready (source newer)"),
        Line::from("                   f fetches · u updates"),
        Line::from("                   colour tier auto · NO_COLOR=1 = mono"),
    ];
    // full-width modal so nothing bleeds around it; capped to the screen so it
    // also fits the 60x15 minimum
    let w = area.width;
    let box_h = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(
        area.x,
        area.y + (area.height.saturating_sub(box_h)) / 2,
        w,
        box_h,
    );
    let inner_h = box_h.saturating_sub(2) as usize;
    let offset = scroll.min(lines.len().saturating_sub(inner_h));
    let visible: Vec<Line> = lines.into_iter().skip(offset).collect();
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(visible)
            .block(Block::bordered().title(" Help — j/k scroll · ? or Esc to close "))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::engine::AppInfo;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
    fn help_overlay_swallows_keys_until_closed() {
        let mut app = App::with_apps(apps(3));
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.help.open);
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.list.sel, 0,
            "navigation must not pass through the overlay"
        );
        app.on_key(key(KeyCode::Esc));
        assert!(!app.help.open);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.list.sel, 1);
    }

    #[test]
    fn help_scrolls_with_j_and_k_without_moving_selection() {
        let mut app = App::with_apps(apps(3));
        app.list.sel = 2;
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.help.scroll, 0, "opening starts at the top");
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.help.scroll, 2);
        assert_eq!(
            app.list.sel, 2,
            "the app selection must not move under the overlay"
        );
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.help.scroll, 1);
        app.on_key(key(KeyCode::Home));
        assert_eq!(app.help.scroll, 0);
        app.on_key(key(KeyCode::Esc));
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.help.scroll, 0, "reopening starts at the top again");
    }

    #[test]
    fn help_overlay_renders_on_top() {
        let mut app = App::with_apps(vec![AppInfo {
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
        }]);
        app.help.open = true;
        let joined = render(&app, 100, 30).join("\n");
        assert!(joined.contains("Esc to close"));
        assert!(joined.contains("Navigation"));
        assert!(joined.contains("attention only"));
        assert!(
            joined.contains("pick the row; press again to unpick"),
            "every hint needs a description: {joined:?}"
        );
        assert!(
            joined.contains("uninstall: binary, menu, icons, clone"),
            "uninstall hint missing from help: {joined:?}"
        );
        assert!(
            joined.contains("how many builds run at once"),
            "parallel-builds hint missing from help: {joined:?}"
        );
    }

    #[test]
    fn help_scrolls_to_later_sections() {
        let mut app = App::with_apps(vec![AppInfo {
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
        app.help.open = true;
        app.help.scroll = 20;
        let joined = render(&app, 80, 24).join("\n");
        assert!(joined.contains("Reading a row"), "{joined:?}");
        assert!(
            !joined.contains("Navigation"),
            "scroll offset ignored: {joined:?}"
        );
    }

    #[test]
    fn help_fits_the_minimum_terminal() {
        let mut app = App::with_apps(vec![AppInfo {
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
        app.help.open = true;
        // first screen: nothing wraps at 60 columns (every line fits 58)
        let top = render(&app, 60, 15).join("\n");
        assert!(top.contains("Navigation"), "{top:?}");
        assert!(
            top.contains("attention only (hide apps that are fine)"),
            "{top:?}"
        );
        // scrolled to the end: the legend is reachable too, and the offset clamps
        app.help.scroll = 99;
        let bottom = render(&app, 60, 15).join("\n");
        assert!(bottom.contains("NO_COLOR=1"), "{bottom:?}");
        assert!(
            !bottom.contains("Navigation"),
            "scroll clamp not applied: {bottom:?}"
        );
    }
}

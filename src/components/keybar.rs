//! The three-line keyboard legend under the details pane.
//!
//! Width-aware: the full legend renders when every row fits with a 1-cell
//! margin (76 cols and up), a condensed set of essential hints otherwise —
//! so nothing clips even at the 60x15 minimum. Two tiers, each internally
//! uniform: dense rows pair tightly (one space), sparse rows breathe (two).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::{Sty, Tint};

/// One `[key] label` hint.
struct Hint {
    key: &'static str,
    label: &'static str,
}

const fn hint(key: &'static str, label: &'static str) -> Hint {
    Hint { key, label }
}

/// Full legend — picking, navigation, batch actions.
const PICK: &[Hint] = &[
    hint("[Space]", "pick"),
    hint("[Ctrl+Space]", "pick more"),
    hint("[Enter]", "open"),
    hint("[u]", "update"),
    hint("[o]", "installed"),
];
const NAV: &[Hint] = &[
    hint("[j/k]", "move"),
    hint("[→]", "details"),
    hint("[←]", "close"),
    hint("[f]", "fetch"),
    hint("[x]", "cancel"),
    hint("[?]", "help"),
    hint("[q]", "quit"),
];
const BULK: &[Hint] = &[
    hint("[U]", "update all"),
    hint("[a]", "install all"),
    hint("[d]", "remove"),
    hint("[p]", "jobs"),
    hint("[r]", "rescan"),
    hint("[t]", "releases"),
];

/// Condensed legend for 60–75 cols: the hints a first-time user needs most;
/// everything else stays one `?` away.
const PICK_NARROW: &[Hint] = &[
    hint("[Space]", "pick"),
    hint("[Enter]", "open"),
    hint("[u]", "update"),
    hint("[o]", "installed"),
];
const NAV_NARROW: &[Hint] = &[
    hint("[j/k]", "move"),
    hint("[→]", "details"),
    hint("[←]", "close"),
    hint("[?]", "help"),
    hint("[q]", "quit"),
];
const BULK_NARROW: &[Hint] = &[
    hint("[U]", "update all"),
    hint("[a]", "install all"),
    hint("[d]", "remove"),
    hint("[r]", "rescan"),
];

const FULL_GAP: &str = " ";
const NARROW_GAP: &str = "  ";

pub struct KeyBarComponent;

impl KeyBarComponent {
    pub fn render(&self, frame: &mut Frame, area: Rect, s: Sty) {
        frame.render_widget(
            Paragraph::new(self.rows(s, area.width)).alignment(Alignment::Center),
            area,
        );
    }

    /// The legend rows that fit `width` with a 1-cell margin on each side.
    pub fn rows(&self, s: Sty, width: u16) -> Vec<Line<'static>> {
        let full = [
            row(s, PICK, FULL_GAP),
            row(s, NAV, FULL_GAP),
            row(s, BULK, FULL_GAP),
        ];
        if full.iter().all(|l| l.width() as u16 + 2 <= width) {
            return full.to_vec();
        }
        vec![
            row(s, PICK_NARROW, NARROW_GAP),
            row(s, NAV_NARROW, NARROW_GAP),
            row(s, BULK_NARROW, NARROW_GAP),
        ]
    }
}

fn row(s: Sty, hints: &[Hint], gap: &'static str) -> Line<'static> {
    let c = s.fg(Tint::Accent);
    let mut spans = Vec::new();
    for (i, h) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(gap));
        }
        spans.push(Span::styled(h.key, c));
        spans.push(Span::raw(format!(" {}", h.label)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::engine::AppInfo;
    use crate::theme::Palette;
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

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn key_bar_fits_80_columns() {
        let rows = render(&app(), 80, 24);
        let row1 = &rows[rows.len() - 3];
        let row2 = &rows[rows.len() - 2];
        let row3 = &rows[rows.len() - 1];
        assert!(row1.contains("[Space] pick"), "pick hint missing: {row1:?}");
        assert!(
            row1.contains("[Ctrl+Space] pick more"),
            "multi-pick hint missing: {row1:?}"
        );
        assert!(
            row1.contains("[o] installed"),
            "filter hint missing: {row1:?}"
        );
        assert!(
            row1.trim_end().ends_with("installed"),
            "row 1 was clipped at 80 columns: {row1:?}"
        );
        assert!(
            row2.contains("[→] details"),
            "expand hint missing: {row2:?}"
        );
        assert!(
            row2.contains("[←] close"),
            "collapse hint missing: {row2:?}"
        );
        assert!(row2.contains("[q] quit"), "quit hint missing: {row2:?}");
        assert!(row2.trim_end().ends_with("quit"), "row 2 clipped: {row2:?}");
        assert!(
            row3.contains("[U] update all"),
            "batch hint missing: {row3:?}"
        );
        assert!(
            row3.contains("[a] install all"),
            "install hint missing: {row3:?}"
        );
        assert!(
            row3.contains("[d] remove"),
            "uninstall hint missing: {row3:?}"
        );
        assert!(
            row3.contains("[p] jobs"),
            "parallel-builds hint missing: {row3:?}"
        );
        assert!(
            row3.trim_end().ends_with("releases"),
            "row 3 was clipped at 80 columns: {row3:?}"
        );
    }

    #[test]
    fn every_row_keeps_a_cell_of_margin_at_every_width() {
        let s = Sty::new(Palette::mono());
        for w in 60..=120u16 {
            for line in KeyBarComponent.rows(s, w) {
                let width = line.width();
                assert!(
                    width + 2 <= w as usize,
                    "row of {width} cells clips at {w} cols: {:?}",
                    text(&line)
                );
            }
        }
    }

    #[test]
    fn narrow_terminals_swap_in_the_condensed_legend() {
        let s = Sty::new(Palette::mono());

        let full: String = KeyBarComponent
            .rows(s, 80)
            .iter()
            .map(|l| text(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(full.contains("[Ctrl+Space] pick more"), "full: {full:?}");

        let narrow: String = KeyBarComponent
            .rows(s, 60)
            .iter()
            .map(|l| text(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !narrow.contains("[Ctrl+Space]"),
            "the condensed tier must drop the low-priority hint: {narrow:?}"
        );
        for want in [
            "[Space] pick",
            "[Enter] open",
            "[u] update",
            "[o] installed",
            "[j/k] move",
            "[q] quit",
            "[U] update all",
            "[a] install all",
        ] {
            assert!(narrow.contains(want), "missing {want:?} in {narrow:?}");
        }
    }
}

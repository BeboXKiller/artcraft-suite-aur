//! The three-line keyboard legend under the details pane.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme::{Sty, Tint};

pub struct KeyBarComponent;

impl KeyBarComponent {
    pub fn render(&self, frame: &mut Frame, area: Rect, s: Sty) {
        frame.render_widget(
            Paragraph::new(self.lines(s)).alignment(Alignment::Center),
            area,
        );
    }

    pub fn lines(&self, s: Sty) -> Vec<Line<'static>> {
        let c = s.fg(Tint::Accent);
        vec![
            Line::from(vec![
                Span::styled("[Space]", c),
                Span::raw(" pick  "),
                Span::styled("[Ctrl+Space]", c),
                Span::raw(" pick more  "),
                Span::styled("[Enter]", c),
                Span::raw(" open  "),
                Span::styled("[u]", c),
                Span::raw(" update  "),
                Span::styled("[o]", c),
                Span::raw(" attention"),
            ]),
            Line::from(vec![
                Span::styled("[j/k]", c),
                Span::raw(" move  "),
                Span::styled("[→]", c),
                Span::raw(" details  "),
                Span::styled("[←]", c),
                Span::raw(" close  "),
                Span::styled("[f]", c),
                Span::raw(" fetch  "),
                Span::styled("[x]", c),
                Span::raw(" cancel  "),
                Span::styled("[?]", c),
                Span::raw(" help  "),
                Span::styled("[q]", c),
                Span::raw(" quit"),
            ]),
            Line::from(vec![
                Span::styled("[U]", c),
                Span::raw(" update all  "),
                Span::styled("[a]", c),
                Span::raw(" install all  "),
                Span::styled("[d]", c),
                Span::raw(" remove  "),
                Span::styled("[p]", c),
                Span::raw(" jobs  "),
                Span::styled("[r]", c),
                Span::raw(" rescan  "),
                Span::styled("[t]", c),
                Span::raw(" releases"),
            ]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::engine::AppInfo;
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
            row1.contains("[o] attention"),
            "filter hint missing: {row1:?}"
        );
        assert!(
            row1.trim_end().ends_with("attention"),
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
}

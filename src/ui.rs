//! Layout and widgets: header, suite list, log tail, key bar, help overlay.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;

/// Smallest grid this layout is designed for; below it we show a polite message.
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 15;
/// Log lines kept visible in the tail pane (the block is `Length(7)`).
const LOG_LINES: usize = 5;

/// Color policy for a frame.
///
/// Honors `NO_COLOR` by dropping fg/bg colors while keeping bold/reverse, which
/// every terminal renders — so the UI never depends on color alone anyway.
#[derive(Clone, Copy)]
struct Sty {
    color: bool,
}

impl Sty {
    fn fg(self, color: Color) -> Style {
        if self.color {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    fn bold(self) -> Style {
        Style::default().bold()
    }

    fn hl(self) -> Style {
        if self.color {
            Style::default().bg(Color::DarkGray).bold()
        } else {
            Style::default().reversed().bold()
        }
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let msg = format!(
            "Terminal too small ({}x{}).\n\nNeeds at least {MIN_WIDTH}x{MIN_HEIGHT}.",
            area.width, area.height
        );
        frame.render_widget(
            Paragraph::new(msg)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let [head, main, tail, keys_row] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(6),
        Constraint::Length(7),
        Constraint::Length(2),
    ])
    .areas(area);

    let s = Sty {
        color: !app.no_color,
    };
    let (status, is_busy) = app.status_line();
    let head_text = vec![
        Line::from(vec![
            Span::styled(" artcraft-suite-aur", s.fg(Color::Cyan).bold()),
            Span::raw(" — install · update · launch the Craft suite from source"),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                status,
                s.fg(if is_busy { Color::Yellow } else { Color::Gray }),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(head_text).block(Block::bordered().title(" ArtCraft Suite ")),
        head,
    );

    draw_list(frame, app, main);
    draw_log(frame, app, tail);
    frame.render_widget(
        Paragraph::new(key_bar(!app.no_color)).alignment(Alignment::Center),
        keys_row,
    );

    if app.show_help {
        draw_help(frame, area, !app.no_color);
    }
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let s = Sty {
        color: !app.no_color,
    };
    // borders (2) + highlight symbol (2)
    let inner_w = area.width.saturating_sub(4) as usize;
    // mark(3) + dot(3) + repo(16) + pkg(22) + version(14) = 58, plus the
    // behind badge (6) and the state column (10)
    let show_state = inner_w >= 74;

    let items: Vec<ListItem> = app
        .apps
        .iter()
        .enumerate()
        .map(|(idx, a)| {
            let (dot, dot_color) = if app.busy.contains_key(a.repo) {
                ("◐", Color::Yellow)
            } else if app.pending.iter().any(|(r, _)| r == a.repo) {
                ("◌", Color::LightBlue)
            } else if a.installed {
                ("●", Color::Green)
            } else if a.cloned {
                ("○", Color::DarkGray)
            } else {
                ("·", Color::DarkGray)
            };
            let (mark, mark_style) = if app.picks.contains(&idx) {
                ("☑", s.bold())
            } else {
                ("☐", s.fg(Color::DarkGray))
            };
            let version = a.version.as_deref().unwrap_or("-");
            let behind = match a.behind {
                Some(n) if n > 0 => format!("  ↑{n}"),
                _ => String::new(),
            };
            let pkg = a.pkg.as_deref().unwrap_or("-");
            let state = match (a.installed, a.cloned) {
                (true, _) => " installed",
                (_, true) => " cloned",
                _ => "",
            };
            let state = if show_state { state } else { "" };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {mark} "), mark_style),
                Span::styled(format!(" {dot} "), s.fg(dot_color)),
                Span::styled(pad(a.repo, 16), s.bold()),
                Span::styled(pad(pkg, 22), s.fg(Color::Gray)),
                Span::styled(pad(version, 14), Style::default()),
                Span::styled(behind, s.fg(Color::Magenta)),
                Span::styled(state, s.fg(Color::Gray)),
            ]))
        })
        .collect();

    let mut title = format!(" Apps ({})", app.apps.len());
    if !app.picks.is_empty() {
        title.push_str(&format!(" · {} picked", app.picks.len()));
    }
    if area.width >= 76 {
        title.push_str("  ● installed  ○ cloned  · missing  ◐ busy  ◌ queued");
    }
    title.push(' ');
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(s.hl())
        .highlight_symbol("▶ ");
    let mut state = ListState::default();
    state.select(Some(app.sel));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let s = Sty {
        color: !app.no_color,
    };
    let sel_repo = app.apps.get(app.sel).map(|a| a.repo).unwrap_or("");
    let lines: Vec<Line> = if app.log.is_empty() {
        vec![Line::from("(no build log for the selected app)")]
    } else {
        let start = app.log.len().saturating_sub(LOG_LINES);
        app.log[start..]
            .iter()
            .map(|l| Line::from(Span::styled(l.clone(), s.fg(Color::Gray))))
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(format!(" Log — {sel_repo} "))),
        area,
    );
}

fn key_bar(color: bool) -> Vec<Line<'static>> {
    let c = Sty { color }.fg(Color::Cyan);
    vec![
        Line::from(vec![
            Span::styled("[j/k]", c),
            Span::raw("  "),
            Span::styled("[Space]", c),
            Span::raw(" pick  "),
            Span::styled("[PgUp/PgDn]", c),
            Span::raw(" page  "),
            Span::styled("[Enter]", c),
            Span::raw(" open  "),
            Span::styled("[u]", c),
            Span::raw(" update  "),
            Span::styled("[U]", c),
            Span::raw(" update all"),
        ]),
        Line::from(vec![
            Span::styled("[a]", c),
            Span::raw(" install all  "),
            Span::styled("[f]", c),
            Span::raw(" fetch  "),
            Span::styled("[x]", c),
            Span::raw(" cancel  "),
            Span::styled("[r]", c),
            Span::raw(" rescan  "),
            Span::styled("[?]", c),
            Span::raw(" help  "),
            Span::styled("[q]", c),
            Span::raw(" quit"),
        ]),
    ]
}

fn draw_help(frame: &mut Frame, area: Rect, color: bool) {
    let s = Sty { color };
    // 13 lines + 2 border rows: exactly the 60x15 minimum
    let lines = vec![
        Line::from(Span::styled("Navigation", s.fg(Color::Cyan).bold())),
        Line::from("  j/k or ↑↓ move  g/G, Home/End first/last  PgUp/PgDn ±10"),
        Line::from("  Enter open/install  Space pick  Ctrl+Space toggle picks"),
        Line::from(""),
        Line::from(Span::styled("Actions", s.fg(Color::Cyan).bold())),
        Line::from("  u update    U update all    i install    a install all"),
        Line::from("  f fetch     r rescan all    x cancel     ? this help"),
        Line::from(""),
        Line::from(Span::styled("Session", s.fg(Color::Cyan).bold())),
        Line::from("  q / Esc quit (asks again while tasks run)   Ctrl+C quit"),
        Line::from(Span::styled("Dots", s.fg(Color::Cyan).bold())),
        Line::from("  ● installed  ○ cloned  · missing  ◐ running  ◌ queued"),
        Line::from("  ↑n   commits you are behind upstream"),
    ];
    // full-width modal so nothing bleeds around it; capped to the screen so it
    // also fits the 60x15 minimum
    let w = area.width;
    let h = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect::new(area.x, area.y + (area.height.saturating_sub(h)) / 2, w, h);
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Help — ? or Esc to close "))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

/// Pad to `w`, truncating with an ellipsis so a long field can never push the
/// columns after it out of the pane.
fn pad(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        format!("{:1$}", s, w)
    } else {
        let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            // deliberately wider than the 14-cell version column
            version: Some("0.5.0 (dev build) with a very long suffix".into()),
            behind: Some(0),
        }])
    }

    fn render(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
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
    fn status_line_is_visible() {
        // regression: the header used to be 3 rows with 2 text lines, so this row
        // was clipped and every status/error message vanished
        for (w, h) in [(120, 40), (80, 24), (60, 15)] {
            let rows = render(&app(), w, h);
            assert!(
                rows.iter().take(4).any(|r| r.contains("ready")),
                "status row missing at {w}x{h}: {rows:?}"
            );
        }
    }

    #[test]
    fn key_bar_fits_80_columns() {
        let rows = render(&app(), 80, 24);
        let row1 = &rows[rows.len() - 2];
        let row2 = &rows[rows.len() - 1];
        assert!(row2.contains("[q]"), "quit hint missing: {row2:?}");
        assert!(row2.contains("[?]"), "help hint missing: {row2:?}");
        assert!(row1.contains("[Space] pick"), "pick hint missing: {row1:?}");
        assert!(
            row1.trim_end().ends_with("update all"),
            "row 1 was clipped at 80 columns: {row1:?}"
        );
    }

    #[test]
    fn picked_rows_show_a_checkbox_and_the_title_counts_them() {
        let mut a = app();
        a.apps.push(AppInfo {
            repo: "videocraft",
            dir: std::path::PathBuf::from("/nonexistent"),
            pkg: None,
            cloned: false,
            installed: false,
            version: None,
            behind: None,
        });
        a.picks.insert(1);
        let rows = render(&a, 80, 24);
        assert!(
            rows.iter().any(|r| r.contains("· 1 picked")),
            "picked count missing from the title: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains('☑')),
            "picked row has no checkmark: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains('☐')),
            "unpicked row has no empty box: {rows:?}"
        );
    }

    #[test]
    fn wide_fields_do_not_shift_the_state_column() {
        let rows = render(&app(), 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("photocraft"))
            .expect("row present");
        assert!(row.contains('…'), "version must be truncated, got: {row:?}");
        assert!(
            row.contains("installed"),
            "state column must stay aligned: {row:?}"
        );
        assert!(row.chars().count() <= 80, "row wider than the screen");
    }

    #[test]
    fn tiny_terminal_gets_a_polite_message() {
        let rows = render(&app(), 40, 12);
        let joined = rows.join("\n");
        assert!(
            joined.contains("too small"),
            "no min-size message: {joined:?}"
        );
    }

    #[test]
    fn help_overlay_renders_on_top() {
        let mut a = app();
        a.show_help = true;
        let joined = render(&a, 100, 30).join("\n");
        assert!(joined.contains("Esc to close"));
        assert!(joined.contains("Navigation"));
    }

    #[test]
    fn pad_truncates_and_pads() {
        assert_eq!(pad("abc", 6), "abc   ");
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(pad("abc", 3), "abc");
    }

    #[test]
    fn help_fits_the_minimum_terminal() {
        let mut a = app();
        a.show_help = true;
        let joined = render(&a, 60, 15).join("\n");
        // last content line must not be clipped at the documented minimum
        assert!(
            joined.contains("commits you are behind upstream"),
            "{joined:?}"
        );
    }

    #[test]
    fn no_color_mode_emits_no_colored_cells() {
        let mut a = app();
        a.no_color = true;
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &a)).unwrap();
        let buf = term.backend().buffer();
        for y in 0..24u16 {
            for x in 0..80u16 {
                let cell = buf.cell((x, y)).unwrap();
                assert_eq!(cell.fg, Color::Reset, "fg color at {x},{y}");
                assert_eq!(cell.bg, Color::Reset, "bg color at {x},{y}");
            }
        }
    }
}

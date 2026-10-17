//! Layout and widgets: the frame layout shell.
//! The list, details, header, key bar, and overlays live in
//! `crate::components`; this module keeps the frame layout until step 8.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::widgets::{Paragraph, Wrap};

use crate::app::App;
use crate::theme::Sty;

/// Smallest grid this layout is designed for; below it we show a polite message.
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 15;

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
        Constraint::Length(3),
    ])
    .areas(area);

    let s = Sty::new(app.palette);
    let (status, tone) = app.status_line();
    app.header.render(
        frame,
        head,
        s,
        crate::components::header::HeaderView {
            status: &status,
            tone,
            apps: &app.list.apps,
            filter_attention: app.list.filter_attention,
            track: app.track,
            tasks: &app.tasks,
        },
    );

    app.list.render(
        frame,
        main,
        s,
        crate::components::list::TaskView {
            busy: &app.tasks.busy,
            pending: &app.tasks.pending,
            failed: &app.tasks.failed,
            track: app.track,
        },
    );
    app.details.render(
        frame,
        tail,
        s,
        app.list.apps.get(app.list.sel),
        &app.tasks,
        app.track,
    );
    app.keybar.render(frame, keys_row, s);

    if app.help.open {
        crate::components::help::render_help_card(frame, area, s, app.help.scroll);
    }
    app.uninstall.render(frame, area, s, &app.list.apps);
    app.jobs.render(frame, area, s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::engine::{AppInfo, Track};
    use crate::theme::{Depth, Palette};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

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
    fn up_to_date_rows_say_so_in_words() {
        let rows = render(&app(), 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("photocraft"))
            .expect("row present");
        assert!(row.contains('✓'), "no up-to-date glyph: {row:?}");
        assert!(row.contains("up to date"), "no up-to-date phrase: {row:?}");
    }

    #[test]
    fn unchecked_rows_admit_it_and_the_header_counts_them() {
        let mut a = app(); // photocraft: checked, up to date
        a.list.apps.push(app_info("wordcraft", true, true, None));
        let rows = render(&a, 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present");
        assert!(row.contains('?'), "no unchecked glyph: {row:?}");
        assert!(row.contains("not checked"), "no unchecked phrase: {row:?}");
        assert!(
            !row.contains("up to date"),
            "stale refs must never read as currency: {row:?}"
        );
        let head = &rows[0];
        assert!(
            head.contains("✓ 1"),
            "checked row still counts up: {head:?}"
        );
        assert!(head.contains("? 1"), "unchecked count missing: {head:?}");

        // the details pane admits it too, and points at the way to fix it
        a.list.sel = 1;
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("wordcraft — not checked"), "{joined:?}");
        assert!(joined.contains("upstream not checked"), "{joined:?}");
        assert!(joined.contains("f fetches · u updates"), "{joined:?}");
    }

    #[test]
    fn the_attention_filter_keeps_unchecked_clones() {
        let mut a = app(); // photocraft: checked, up to date
        a.list.apps.push(app_info("wordcraft", true, true, None));
        assert!(
            a.list.needs_attention(&a.list.apps[1], &Default::default()),
            "an unchecked clone still needs checking"
        );
        a.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('o'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(a.visible_rows(), vec![1], "only the unchecked clone stays");
    }

    /// The vectorcraft regression: the clone sat on 0.12.0, the binary was
    /// 0.11.0, and the row said "up to date". A stale install must announce
    /// itself, leave the up-to-date counter, and stay in the attention view.
    #[test]
    fn a_stale_install_says_update_ready_not_up_to_date() {
        let mut a = app(); // photocraft: behind 0, built from the same HEAD
        a.list.apps.push(app_info("wordcraft", true, true, Some(0)));
        a.list.apps[1].stale = true;
        a.list.apps[1].version = Some("0.11.0".into());
        let rows = render(&a, 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present");
        assert!(row.contains('↻'), "no stale glyph: {row:?}");
        assert!(row.contains("update ready"), "no stale phrase: {row:?}");
        assert!(
            !row.contains("up to date"),
            "a stale install must never read as current: {row:?}"
        );
        let head = &rows[0];
        assert!(
            head.contains("✓ 1"),
            "only the fresh photocraft counts up: {head:?}"
        );
        // the details pane explains it and points at the fix
        a.list.sel = 1;
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("wordcraft — update ready"), "{joined:?}");
        assert!(
            joined.contains("source is newer than the install"),
            "{joined:?}"
        );
        assert!(joined.contains("u rebuilds it"), "{joined:?}");
        // and the attention filter keeps it
        a.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('o'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(a.visible_rows(), vec![1], "the stale install stays");
    }

    #[test]
    fn behind_rows_say_so_in_words() {
        let mut a = app();
        a.list.apps.push(app_info("wordcraft", true, true, Some(3)));
        let rows = render(&a, 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present");
        assert!(row.contains("↑3"), "no behind glyph: {row:?}");
        assert!(
            row.contains("3 commits behind"),
            "no behind phrase: {row:?}"
        );
    }

    #[test]
    fn releases_mode_counts_releases_in_words() {
        let mut a = app();
        a.track = Track::Releases;
        a.list.apps.push(app_info("wordcraft", true, true, Some(2)));
        let rows = render(&a, 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present");
        assert!(row.contains("↑2"), "no behind glyph: {row:?}");
        assert!(
            row.contains("2 releases behind"),
            "no releases phrase: {row:?}"
        );

        let mut b = app();
        b.track = Track::Releases;
        b.list.apps.push(app_info("gridcraft", true, true, Some(1)));
        let rows = render(&b, 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("gridcraft"))
            .expect("row present");
        assert!(
            row.contains("1 release behind"),
            "singular releases phrase missing: {row:?}"
        );
    }

    #[test]
    fn commits_mode_keeps_saying_commits() {
        // regression: the same row under the default track must not flip
        let mut a = app();
        a.track = Track::Commits;
        a.list.apps.push(app_info("wordcraft", true, true, Some(2)));
        let row = render(&a, 80, 24)
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present")
            .clone();
        assert!(row.contains("2 commits behind"), "{row:?}");
        assert!(!row.contains("release"), "{row:?}");
    }

    #[test]
    fn a_three_digit_behind_count_keeps_the_phrase_and_glyph_intact() {
        // At 3 digits "100 commits behind" is 18 chars and the glyph is
        // "↑99+" (4 chars). Both used to overflow their columns and clip the
        // tail, so the phrase lost its "behind" and the glyph read "↑9…".
        let mut a = app();
        a.list
            .apps
            .push(app_info("wordcraft", true, true, Some(1234)));
        let row = render(&a, 80, 24)
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present")
            .clone();
        assert!(row.contains("↑99+"), "behind glyph truncated: {row:?}");
        assert!(
            row.contains("1234 commits behind"),
            "behind phrase clipped: {row:?}"
        );
        assert!(
            row.chars().count() <= 80,
            "row wider than the screen: {row:?}"
        );
    }

    #[test]
    fn an_app_installed_without_its_clone_never_claims_up_to_date() {
        // launcher/AUR install: binary and menu entry, but no source on disk
        let a = App::with_apps(vec![app_info("photocraft", false, true, None)]);
        let rows = render(&a, 80, 24);
        let joined = rows.join("\n");
        assert!(joined.contains("installed"), "{joined:?}");
        assert!(
            !joined.contains("up to date"),
            "without a clone there is nothing to compare against: {joined:?}"
        );
        assert!(joined.contains("installed, no clone"), "{joined:?}");
    }

    #[test]
    fn attention_filter_shows_only_what_needs_it() {
        let mut a = app(); // photocraft: up to date
        a.list.apps.push(app_info("wordcraft", true, true, Some(2)));
        a.list.apps.push(app_info("cadcraft", false, false, None));
        a.list.sel = 0;
        a.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('o'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(a.list.filter_attention);
        assert_eq!(a.visible_rows(), vec![1, 2], "up-to-date row must hide");
        assert_eq!(a.list.sel, 1, "the highlight must land on a visible row");
        let rows = render(&a, 80, 24);
        assert!(
            rows.iter().any(|r| r.contains("attention only")),
            "the active filter must be visible: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("photocraft")),
            "filtered-out row still rendered: {rows:?}"
        );
    }

    #[test]
    fn expanded_row_shows_an_indented_description() {
        let mut a = app();
        a.list.expanded.insert(0);
        let rows = render(&a, 80, 24);
        let desc = rows
            .iter()
            .find(|r| r.contains("image editor"))
            .expect("description row");
        assert!(
            desc.trim_start_matches('│').starts_with("        "),
            "description must be indented: {desc:?}"
        );
        let app_row = rows
            .iter()
            .find(|r| r.contains("photocraft"))
            .expect("app row");
        assert!(
            app_row.contains('▶'),
            "highlight must stay on the app row: {app_row:?}"
        );
        assert!(
            !desc.contains('▶'),
            "description row must not be selectable"
        );
    }

    #[test]
    fn picked_rows_show_a_filled_square_and_the_title_counts_them() {
        let mut a = app();
        a.list.apps.push(app_info("videocraft", false, false, None));
        a.list.picks.insert(1);
        let rows = render(&a, 80, 24);
        assert!(
            rows.iter().any(|r| r.contains("· 1 picked")),
            "picked count missing from the title: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains('■')),
            "picked row has no filled square: {rows:?}"
        );
        assert!(
            rows.iter().any(|r| r.contains('□')),
            "unpicked row has no empty square: {rows:?}"
        );
    }

    #[test]
    fn wide_fields_do_not_shift_the_phrase_column() {
        let rows = render(&app(), 80, 24);
        let row = rows
            .iter()
            .find(|r| r.contains("photocraft"))
            .expect("row present");
        assert!(row.contains('…'), "version must be truncated, got: {row:?}");
        assert!(
            row.contains("up to date"),
            "the phrase column must stay aligned: {row:?}"
        );
        assert!(row.chars().count() <= 80, "row wider than the screen");
    }

    #[test]
    fn version_column_grows_with_the_terminal() {
        let a = app();
        let narrow = render(&a, 60, 15);
        let narrow_row = narrow
            .iter()
            .find(|r| r.contains("photocraft"))
            .expect("row present");
        assert!(
            narrow_row.contains('…'),
            "expected truncation at 60: {narrow_row:?}"
        );
        let wide = render(&a, 120, 30);
        let wide_row = wide
            .iter()
            .find(|r| r.contains("photocraft"))
            .expect("row present");
        assert!(
            !wide_row.contains('…'),
            "the middle column must use the extra width: {wide_row:?}"
        );
        assert!(
            wide_row.contains("0.5.0 (dev build) with a very long suffix"),
            "{wide_row:?}"
        );
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
    fn mono_palette_emits_no_colored_cells() {
        let mut a = app();
        a.palette = Palette::mono();
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

    #[test]
    fn highlighted_row_text_is_dark_on_the_accent_bar() {
        let mut a = app();
        a.palette = Palette::of(Depth::TrueColor);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &a)).unwrap();
        let buf = term.backend().buffer();
        let row_text = |y: u16| {
            (0..80u16)
                .map(|x| {
                    buf.cell((x, y))
                        .unwrap()
                        .symbol()
                        .chars()
                        .next()
                        .unwrap_or(' ')
                })
                .collect::<String>()
        };
        let y = (0..24u16)
            .find(|&y| row_text(y).contains("photocraft"))
            .expect("photocraft row");
        let dark = Color::Rgb(0x10, 0x10, 0x14);
        let dark_cells = (0..80u16)
            .filter(|&x| {
                let cell = buf.cell((x, y)).unwrap();
                cell.symbol() != " " && cell.fg == dark
            })
            .count();
        assert!(
            dark_cells > 10,
            "the highlighted row must be dark ink, got {dark_cells} dark cells"
        );
        assert!(
            (0..80u16).any(|x| buf.cell((x, y)).unwrap().bg == Color::Rgb(0x2d, 0x81, 0xff)),
            "accent bar missing behind the row"
        );
    }

    #[test]
    fn truecolor_palette_paints_status_in_brand_rgb() {
        let mut a = app();
        a.palette = Palette::of(Depth::TrueColor);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &a)).unwrap();
        let buf = term.backend().buffer();
        let teal = Color::Rgb(0x3f, 0xd6, 0xb8);
        let found = (0..24u16).any(|y| (0..80u16).any(|x| buf.cell((x, y)).unwrap().fg == teal));
        assert!(found, "no cell painted with the brand 'good' teal");
    }

    #[test]
    fn ansi16_palette_still_marks_status() {
        let mut a = app();
        a.palette = Palette::of(Depth::Ansi16);
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| draw(f, &a)).unwrap();
        let buf = term.backend().buffer();
        let found =
            (0..24u16).any(|y| (0..80u16).any(|x| buf.cell((x, y)).unwrap().fg == Color::Green));
        assert!(found, "no cell painted with the 16-colour 'good' green");
    }
}

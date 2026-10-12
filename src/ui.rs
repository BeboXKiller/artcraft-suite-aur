//! Layout and widgets: header, suite list, log tail, key bar.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::engine;

pub fn draw(frame: &mut Frame, app: &App) {
    let [head, main, tail, keys_row] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let busy = engine::busy_summary(&app.busy);
    let is_busy = busy.is_some();
    let status = busy.unwrap_or_else(|| app.msg.clone());
    let head_text = vec![
        Line::from(vec![
            Span::styled(" artcraft-suite-aur", Style::default().bold().fg(Color::Cyan)),
            Span::raw(" — install · update · launch the Craft suite from source"),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                status,
                Style::default().fg(if is_busy { Color::Yellow } else { Color::Gray }),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(head_text).block(Block::bordered().title(" ArtCraft Suite ")),
        head,
    );

    let items: Vec<ListItem> = app
        .apps
        .iter()
        .map(|a| {
            let (dot, dot_color) = if app.busy.contains_key(a.repo) {
                ("◐", Color::Yellow)
            } else if a.installed {
                ("●", Color::Green)
            } else if a.cloned {
                ("○", Color::DarkGray)
            } else {
                ("·", Color::DarkGray)
            };
            let version = a.version.as_deref().unwrap_or("-");
            let behind = match a.behind {
                Some(n) => format!("  ↑{n}"),
                None => String::new(),
            };
            let pkg = a.pkg.clone().unwrap_or_else(|| "-".into());
            let state = if a.installed { " installed" } else if a.cloned { " cloned" } else { "" };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {dot} "), Style::default().fg(dot_color)),
                Span::styled(format!("{:<16}", a.repo), Style::default().bold()),
                Span::styled(format!("{:<22}", pkg), Style::default().fg(Color::Gray)),
                Span::styled(format!("{:<14}", version), Style::default().fg(Color::White)),
                Span::styled(behind, Style::default().fg(Color::Magenta)),
                Span::styled(state, Style::default().fg(Color::Gray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::bordered().title(format!(" Apps ({}) ", app.apps.len())))
        .highlight_style(Style::default().bg(Color::DarkGray).bold())
        .highlight_symbol("▶ ");
    let mut state = ListState::default();
    state.select(Some(app.sel));
    frame.render_stateful_widget(list, main, &mut state);

    let sel_repo = app.apps.get(app.sel).map(|a| a.repo).unwrap_or("");
    let log_text = engine::log_tail(sel_repo, 4096).unwrap_or_else(|| "(no build log for the selected app)".into());
    let all: Vec<&str> = log_text.lines().collect();
    let start = all.len().saturating_sub(4);
    let tail_lines: Vec<Line> = all[start..]
        .iter()
        .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(Color::Gray))))
        .collect();
    frame.render_widget(
        Paragraph::new(tail_lines).block(Block::bordered().title(format!(" Log — {sel_repo} "))),
        tail,
    );

    let keys = Line::from(vec![
        Span::styled("[Enter]", Style::default().fg(Color::Cyan)),
        Span::raw(" open/install  "),
        Span::styled("[u]", Style::default().fg(Color::Cyan)),
        Span::raw(" update  "),
        Span::styled("[U]", Style::default().fg(Color::Cyan)),
        Span::raw(" update all  "),
        Span::styled("[a]", Style::default().fg(Color::Cyan)),
        Span::raw(" install all  "),
        Span::styled("[f]", Style::default().fg(Color::Cyan)),
        Span::raw(" fetch  "),
        Span::styled("[r]", Style::default().fg(Color::Cyan)),
        Span::raw(" refresh  "),
        Span::styled("[q]", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]);
    frame.render_widget(Paragraph::new(keys).centered(), keys_row);
}

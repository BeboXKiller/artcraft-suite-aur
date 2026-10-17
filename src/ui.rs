//! Layout and widgets: header, suite list, details pane, key bar, help overlay.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, Tone};
use crate::engine::{self, AppInfo, Track};
use crate::theme::{Sty, Tint};

/// Smallest grid this layout is designed for; below it we show a polite message.
const MIN_WIDTH: u16 = 60;
const MIN_HEIGHT: u16 = 15;
/// Detail lines kept visible in the bottom pane (the block is `Length(7)`).
const DETAIL_LINES: usize = 5;

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
    let tone_tint = match tone {
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
        Line::from(vec![Span::raw(" "), Span::styled(status, s.fg(tone_tint))]),
    ];
    let mut head_block = Block::bordered().title(" ArtCraft Suite ");
    let summary = summary(app, s);
    if !summary.is_empty() {
        head_block = head_block.title(Line::from(summary).right_aligned());
    }
    frame.render_widget(Paragraph::new(head_text).block(head_block), head);

    draw_list(frame, app, main, s);
    draw_details(frame, app, tail, s);
    frame.render_widget(
        Paragraph::new(key_bar(s)).alignment(Alignment::Center),
        keys_row,
    );

    if app.show_help {
        draw_help(frame, area, s, app.help_scroll);
    }
    if let Some(targets) = &app.confirm_uninstall {
        draw_confirm_uninstall(frame, area, s, app, targets);
    }
    if app.jobs_popup {
        draw_jobs_popup(frame, area, s, app.jobs_scratch);
    }
}

/// Right-aligned header counters: how much is current, behind, or missing.
fn summary(app: &App, s: Sty) -> Vec<Span<'static>> {
    if app.apps.is_empty() {
        return Vec::new();
    }
    let behind = app
        .apps
        .iter()
        .filter(|a| a.behind.is_some_and(|n| n > 0))
        .count();
    // nothing present at all: no clone *and* no installed binary — an app
    // that is installed without its source is not "missing"
    let missing = app
        .apps
        .iter()
        .filter(|a| !a.cloned && !a.installed)
        .count();
    let up = app
        .apps
        .iter()
        .filter(|a| a.installed && !a.stale && (a.behind == Some(0) || !a.cloned))
        .count();
    // clones whose upstream check hasn't landed — unknown, not "current"
    let unchecked = app
        .apps
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
    if app.filter_attention {
        spans.push(Span::styled(" attention ", s.fg(Tint::Accent).bold()));
    }
    if app.track == Track::Releases {
        spans.push(Span::styled(" ⚑ releases ", s.fg(Tint::Accent)));
    }
    if !app.busy.is_empty() {
        spans.push(Span::styled(
            format!(" ⚙ {}/{} ", app.busy.len(), app.max_parallel),
            s.fg(Tint::Accent),
        ));
    }
    spans
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect, s: Sty) {
    // borders (2) + highlight symbol (2)
    let inner_w = area.width.saturating_sub(4) as usize;
    // Responsive columns: mark(3) + glyph(6) are fixed, the status phrase keeps
    // a 22-column reserve, and repo + version share the rest — version taking
    // two thirds of the slack, so the middle column stops being the cut-off one
    // as the terminal grows. Below 52 usable columns the version hides entirely.
    // The phrase reserve is sized for the longest phrase ("999 commits behind",
    // 19 chars) plus a little right padding so it never crowds the border.
    let show_version = inner_w >= 52;
    let slack = inner_w.saturating_sub(9 + 16 + 11 + 22);
    let version_extra = slack * 2 / 3;
    let (repo_w, version_w) = if show_version {
        (16 + slack - version_extra, 11 + version_extra)
    } else {
        (16 + 11 + slack, 0)
    };

    let vis = app.visible_rows();
    let mut items: Vec<ListItem> = Vec::with_capacity(vis.len() * 2);
    let mut sel_pos = None;
    for &idx in &vis {
        let a = &app.apps[idx];
        let (glyph, phrase, tint) = row_state(app, a);
        let selected = idx == app.sel;
        let bar = s.on_bar();
        let cell = |t: Tint| if selected { bar } else { s.fg(t) };
        let (mark, mark_style) = if app.picks.contains(&idx) {
            (
                "■",
                if selected {
                    bar.bold()
                } else {
                    s.fg(Tint::Accent).bold()
                },
            )
        } else {
            ("□", cell(Tint::Faint))
        };
        let mut spans = vec![
            Span::styled(format!(" {mark} "), mark_style),
            Span::styled(format!(" {} ", pad(&glyph, 4)), cell(tint)),
            Span::styled(
                pad(a.repo, repo_w),
                if selected { bar.bold() } else { s.bold() },
            ),
        ];
        if show_version {
            spans.push(Span::styled(
                pad(a.version.as_deref().unwrap_or("-"), version_w),
                cell(Tint::Muted),
            ));
        }
        spans.push(Span::styled(format!(" {phrase}"), cell(tint)));
        if selected {
            sel_pos = Some(items.len());
        }
        items.push(ListItem::new(Line::from(spans)));
        if app.expanded.contains(&idx) {
            items.push(ListItem::new(Line::from(Span::styled(
                format!(
                    "        {}",
                    clip(
                        crate::engine::description_of(a.repo),
                        inner_w.saturating_sub(8)
                    )
                ),
                s.fg(Tint::Muted),
            ))));
        }
    }

    let mut title = format!(" Apps ({})", vis.len());
    if !app.picks.is_empty() {
        title.push_str(&format!(" · {} picked", app.picks.len()));
    }
    if app.filter_attention {
        title.push_str(" · attention only");
    }
    title.push(' ');
    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(s.hl())
        .highlight_symbol("▶ ");
    let mut state = ListState::default();
    state.select(sel_pos);
    frame.render_stateful_widget(list, area, &mut state);
}

/// One row's state: a glyph, the same fact in words, and its brand colour.
///
/// Colour never carries the meaning alone — the phrase and the glyph do.
fn row_state(app: &App, a: &AppInfo) -> (String, String, Tint) {
    if let Some(verb) = app.busy.get(a.repo) {
        return ("◐".into(), format!("{verb}…"), Tint::Accent);
    }
    if app.pending.iter().any(|(r, _)| r == a.repo) {
        return ("◌".into(), "queued".into(), Tint::Accent);
    }
    if app.failed.contains_key(a.repo) {
        return ("✗".into(), "build failed".into(), Tint::Bad);
    }
    if a.installed {
        if !a.cloned {
            // installed by the launcher/an AUR package, no source on disk:
            // there is nothing to compare against, so don't claim "up to date"
            return ("✓".into(), "installed".into(), Tint::Good);
        }
        match a.behind {
            Some(n) if n > 0 => (
                behind_glyph(n),
                format!("{n} {} behind", behind_word(app.track, n)),
                Tint::Warn,
            ),
            // the clone is current but the binary was built from an older
            // HEAD — a pull whose build never finished. Say so; `u` rebuilds.
            Some(_) if a.stale => ("↻".into(), "update ready".into(), Tint::Warn),
            Some(_) => ("✓".into(), "up to date".into(), Tint::Good),
            // cloned, but the upstream check hasn't landed (offline or no
            // upstream) — say so instead of trusting stale refs
            None if a.stale => ("↻".into(), "update ready".into(), Tint::Warn),
            None => ("?".into(), "not checked".into(), Tint::Muted),
        }
    } else if a.cloned {
        ("○".into(), "cloned, not built".into(), Tint::Muted)
    } else {
        ("·".into(), "not installed".into(), Tint::Faint)
    }
}

fn behind_glyph(n: u64) -> String {
    if n < 100 {
        format!("↑{n}")
    } else {
        "↑99+".into()
    }
}

fn commit_word(n: u64) -> &'static str {
    if n == 1 { "commit" } else { "commits" }
}

/// "commit(s)" or "release(s)" — what `behind` counts under the current track.
fn behind_word(track: Track, n: u64) -> &'static str {
    match track {
        Track::Commits => commit_word(n),
        Track::Releases => {
            if n == 1 {
                "release"
            } else {
                "releases"
            }
        }
    }
}

/// The bottom pane: the live log while a task runs, the update's contents
/// otherwise — what the next `u` would actually bring.
fn draw_details(frame: &mut Frame, app: &App, area: Rect, s: Sty) {
    let width = area.width.saturating_sub(4) as usize;
    let Some(a) = app.apps.get(app.sel) else {
        frame.render_widget(
            Paragraph::new(" nothing to show — no apps scanned ")
                .block(Block::bordered().title(" Details ")),
            area,
        );
        return;
    };
    let (title, lines) = details_for(app, a, s, width);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

fn details_for<'a>(app: &App, a: &AppInfo, s: Sty, width: usize) -> (String, Vec<Line<'a>>) {
    let repo = a.repo;
    if let Some(verb) = app.busy.get(repo) {
        let title = format!(" {verb} — {repo} ");
        let mut lines = Vec::new();
        if let Some(p) = app.progress.get(repo) {
            lines.push(progress_line(p, s, width));
        }
        lines.extend(tail_lines(
            &app.log,
            s.fg(Tint::Muted),
            width,
            "(waiting for output…)",
        ));
        return (title, lines);
    }
    if let Some(err) = app.failed.get(repo) {
        let title = format!(" ✗ {repo} — build failed ");
        let mut lines = vec![Line::from(Span::styled(format!(" {err}"), s.fg(Tint::Bad)))];
        lines.extend(tail_lines(
            &app.log,
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
                format!(" {version} · {n} {} to pull", behind_word(app.track, n)),
                s.fg(Tint::Warn).bold(),
            ))];
            if a.commits.is_empty() {
                lines.push(Line::from(Span::styled(
                    match app.track {
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
            let eta_str = eta.map_or_else(|| "…".into(), crate::app::coarse_eta);
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
            format!(
                " building… {} elapsed",
                crate::app::coarse_eta(p.elapsed_secs())
            ),
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

fn key_bar(s: Sty) -> Vec<Line<'static>> {
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

fn draw_help(frame: &mut Frame, area: Rect, s: Sty, scroll: usize) {
    let h = s.fg(Tint::Accent).bold();
    // Every hint with a one-line description of what it actually does. Kept
    // within 58 columns so nothing wraps on the 60x15 minimum; longer than a
    // small screen, so j/k scroll it.
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

/// Centered confirmation for `d`: deleting the clone deserves more than a
/// status-line nudge — a small card, the names, what goes, how to say yes.
fn draw_confirm_uninstall(frame: &mut Frame, area: Rect, s: Sty, app: &App, targets: &[usize]) {
    let mut lines: Vec<Line> = Vec::new();
    let names: Vec<&str> = targets
        .iter()
        .filter_map(|i| app.apps.get(*i).map(|a| a.repo))
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

/// Centered editor for the parallel-build limit (`p`): the number, the honest
/// warning for weak machines, and how to change or leave.
fn draw_jobs_popup(frame: &mut Frame, area: Rect, s: Sty, scratch: usize) {
    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {scratch}"),
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

/// Truncate to at most `w` characters, marking the cut with an ellipsis.
fn clip(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
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
    fn header_summarises_the_suite() {
        let mut a = app();
        a.apps.push(app_info("wordcraft", true, true, Some(3)));
        a.apps.push(app_info("cadcraft", false, false, None));
        let rows = render(&a, 80, 24);
        let head = rows[0].clone();
        assert!(head.contains("✓ 1"), "up-to-date count missing: {head:?}");
        assert!(head.contains("↑ 1"), "behind count missing: {head:?}");
        assert!(head.contains("· 1"), "missing count missing: {head:?}");
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
        a.apps.push(app_info("wordcraft", true, true, None));
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
        a.sel = 1;
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("wordcraft — not checked"), "{joined:?}");
        assert!(joined.contains("upstream not checked"), "{joined:?}");
        assert!(joined.contains("f fetches · u updates"), "{joined:?}");
    }

    #[test]
    fn the_attention_filter_keeps_unchecked_clones() {
        let mut a = app(); // photocraft: checked, up to date
        a.apps.push(app_info("wordcraft", true, true, None));
        assert!(
            a.needs_attention(&a.apps[1]),
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
        a.apps.push(app_info("wordcraft", true, true, Some(0)));
        a.apps[1].stale = true;
        a.apps[1].version = Some("0.11.0".into());
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
        a.sel = 1;
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
        a.apps.push(app_info("wordcraft", true, true, Some(3)));
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
        a.apps.push(app_info("wordcraft", true, true, Some(2)));
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
        b.apps.push(app_info("gridcraft", true, true, Some(1)));
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
        a.apps.push(app_info("wordcraft", true, true, Some(2)));
        let row = render(&a, 80, 24)
            .iter()
            .find(|r| r.contains("wordcraft"))
            .expect("row present")
            .clone();
        assert!(row.contains("2 commits behind"), "{row:?}");
        assert!(!row.contains("release"), "{row:?}");
    }

    #[test]
    fn releases_badge_shows_in_the_header_only_in_releases_mode() {
        let mut a = app();
        a.apps.push(app_info("wordcraft", true, true, Some(0)));
        let head = render(&a, 80, 24)[0].clone();
        assert!(!head.contains("releases"), "no badge expected: {head:?}");
        a.track = Track::Releases;
        let head = render(&a, 80, 24)[0].clone();
        assert!(head.contains("⚑ releases"), "badge missing: {head:?}");
    }

    #[test]
    fn details_pane_lists_release_tags() {
        let mut a = app();
        a.track = Track::Releases;
        let mut b = app_info("wordcraft", true, true, Some(2));
        b.commits = vec!["v0.14.0".into(), "v0.15.0".into()];
        a.apps.push(b);
        a.sel = 1;
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("2 releases to pull"), "{joined:?}");
        assert!(joined.contains("v0.15.0"), "{joined:?}");
    }

    #[test]
    fn a_three_digit_behind_count_keeps_the_phrase_and_glyph_intact() {
        // At 3 digits "100 commits behind" is 18 chars and the glyph is
        // "↑99+" (4 chars). Both used to overflow their columns and clip the
        // tail, so the phrase lost its "behind" and the glyph read "↑9…".
        let mut a = app();
        a.apps.push(app_info("wordcraft", true, true, Some(1234)));
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
    fn details_pane_shows_what_the_update_contains() {
        let mut a = app();
        let mut b = app_info("wordcraft", true, true, Some(2));
        b.commits = vec![
            "abc1234 fix table borders".into(),
            "def5678 faster previews".into(),
        ];
        a.apps.push(b);
        a.sel = 1;
        let rows = render(&a, 80, 24);
        let joined = rows.join("\n");
        assert!(joined.contains("What's new in wordcraft"), "{joined:?}");
        assert!(joined.contains("fix table borders"), "{joined:?}");
        assert!(joined.contains("2 commits to pull"), "{joined:?}");
    }

    #[test]
    fn details_pane_shows_the_live_log_while_busy() {
        let mut a = app();
        a.busy.insert("photocraft".into(), "installing".into());
        a.log = vec!["   Compiling photocraft v0.5.0".into()];
        let rows = render(&a, 80, 24);
        let joined = rows.join("\n");
        assert!(joined.contains("installing — photocraft"), "{joined:?}");
        assert!(joined.contains("Compiling photocraft"), "{joined:?}");
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
        a.busy.insert("photocraft".into(), "building".into());
        a.progress.insert(
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
        a.busy.insert("photocraft".into(), "building".into());
        a.progress.insert(
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
        a.busy.insert("photocraft".into(), "building".into());
        a.progress.insert(
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
        a.apps.push(app_info("wordcraft", true, true, Some(2)));
        a.apps.push(app_info("cadcraft", false, false, None));
        a.sel = 0;
        a.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('o'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(a.filter_attention);
        assert_eq!(a.visible_rows(), vec![1, 2], "up-to-date row must hide");
        assert_eq!(a.sel, 1, "the highlight must land on a visible row");
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
        a.expanded.insert(0);
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
        a.apps.push(app_info("videocraft", false, false, None));
        a.picks.insert(1);
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
    fn help_overlay_renders_on_top() {
        let mut a = app();
        a.show_help = true;
        let joined = render(&a, 100, 30).join("\n");
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
        let mut a = app();
        a.show_help = true;
        a.help_scroll = 20;
        let joined = render(&a, 80, 24).join("\n");
        assert!(joined.contains("Reading a row"), "{joined:?}");
        assert!(
            !joined.contains("Navigation"),
            "scroll offset ignored: {joined:?}"
        );
    }

    #[test]
    fn jobs_popup_shows_the_value_and_the_low_spec_warning() {
        let mut a = app();
        a.jobs_popup = true;
        a.jobs_scratch = 5;
        let joined = render(&a, 80, 24).join("\n");
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
        let b = app();
        assert!(
            !render(&b, 80, 24).join("\n").contains("Parallel builds"),
            "popup leaked into a normal frame"
        );
    }

    #[test]
    fn header_counts_running_builds_against_the_limit() {
        let mut a = app();
        a.busy.insert("photocraft".into(), "installing".into());
        a.max_parallel = 2;
        let head = render(&a, 80, 24).join("\n");
        assert!(head.contains("⚙ 1/2"), "no parallel badge: {head:?}");
        let b = app();
        assert!(
            !render(&b, 80, 24).join("\n").contains('⚙'),
            "idle badge leaked"
        );
    }

    #[test]
    fn confirm_uninstall_is_a_centered_card_not_a_full_screen() {
        let mut a = app();
        a.confirm_uninstall = Some(vec![0]);
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
        let b = app();
        assert!(
            !render(&b, 80, 24).join("\n").contains("Confirm uninstall"),
            "modal leaked into a normal frame"
        );
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
        // first screen: nothing wraps at 60 columns (every line fits 58)
        let top = render(&a, 60, 15).join("\n");
        assert!(top.contains("Navigation"), "{top:?}");
        assert!(
            top.contains("attention only (hide apps that are fine)"),
            "{top:?}"
        );
        // scrolled to the end: the legend is reachable too, and the offset clamps
        a.help_scroll = 99;
        let bottom = render(&a, 60, 15).join("\n");
        assert!(bottom.contains("NO_COLOR=1"), "{bottom:?}");
        assert!(
            !bottom.contains("Navigation"),
            "scroll clamp not applied: {bottom:?}"
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

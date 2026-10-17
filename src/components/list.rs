//! The suite list: rows, highlight, picks, attention filter.
//!
//! Step 4b adds the rendering (`render`, `row_state` and helpers) moved
//! verbatim from `ui.rs`; task state is passed as arguments. The `enter`/pick
//! decision logic stays in `app.rs` until step 4c. Task-dependent reads take
//! the failed map as an argument (data flows as arguments; TaskManager owns
//! that data from step 5 on).

use std::collections::{HashMap, HashSet, VecDeque};

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::components::ComponentEvent;
use crate::engine::{self, Action, AppInfo, Track};
use crate::theme::{Sty, Tint};

#[derive(Default)]
pub struct ListComponent {
    pub apps: Vec<AppInfo>,
    pub sel: usize,
    pub picks: HashSet<usize>,
    pub filter_attention: bool,
    /// apps whose description row is open (toggled with → / ←)
    pub expanded: HashSet<usize>,
}

/// Read-only slice of the task state the list needs to draw a row. Keeps
/// [`ListComponent::render`] inside clippy's argument limit without a wider
/// context argument; TaskManager hands these out from step 5 on.
pub struct TaskView<'a> {
    pub busy: &'a HashMap<String, String>,
    pub pending: &'a VecDeque<(String, Action)>,
    pub failed: &'a HashMap<String, String>,
    pub track: Track,
}

impl ListComponent {
    /// Rows the list currently shows: all of them, or just the ones needing
    /// attention. `failed` comes from the task side (see the module doc).
    pub fn visible_rows(&self, failed: &HashMap<String, String>) -> Vec<usize> {
        if !self.filter_attention {
            return (0..self.apps.len()).collect();
        }
        self.apps
            .iter()
            .enumerate()
            .filter(|(_, a)| self.needs_attention(a, failed))
            .map(|(i, _)| i)
            .collect()
    }

    /// Missing, behind upstream, unchecked, stale install, or failed — the
    /// rows `o` keeps.
    pub fn needs_attention(&self, a: &AppInfo, failed: &HashMap<String, String>) -> bool {
        !a.installed
            || failed.contains_key(a.repo)
            || a.behind.is_some_and(|n| n > 0)
            || a.stale
            || (a.cloned && a.behind.is_none())
    }

    /// Step the highlight by `step` rows *within what is visible*.
    pub fn move_sel(&mut self, step: isize, failed: &HashMap<String, String>) {
        let vis = self.visible_rows(failed);
        if vis.is_empty() {
            return;
        }
        let pos = vis.iter().position(|&i| i == self.sel).unwrap_or(0) as isize;
        let next = (pos + step).clamp(0, vis.len() as isize - 1) as usize;
        self.sel = vis[next];
    }

    /// Keep the highlight on a row the list is actually showing. An empty
    /// view drops the filter rather than trapping the user on nothing.
    pub fn clamp_selection(&mut self, failed: &HashMap<String, String>) {
        let vis = self.visible_rows(failed);
        if vis.is_empty() {
            self.filter_attention = false;
            self.sel = self.sel.min(self.apps.len().saturating_sub(1));
            return;
        }
        if !vis.contains(&self.sel) {
            self.sel = vis
                .iter()
                .copied()
                .find(|&i| i >= self.sel)
                .unwrap_or_else(|| *vis.last().unwrap());
        }
    }

    pub fn row_label(&self, idx: usize) -> String {
        self.apps
            .get(idx)
            .map(|a| a.repo.to_string())
            .unwrap_or_else(|| "?".into())
    }

    /// Rows an operation applies to: the picked set, else the highlighted row.
    pub fn targets(&self) -> Vec<usize> {
        if self.picks.is_empty() {
            vec![self.sel]
        } else {
            let mut v: Vec<usize> = self.picks.iter().copied().collect();
            v.sort_unstable();
            v
        }
    }

    /// `Space`: pick just this row; pressing it again on the only pick clears
    /// it. Returns the status message for App to apply.
    pub fn pick_one(&mut self) -> ComponentEvent {
        if self.picks.len() == 1 && self.picks.contains(&self.sel) {
            self.picks.clear();
            ComponentEvent::Msg("picks cleared".into())
        } else {
            self.picks.clear();
            self.picks.insert(self.sel);
            ComponentEvent::Msg(format!("picked {}", self.row_label(self.sel)))
        }
    }

    /// `Ctrl+Space`: add or remove this row from the picked set. Returns the
    /// status message for App to apply.
    pub fn pick_toggle(&mut self) -> ComponentEvent {
        let label = self.row_label(self.sel);
        if self.picks.remove(&self.sel) {
            ComponentEvent::Msg(format!("unpicked {label} ({} left)", self.picks.len()))
        } else {
            self.picks.insert(self.sel);
            ComponentEvent::Msg(format!("picked {label} ({} total)", self.picks.len()))
        }
    }

    /// The list pane. Task state arrives as a [`TaskView`] (see the module
    /// doc). Moved verbatim from `ui::draw_list`.
    pub fn render(&self, frame: &mut Frame, area: Rect, s: Sty, tasks: TaskView) {
        let TaskView {
            busy,
            pending,
            failed,
            track,
        } = tasks;
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

        let vis = self.visible_rows(failed);
        let mut items: Vec<ListItem> = Vec::with_capacity(vis.len() * 2);
        let mut sel_pos = None;
        for &idx in &vis {
            let a = &self.apps[idx];
            let (glyph, phrase, tint) = row_state(a, busy, pending, failed, track);
            let selected = idx == self.sel;
            let bar = s.on_bar();
            let cell = |t: Tint| if selected { bar } else { s.fg(t) };
            let (mark, mark_style) = if self.picks.contains(&idx) {
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
            if self.expanded.contains(&idx) {
                items.push(ListItem::new(Line::from(Span::styled(
                    format!(
                        "        {}",
                        clip(engine::description_of(a.repo), inner_w.saturating_sub(8))
                    ),
                    s.fg(Tint::Muted),
                ))));
            }
        }

        let mut title = format!(" Apps ({})", vis.len());
        if !self.picks.is_empty() {
            title.push_str(&format!(" · {} picked", self.picks.len()));
        }
        if self.filter_attention {
            title.push_str(" · attention only");
        }
        title.push(' ');
        let item_count = items.len();
        let list = List::new(items)
            .block(Block::bordered().title(title))
            .highlight_style(s.hl())
            .highlight_symbol("▶ ");
        let mut state = ListState::default();
        state.select(sel_pos);
        frame.render_stateful_widget(list, area, &mut state);

        // The row window is smaller than the list — a thumb on the right
        // border shows where the viewport sits. Only on overflow: a thumb on
        // a short list would promise scrolling that does nothing.
        let viewport = usize::from(area.height.saturating_sub(2));
        if item_count > viewport && viewport > 0 {
            let mut bar = ScrollbarState::new(item_count).position(sel_pos.unwrap_or(0));
            frame.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .thumb_symbol("█")
                    .track_symbol(None)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .style(s.fg(Tint::Faint)),
                area.inner(Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut bar,
            );
        }
    }
}

/// One row's state: a glyph, the same fact in words, and its brand colour.
///
/// Colour never carries the meaning alone — the phrase and the glyph do.
/// Moved verbatim from `ui::row_state`.
pub fn row_state(
    a: &AppInfo,
    busy: &HashMap<String, String>,
    pending: &VecDeque<(String, Action)>,
    failed: &HashMap<String, String>,
    track: Track,
) -> (String, String, Tint) {
    if let Some(verb) = busy.get(a.repo) {
        return ("◐".into(), format!("{verb}…"), Tint::Accent);
    }
    if pending.iter().any(|(r, _)| r == a.repo) {
        return ("◌".into(), "queued".into(), Tint::Accent);
    }
    if failed.contains_key(a.repo) {
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
                format!("{n} {} behind", behind_word(track, n)),
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

pub(crate) fn behind_glyph(n: u64) -> String {
    if n < 100 {
        format!("↑{n}")
    } else {
        "↑99+".into()
    }
}

pub(crate) fn commit_word(n: u64) -> &'static str {
    if n == 1 { "commit" } else { "commits" }
}

/// "commit(s)" or "release(s)" — what `behind` counts under the current track.
pub(crate) fn behind_word(track: Track, n: u64) -> &'static str {
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

/// Pad to `w`, truncating with an ellipsis so a long field can never push the
/// columns after it out of the pane.
pub(crate) fn pad(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        format!("{:1$}", s, w)
    } else {
        let keep = w.saturating_sub(1);
        let cut: String = s.chars().take(keep).collect();
        format!("{cut}…")
    }
}

/// Trim to `w` columns with an ellipsis, for free-form lines.
pub(crate) fn clip(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let keep = w.saturating_sub(1);
        let cut: String = s.chars().take(keep).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(repo: &str, cloned: bool, installed: bool, behind: Option<u64>) -> AppInfo {
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

    fn no_fail() -> HashMap<String, String> {
        HashMap::new()
    }

    #[test]
    fn visible_rows_is_everything_or_only_the_attention_rows() {
        let mut l = ListComponent {
            apps: vec![
                info("a", true, true, Some(0)), // fine
                info("b", false, false, None),  // missing
                info("c", true, true, Some(2)), // behind
            ],
            ..Default::default()
        };
        assert_eq!(l.visible_rows(&no_fail()), vec![0, 1, 2]);
        l.filter_attention = true;
        assert_eq!(l.visible_rows(&no_fail()), vec![1, 2]);
    }

    #[test]
    fn a_failed_row_needs_attention_even_when_installed_and_current() {
        let l = ListComponent {
            apps: vec![info("a", true, true, Some(0))],
            ..Default::default()
        };
        let mut failed = HashMap::new();
        failed.insert("a".to_string(), "boom".to_string());
        assert!(l.needs_attention(&l.apps[0], &failed));
        assert_eq!(l.visible_rows(&failed), vec![0]);
    }

    #[test]
    fn move_sel_walks_the_visible_rows_and_clamps() {
        let mut l = ListComponent {
            apps: vec![
                info("a", true, true, Some(0)),
                info("b", false, false, None),
                info("c", true, true, Some(1)),
            ],
            ..Default::default()
        };
        l.filter_attention = true; // hides row 0; App clamps right after `o`
        l.clamp_selection(&no_fail());
        assert_eq!(l.sel, 1, "clamped onto the first visible row");
        l.move_sel(1, &no_fail());
        assert_eq!(l.sel, 2);
        l.move_sel(1, &no_fail());
        assert_eq!(l.sel, 2, "clamped at the last visible row");
        l.move_sel(-10, &no_fail());
        assert_eq!(l.sel, 1, "clamped at the first visible row");
    }

    #[test]
    fn clamp_selection_lands_on_the_next_visible_row() {
        let mut l = ListComponent {
            apps: vec![
                info("a", true, true, Some(0)),
                info("b", false, false, None),
                info("c", true, true, Some(1)),
            ],
            sel: 0,
            ..Default::default()
        };
        l.filter_attention = true;
        l.clamp_selection(&no_fail());
        assert_eq!(l.sel, 1, "moves forward to the first visible row");
        l.sel = 2;
        l.clamp_selection(&no_fail());
        assert_eq!(l.sel, 2, "already visible: stays");
    }

    #[test]
    fn clamp_selection_drops_the_filter_when_nothing_needs_attention() {
        let mut l = ListComponent {
            apps: vec![info("a", true, true, Some(0))],
            filter_attention: true,
            ..Default::default()
        };
        l.clamp_selection(&no_fail());
        assert!(!l.filter_attention, "an empty view must not trap the user");
        assert_eq!(l.sel, 0);
    }

    #[test]
    fn targets_are_the_picks_sorted_else_the_highlight() {
        let mut l = ListComponent {
            apps: vec![info("a", false, false, None), info("b", false, false, None)],
            sel: 1,
            ..Default::default()
        };
        assert_eq!(l.targets(), vec![1]);
        l.picks.insert(0);
        l.picks.insert(1);
        assert_eq!(l.targets(), vec![0, 1], "picks are sorted, not hashed");
    }

    #[test]
    fn pad_truncates_and_pads() {
        assert_eq!(pad("abc", 6), "abc   ");
        assert_eq!(pad("abcdef", 4), "abc…");
        assert_eq!(pad("abc", 3), "abc");
    }

    #[test]
    fn pick_one_replaces_the_set_and_toggles_the_sole_pick_off() {
        let mut l = ListComponent {
            apps: vec![info("a", false, false, None), info("b", false, false, None)],
            sel: 1,
            ..Default::default()
        };
        match l.pick_one() {
            ComponentEvent::Msg(m) => assert_eq!(m, "picked b"),
            e => panic!("expected a message, got {e:?}"),
        }
        assert_eq!(l.picks, [1usize].into_iter().collect());
        match l.pick_one() {
            ComponentEvent::Msg(m) => assert_eq!(m, "picks cleared"),
            e => panic!("expected a message, got {e:?}"),
        }
        assert!(l.picks.is_empty());
    }

    #[test]
    fn pick_toggle_adds_and_removes_without_dropping_the_rest() {
        let mut l = ListComponent {
            apps: vec![
                info("a", false, false, None),
                info("b", false, false, None),
                info("c", false, false, None),
            ],
            ..Default::default()
        };
        l.pick_toggle();
        l.sel = 2;
        l.pick_toggle();
        assert_eq!(l.picks, [0usize, 2].into_iter().collect());
        l.sel = 0;
        match l.pick_toggle() {
            ComponentEvent::Msg(m) => assert_eq!(m, "unpicked a (1 left)"),
            e => panic!("expected a message, got {e:?}"),
        }
        assert_eq!(l.picks, [2usize].into_iter().collect());
    }
}

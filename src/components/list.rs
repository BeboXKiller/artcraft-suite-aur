//! The suite list: rows, highlight, picks, attention filter.
//!
//! Step 4a of the refactor: state plus the pure row math. Rendering and the
//! `enter`/pick decision logic stay in `ui.rs`/`app.rs` until steps 4b/4c.
//! Task-dependent reads take the failed map as an argument (AppContext stays
//! minimal; TaskManager owns that data from step 5 on).

use std::collections::{HashMap, HashSet};

use crate::engine::AppInfo;

#[derive(Default)]
pub struct ListComponent {
    pub apps: Vec<AppInfo>,
    pub sel: usize,
    pub picks: HashSet<usize>,
    pub filter_attention: bool,
    /// apps whose description row is open (toggled with → / ←)
    pub expanded: HashSet<usize>,
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
}

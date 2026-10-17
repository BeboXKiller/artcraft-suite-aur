//! Background task state: what is running, queued, failed, and its progress.
//!
//! Step 5 of the refactor. The queue/spawn logic (`try_start`, `start_now`,
//! `pump`) stays on `App` in this step — it needs the list's clone dirs and
//! the event sender; this module owns the maps and the pure bookkeeping so
//! `App` is no longer the state bag.

use std::collections::{HashMap, VecDeque};

use crate::engine::{Action, Progress};

/// How many apps may compile at once unless the user says otherwise
/// (`-p` on the CLI, `ARTCRAFT_PARALLEL`, or the `p` popup in the UI).
pub const DEFAULT_MAX_PARALLEL: usize = 2;
/// Smallest and largest accepted parallel-build limit.
pub const MIN_JOBS: usize = 1;
pub const MAX_JOBS: usize = 16;

pub struct TaskManager {
    /// repo → verb ("installing"…) of the running task
    pub busy: HashMap<String, String>,
    /// repo → live build progress, while a build reports it
    pub progress: HashMap<String, Progress>,
    pub pending: VecDeque<(String, Action)>,
    /// repo → error of the last failed task, until a retry clears it
    pub failed: HashMap<String, String>,
    pub max_parallel: usize,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self {
            busy: HashMap::new(),
            progress: HashMap::new(),
            pending: VecDeque::new(),
            failed: HashMap::new(),
            max_parallel: DEFAULT_MAX_PARALLEL,
        }
    }
}

impl TaskManager {
    pub fn new(max_parallel: usize) -> Self {
        Self {
            max_parallel: max_parallel.clamp(MIN_JOBS, MAX_JOBS),
            ..Default::default()
        }
    }

    /// Repo is running or queued.
    pub fn is_staged(&self, repo: &str) -> bool {
        self.busy.contains_key(repo) || self.pending.iter().any(|(r, _)| r == repo)
    }

    /// A finished task lands here: drop it from the runners, record or clear
    /// its failure. `action_label` names the verb for the failure text.
    /// Returns the status-line text for `App` to set.
    pub fn finish(
        &mut self,
        repo: &str,
        action_label: &str,
        result: &Result<String, String>,
    ) -> String {
        self.busy.remove(repo);
        self.progress.remove(repo);
        match result {
            Ok(m) => {
                self.failed.remove(repo);
                format!("✓ {m}")
            }
            Err(e) => {
                self.failed.insert(repo.to_string(), e.clone());
                format!("✗ {repo} · {action_label}: {e}")
            }
        }
    }

    /// Slots free enough to start one more task right now.
    pub fn has_slot(&self) -> bool {
        self.busy.len() < self.max_parallel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn progress() -> Progress {
        Progress {
            done: 0,
            seen: 0,
            total: 0,
            started: Instant::now(),
            pace_secs: None,
            last_real: None,
        }
    }

    #[test]
    fn is_staged_covers_running_and_queued() {
        let mut t = TaskManager::new(2);
        t.busy.insert("a".into(), "installing".into());
        t.pending.push_back(("b".into(), Action::Update));
        assert!(t.is_staged("a"));
        assert!(t.is_staged("b"));
        assert!(!t.is_staged("c"));
    }

    #[test]
    fn finish_clears_the_runner_and_records_the_failure() {
        let mut t = TaskManager::new(2);
        t.busy.insert("a".into(), "installing".into());
        t.progress.insert("a".into(), progress());
        let msg = t.finish("a", "update", &Err("boom".into()));
        assert!(t.busy.is_empty(), "the runner slot must be released");
        assert!(t.progress.is_empty(), "stale progress must not linger");
        assert_eq!(t.failed["a"], "boom");
        assert!(msg.starts_with('✗'), "failure text: {msg}");
    }

    #[test]
    fn finish_on_success_forgets_the_old_failure() {
        let mut t = TaskManager::new(2);
        t.failed.insert("a".into(), "old".into());
        let msg = t.finish("a", "install", &Ok("a 1.0 installed".into()));
        assert!(t.failed.is_empty());
        assert!(msg.starts_with('✓'), "success text: {msg}");
    }

    #[test]
    fn new_clamps_the_parallel_limit() {
        assert_eq!(TaskManager::new(0).max_parallel, MIN_JOBS);
        assert_eq!(TaskManager::new(99).max_parallel, MAX_JOBS);
        assert_eq!(TaskManager::new(3).max_parallel, 3);
    }

    #[test]
    fn default_has_a_usable_parallel_limit() {
        let tasks = TaskManager::default();
        assert_eq!(tasks.max_parallel, DEFAULT_MAX_PARALLEL);
        assert!(tasks.max_parallel >= MIN_JOBS);
        assert!(tasks.has_slot());
    }
}

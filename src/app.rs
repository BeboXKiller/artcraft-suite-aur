//! Application state: selection, background tasks, key handling.

use std::collections::HashMap;
use std::sync::mpsc::{Sender, Receiver};

use crate::engine::{self, Action, AppInfo, Ev};

pub struct App {
    pub apps: Vec<AppInfo>,
    pub sel: usize,
    pub busy: HashMap<String, String>,
    pub msg: String,
    pub quit: bool,
    tx: Sender<Ev>,
}

impl App {
    pub fn new(tx: Sender<Ev>) -> Self {
        Self { apps: engine::scan(), sel: 0, busy: HashMap::new(), msg: "ready".into(), quit: false, tx }
    }

    pub fn drain(&mut self, rx: &Receiver<Ev>) {
        while let Ok(ev) = rx.try_recv() {
            let Ev::Done { repo, action, result } = ev;
            self.busy.remove(&repo);
            self.msg = match result {
                Ok(m) => m,
                Err(e) => format!("{repo} · {}: {e}", action.label()),
            };
            // statuses may have changed (installed, version)
            engine::rescan(&mut self.apps);
        }
    }

    pub fn on_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode::*;
        match code {
            Char('q') | Esc => self.quit = true,
            Up | Char('k') => {
                self.sel = self.sel.saturating_sub(1);
            }
            Down | Char('j') => {
                if self.sel + 1 < self.apps.len() {
                    self.sel += 1;
                }
            }
            Home => self.sel = 0,
            End => self.sel = self.apps.len().saturating_sub(1),
            Enter => self.enter(),
            Char('u') => self.start_selected(Action::Update),
            Char('i') => self.start_selected(Action::Install),
            Char('f') => self.start_selected(Action::Fetch),
            Char('U') => {
                for i in 0..self.apps.len() {
                    let repo = self.apps[i].repo.to_string();
                    if self.apps[i].cloned && !self.busy.contains_key(&repo) {
                        self.busy.insert(repo.clone(), Action::Update.label().into());
                        engine::spawn(repo, Action::Update, self.tx.clone());
                    }
                }
            }
            Char('a') => {
                for i in 0..self.apps.len() {
                    let repo = self.apps[i].repo.to_string();
                    if !self.apps[i].installed && !self.busy.contains_key(&repo) {
                        self.busy.insert(repo.clone(), Action::Install.label().into());
                        engine::spawn(repo, Action::Install, self.tx.clone());
                    }
                }
            }
            Char('r') => {
                engine::rescan(&mut self.apps);
                self.msg = "refreshed".into();
            }
            _ => {}
        }
    }

    fn enter(&mut self) {
        let Some(a) = self.apps.get(self.sel) else { return };
        if self.busy.contains_key(a.repo) {
            self.msg = format!("{} already running for {}", self.busy[a.repo], a.repo);
            return;
        }
        if a.installed {
            let pkg = a.pkg.clone().unwrap_or_else(|| a.repo.to_string());
            self.msg = match engine::launch(&pkg) {
                Ok(()) => format!("launched {pkg}"),
                Err(e) => e,
            };
        } else {
            self.start_selected(Action::Install);
        }
    }

    fn start_selected(&mut self, action: Action) {
        let Some(a) = self.apps.get(self.sel) else { return };
        if self.busy.contains_key(a.repo) {
            self.msg = format!("{} already running for {}", self.busy[a.repo], a.repo);
            return;
        }
        let repo = a.repo.to_string();
        self.busy.insert(repo.clone(), action.label().into());
        engine::spawn(repo, action, self.tx.clone());
    }
}

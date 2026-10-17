//! Concrete TUI components and their small key-dispatch interface.
//!
//! Rendering uses concrete methods with explicit data arguments; `App` keeps
//! orchestration and dispatches keys to the components that own key state.

use crossterm::event::KeyEvent;

pub mod details;
pub mod header;
pub mod help;
pub mod jobs;
pub mod keybar;
pub mod list;
pub mod uninstall;
pub use details::DetailsComponent;
pub use header::HeaderComponent;
pub use help::HelpOverlayComponent;
pub use jobs::JobsPopupComponent;
pub use keybar::KeyBarComponent;
pub use list::ListComponent;
pub use uninstall::UninstallConfirmComponent;

/// Outcomes returned by components that own key handling.
#[derive(Debug)]
pub enum ComponentEvent {
    Ignored,
    Msg(String),
    JobsApply,
    CloseOverlay,
}

pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> ComponentEvent {
        ComponentEvent::Ignored
    }
}

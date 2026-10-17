//! Component scaffolding for the TUI refactor.
//!
//! Each overlay/pane becomes a concrete struct in its own file; the trait here
//! exists for uniformity, not for a `dyn` registry — dispatch is static and
//! `App` calls each component directly. See `REFACTOR_PLAN.md` for the layout
//! and the justified deviations from the original sketch.

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::engine::{Action, Ev};
use crate::theme::Palette;

pub mod help;
pub mod jobs;
pub mod uninstall;
pub use help::HelpOverlayComponent;
pub use jobs::JobsPopupComponent;
pub use uninstall::UninstallConfirmComponent;

/// Shared read-only context handed to every render. Kept minimal on purpose:
/// components that need data take it as method arguments instead.
#[expect(
    dead_code,
    reason = "App::render adopts AppContext when ui.rs is deleted (step 8)"
)]
pub struct AppContext<'a> {
    pub palette: &'a Palette,
}

/// What a component wants `App` to do after a key or background event.
/// Discovered from every key path in `BEHAVIOR_CONTRACT.md`: messages, task
/// requests, rescans and the two-step quit all surface through this enum.
#[derive(Debug)]
#[expect(
    dead_code,
    reason = "variants light up as each component lands (steps 2-7)"
)]
pub enum ComponentEvent {
    Ignored,
    /// Set the status-line message.
    Msg(String),
    /// Apply this action to the list's current targets (picks or sel).
    RequestAction(Action),
    /// Uninstall the current targets (App computes eligibility).
    UninstallRequest,
    /// Confirm the pending uninstall (`y`).
    UninstallConfirm,
    /// Open the parallel-builds popup.
    JobsOpen,
    /// Adopt the popup's scratch limit (`Enter` in the `p` editor).
    JobsApply,
    /// Full rescan (`r`) — App owns the pipeline.
    Rescan,
    /// Flip the track (`t`) and rescan.
    ToggleTrack,
    /// Launch the selected installed app; App resolves the package.
    Launch,
    /// Two-step quit request.
    RequestQuit,
    OpenOverlay(OverlayKind),
    CloseOverlay,
    Quit,
}

/// Overlay identities for the `OpenOverlay`/`CloseOverlay` events.
///
/// The three overlays stay *independent* (they can be open simultaneously, as
/// they are today); this enum only names them in events. `App` keeps the exact
/// key precedence and paint order from the contract.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[expect(
    dead_code,
    reason = "named in events once the uninstall/jobs overlays land (steps 2-3)"
)]
pub enum OverlayKind {
    Help,
    UninstallConfirm,
    JobsPopup,
}

pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> ComponentEvent {
        ComponentEvent::Ignored
    }
    #[expect(dead_code, reason = "used when TaskManager starts routing Ev (step 5)")]
    fn handle_background(&mut self, _ev: &Ev) {}
    #[expect(
        dead_code,
        reason = "App::render calls it when ui.rs is deleted (step 8)"
    )]
    fn render(&mut self, _frame: &mut Frame, _area: Rect, _ctx: &AppContext) {}
}

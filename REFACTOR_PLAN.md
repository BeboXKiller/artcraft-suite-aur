# Refactor Plan — Component Architecture

Turns the `App`/`ui.rs` monolith into one file per component behind a
`Component` trait. Behavior is frozen by `BEHAVIOR_CONTRACT.md`; every step
diffs against it.

## Ground rules (from the brief, restated)

1. Behavior preservation is non-negotiable. Preserve the quirks listed in the
   contract (help/`?`/Ctrl precedence, End/G gap in help, uninstall `y`
   fall-through, `confirm_quit` clearing rules, overlay paint order).
2. No logic changes to `engine.rs` or `theme.rs`. Visibility changes only.
3. No new dependencies. No `dyn Component`, no trait-object registry, no
   async, no gratuitous generics — concrete structs, static dispatch.
4. One component per file, under `src/components/`; the task manager lives in
   `src/task/mod.rs`.
5. `AppContext` stays minimal (palette only). If a component needs data,
   prefer method arguments over growing `AppContext`.
6. `ui.rs` is deleted at the very end, after every consumer has moved.
7. `main.rs` keeps its entry behavior; only the two call sites adapt.
8. All tests survive. Test strategy (decision 1A): adapt in place — same
   assertions, new construction boundary. Each component's tests move into its
   own file.
9. Branch `refactor/component-arch`, one commit per component, never squash,
   ~200-line step cap. Net change over 200 lines ⇒ split the step.
10. Stop and ask when a test would otherwise have to change semantically, or
    when the implementation deviates from this plan mid-step (update this file
    first, then report).

## Component breakdown

| Component | File | State moved in | Functions moved in |
|---|---|---|---|
| `HelpOverlayComponent` | `components/help.rs` | `show_help: bool`, `help_scroll: usize` | help branch of `on_key`, `draw_help` |
| `UninstallConfirmComponent` | `components/uninstall.rs` | `confirm_uninstall: Option<Vec<usize>>` | confirm branch of `on_key` (cancel path), `draw_confirm_uninstall` (takes `&[AppInfo]` as an argument) |
| `JobsPopupComponent` | `components/jobs.rs` | `jobs_popup: bool`, `jobs_scratch: usize` | jobs branch of `on_key`, the scratch→`max_parallel` math, `draw_jobs_popup` |
| `ListComponent` | `components/list.rs` | `apps`, `sel`, `picks`, `filter_attention`, `expanded` | `visible_rows`, `needs_attention`, `move_sel`, `clamp_selection`, `pick_one`, `pick_toggle`, `row_label`, `targets`, `draw_list`, `row_state`, `behind_*`, `pad`, `clip`. **Amended (step 4c):** `enter`/`enter_many` stay on `App` — they are task orchestration (launch, `try_start`, busy/queue math), not list state |
| `TaskManager` | `task/mod.rs` | `busy`, `progress`, `pending`, `failed`, `max_parallel` | `try_start`, `start_now`, `pump`, `is_staged`, `cancel_selected`, the uninstall loop, status-data prep, `handle_background(Ev)` |
| `DetailsComponent` | `components/details.rs` | `log`, `log_repo`, `log_len` | `refresh_log`, `draw_details`, `details_for`, `progress_line`, `tail_lines`, `short_dir` |
| `HeaderComponent` | `components/header.rs` | — (read-only) | header block build, `summary` |
| `KeyBarComponent` | `components/key_bar.rs` | — (read-only) | `key_bar` |

Stays on `App` (coordinator): `msg`, `quit`, `confirm_quit`, `track`,
`palette`, `scanning`, `tx`, `drain` (now delegating), `status_line`
(composing list + tasks), `request_quit`, `rescan_at`/`rescan_all`,
`toggle_track`, `on_key` (the router), `render` (layout + paint order).

## Refactor order

1. `HelpOverlayComponent` — smallest leaf, no data deps.
2. `UninstallConfirmComponent` — small; render takes `apps` as an argument.
3. `JobsPopupComponent` — small; `jobs_apply` output stays on `App`.
4. `ListComponent` — the big one; carries all row state.
5. `TaskManager` — feeds list + header; extracted after list exists to
   receive its events.
6. `DetailsComponent` — log cache + details pane.
7. `HeaderComponent` + `KeyBarComponent` — read-only views.
8. Delete `ui.rs`, remove `mod ui;`, final green sweep.

Overlays come first because they are leaves; list before tasks because tasks
consume the list's targets; `ui.rs` dies last.

## `src/components/mod.rs` (the real code, not a sketch)

```rust
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::engine::{Action, Ev};
use crate::theme::Palette;

pub struct AppContext<'a> {
    pub palette: &'a Palette,
}

#[derive(Debug)]
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayKind {
    Help,
    UninstallConfirm,
    JobsPopup,
}

pub trait Component {
    fn handle_key(&mut self, _key: KeyEvent) -> ComponentEvent {
        ComponentEvent::Ignored
    }
    fn handle_background(&mut self, _ev: &Ev) {}
    fn render(&mut self, _frame: &mut Frame, _area: Rect, _ctx: &AppContext) {}
}
```

## Justified deviations from the brief's sketch

1. **No `active_overlay: Option<OverlayKind>` on `App`.** Discovery proved the
   three overlay states are independent today (`show_help`,
   `confirm_uninstall`, `jobs_popup` can be simultaneously true; paint order
   and key precedence are fixed). `App::on_key` routes in the exact order
   Ctrl > `?` > help > jobs > uninstall-confirm > normal, and `App::render`
   paints help < uninstall < jobs. Each overlay component owns its own
   open/scroll/target state; `OverlayKind` is retained in `mod.rs` only as the
   payload of `OpenOverlay`/`CloseOverlay` events.
2. **`ComponentEvent` grows the variants shown above.** Every key path in the
   contract produces a message, a rescan, an uninstall, a jobs-popup open, or
   a quit request; the sketch's four variants cannot carry them.
3. **`AppContext` stays exactly `{ palette }`.** Components that need more
   take method arguments instead: `UninstallConfirmComponent::render` takes
   `&[AppInfo]`; list rendering receives the task state via arguments from
   `App::render`; header receives the composed status string. This follows the
   brief's own rule ("prefer method arguments over stuffing it into
   AppContext").
4. **The `Component` trait is used where it fits (overlays); richer inherent
   methods cover the data-heavy components.** `App::render` calls each
   component's concrete method with the arguments it actually needs — static
   dispatch, no `dyn`, per ground rule 3.

## Final `App`

```rust
pub struct App {
    list: ListComponent,
    details: DetailsComponent,
    header: HeaderComponent,
    key_bar: KeyBarComponent,
    help: HelpOverlayComponent,
    uninstall: UninstallConfirmComponent,
    jobs: JobsPopupComponent,
    tasks: TaskManager,
    msg: String,
    quit: bool,
    confirm_quit: bool,
    scanning: bool,
    palette: Palette,
    track: Track,
    tx: Sender<Ev>,
}
```

`App::on_key` keeps the exact routing order; each branch delegates to the
component and applies the returned `ComponentEvent` using the *existing*
method bodies (`start_action`, `enter`, `request_quit`, …), kept largely intact
on `App`/`TaskManager` so message strings and edge cases do not drift.
`App::render` keeps the min-size gate, the four-row layout, the header/list/
details/key-bar paint, and the three overlays in today's order. `main.rs`
changes: drop `mod ui;`, replace `ui::draw(f, app)` with `app.render(f)`.

## Per-component execution checklist

- [ ] Extract into the component file; `App` fields become the component's.
- [ ] `cargo build` green.
- [ ] `cargo clippy --all-features --all-targets -- -D warnings` clean (the
      baseline is 0 warnings, so strict mode applies from step one).
- [ ] `cargo test` 101/101 (tests moved with the component, assertions
      unchanged).
- [ ] `python3 scripts/tui_smoke.py` exit 0.
- [ ] Diff the touched surface against `BEHAVIOR_CONTRACT.md`.
- [ ] Commit `refactor: extract <Name>Component`.
- [ ] Net change > 200 lines ⇒ split the step before committing.
- [ ] Deviation from this plan ⇒ update this file, then report and stop.

## Final sweep (step 8)

- Delete `src/ui.rs`, remove `mod ui;` from `main.rs`.
- Full verification: fmt, clippy `-D warnings`, 101 tests, smoke exit 0.
- Update `REFACTOR_PLAN.md` checkboxes; report the finished state.

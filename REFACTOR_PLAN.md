# Refactor Plan — Component Architecture

Split the `App`/`ui.rs` monolith into one file per component. Behavior is
frozen by `BEHAVIOR_CONTRACT.md`; each step was diffed against it.

## Ground rules (from the brief, restated)

1. Behavior preservation is non-negotiable. Preserve the quirks listed in the
   contract (help/`?`/Ctrl precedence, End/G gap in help, uninstall `y`
   fall-through, `confirm_quit` clearing rules, overlay paint order).
2. No logic changes to `engine.rs` or `theme.rs`. Visibility changes only.
3. No new dependencies. No `dyn Component`, no trait-object registry, no
   async, no gratuitous generics — concrete structs, static dispatch.
4. One component per file, under `src/components/`; the task manager lives in
   `src/task/mod.rs`.
5. Components receive cross-component data through explicit arguments or view
   structs, not a growing app-wide context.
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
| `TaskManager` | `task/mod.rs` | `busy`, `progress`, `pending`, `failed`, `max_parallel` (+ `DEFAULT_MAX_PARALLEL`/`MIN_JOBS`/`MAX_JOBS`, re-exported from `app`) | `is_staged`, `has_slot`, `finish` (Done-event bookkeeping). **Amended (step 5):** `try_start`/`start_now`/`pump` stay on `App` — they need the list's clone dirs, `track` and the event sender; they delegate to the TaskManager helpers. `cancel_selected` stays on `App` (needs list targets). `App` exposes the maps as `app.tasks.*`; thin `App::is_staged` wrapper kept |
| `DetailsComponent` | `components/details.rs` | `log`, `log_repo`, `log_len` | `refresh_log`, `draw_details`, `details_for`, `progress_line`, `tail_lines`, `short_dir`. **Amended (step 6):** `render` takes `Option<&AppInfo>` + `&TaskManager` + `Track` (data as args); `App` keeps a thin `refresh_log` wrapper that picks the selected repo |
| `HeaderComponent` | `components/header.rs` | — (read-only) | header block build, `summary`. **Amended (step 7):** render takes a `HeaderView` struct (`status`/`tone`/`apps`/`filter_attention`/`track`/`tasks`); `App` keeps `status_line` |
| `KeyBarComponent` | `components/keybar.rs` | — (read-only) | `key_bar` as `lines(s)` + `render`. **Amended (step 7):** file named `keybar.rs` to match sibling module naming |

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
8. **Complete:** deleted `src/ui.rs`; moved the layout shell and remaining
   full-frame tests into `App::render`/`app.rs`; changed `main.rs` to call
   `app.render(f)` and removed `mod ui;`.

Overlays come first because they are leaves; list before tasks because tasks
consume the list's targets; `ui.rs` was deleted after its consumers moved.

## Component interface

`Component` provides the `handle_key` seam used by help, jobs, and uninstall
confirmation. Its event enum contains only outcomes those handlers return
(`Ignored`, `Msg`, `JobsApply`, and `CloseOverlay`). `App` retains overall key
routing and task orchestration. Rendering uses concrete methods and explicit
data arguments/view structs rather than a shared render context.

## Justified deviations from the brief's sketch

1. **No `active_overlay` on `App`.** Discovery proved the
   three overlay states are independent today (`show_help`,
   `confirm_uninstall`, `jobs_popup` can be simultaneously true; paint order
   and key precedence are fixed). `App::on_key` routes in the exact order
   Ctrl > `?` > help > jobs > uninstall-confirm > normal, and `App::render`
   paints help < uninstall < jobs. Each overlay component owns its own
   open/scroll/target state.
2. **`ComponentEvent` is intentionally narrow.** It carries only outcomes
   returned by component-owned key handling; `App` keeps the remaining key
   actions and orchestration.
3. **Data is passed explicitly.** Uninstall rendering takes `&[AppInfo]`;
   list rendering takes task state; header rendering receives a `HeaderView`;
   details rendering receives the selected app and task state.
4. **The trait covers key handling only.** Component-specific render methods
   have distinct data requirements, so `App::render` calls their concrete
   methods directly — static dispatch, no `dyn`, per ground rule 3.

## Final `App`

```rust
pub struct App {
    list: ListComponent,
    details: DetailsComponent,
    header: HeaderComponent,
    keybar: KeyBarComponent,
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

`App::on_key` keeps the exact routing order; component-owned key state
delegates through `Component::handle_key`, while task actions and app-wide
orchestration remain on `App` so message strings and edge cases do not drift.
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

## Final sweep (completed)

- `src/ui.rs` was deleted and `mod ui;` removed from `main.rs`.
- Full verification passed: fmt, clippy `-D warnings`, 114 tests, smoke exit 0.

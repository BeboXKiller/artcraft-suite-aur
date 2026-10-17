# Phase 0 — Discovery Report

Read-only survey of the whole `src/` tree (6,502 lines), plus `Cargo.toml`, git
state, and the test/smoke harnesses, before touching any code. Findings that
deviate from the refactor brief's working hypotheses are marked **Deviation**.

## 1. Actual file tree

```
src/
├── main.rs      312 lines   CLI parsing (args/env), TTY gate, event loop, --status
├── app.rs      1452         App struct, key dispatcher, task orchestration, scan_pipeline
├──engine.rs   2878         domain: suite registry, git, build, install, Progress, Ev
├── theme.rs     284         Depth/Tint/Palette/Sty (palette + style builder)
└── ui.rs       1576         draw() + all widget fns + ~800 lines of render tests
```

Supporting files: `scripts/tui_smoke.py` (pty end-to-end harness), `aur/PKGBUILD`,
`README.md`. Dependencies: **ratatui 0.30.2 + crossterm 0.29.0 only**. Single
binary crate; modules are declared in `main.rs` (there is no lib.rs). 101 tests
in total. Working tree was clean on `master` at survey time.

## 2. `App` struct inventory (app.rs:58-101)

| Field | Type | Purpose |
|---|---|---|
| `apps` | `Vec<AppInfo>` | suite rows, fixed order |
| `sel` | `usize` | highlighted row (index into `apps`) |
| `picks` | `HashSet<usize>` | Space/Ctrl+Space picks; empty ⇒ act on `sel` |
| `busy` | `HashMap<String,String>` | repo → verb ("installing"…) |
| `progress` | `HashMap<String, engine::Progress>` | live build progress per repo |
| `pending` | `VecDeque<(String, Action)>` | queue beyond `max_parallel` |
| `failed` | `HashMap<String,String>` | repo → last error, until retry |
| `filter_attention` | `bool` | `o` |
| `track` | `Track` | commits vs releases; `t` |
| `expanded` | `HashSet<usize>` | rows with open description |
| `msg` | `String` | status line text |
| `quit` | `bool` | loop exit flag (read by main.rs) |
| `show_help` | `bool` | `?` overlay |
| `help_scroll` | `usize` | help scroll offset |
| `palette` | `Palette` | detected once |
| `confirm_quit` *(private)* | `bool` | second `q` while tasks run |
| `confirm_uninstall` | `Option<Vec<usize>>` | pending `d` targets |
| `max_parallel` | `usize` | jobs limit (1-16) |
| `jobs_popup` / `jobs_scratch` | `bool` / `usize` | `p` popup + its editor value |
| `log` / `log_repo` / `log_len` *(last two private)* | `Vec<String>` / `&'static str` / `u64` | details-pane log tail cache |
| `scanning` *(private)* | `bool` | first-scan vs refresh, for the Checked message |
| `tx` *(private)* | `Sender<Ev>` | background channel |

Methods that are really orchestration and live on `App` alongside the state:
`drain`, `refresh_log`, `status_line`, `try_start`, `start_now`, `pump`,
`rescan_at`, `rescan_all`, `on_key`, `toggle_track`, `visible_rows`,
`needs_attention`, `move_sel`, `clamp_selection`, `request_quit`, `pick_one`,
`pick_toggle`, `row_label`, `targets`, `is_staged`, `cancel_selected`,
`jobs_apply`, `uninstall_selected`, `run_uninstall`, `enter`, `enter_many`,
`start_action`. The free function `scan_pipeline` also lives in app.rs.

## 3. Key-handling inventory

Dispatcher order in `App::on_key` **is** the semantics; every rule below must
be preserved verbatim.

1. **Ctrl chords (always first, even over open overlays):** Ctrl+C / Ctrl+D →
   `quit = true`; Ctrl+Space → `pick_toggle()` (multi-pick add/remove); any
   other Ctrl chord → ignored. All of them clear `confirm_quit`.
2. **`?` (second, before every overlay branch):** toggles `show_help`; opening
   the help resets `help_scroll = 0`; also clears `confirm_quit`.
3. **Help open:** `Esc`/`q`/`Enter` → close; `Up`/`k` scroll −1; `Down`/`j` +1;
   `PageUp`/`PageDown` ∓10; `Home`/`g` → 0. Everything else is ignored.
   Quirk: `End`/`G` are NOT handled in the help branch.
4. **Jobs popup open:** `Up`/`+` scratch+1 (≤16); `Down`/`−` scratch−1 (≥1);
   `Enter` → `jobs_apply()` (sets `max_parallel`, closes, message, `pump()`);
   `Esc` → close + message "parallel builds unchanged". Everything else ignored.
5. **Uninstall confirm pending and key ≠ `y`:** cancel (`confirm_uninstall =
   None`, message "uninstall cancelled", clear `confirm_quit`), return. The `y`
   key falls through to the normal-mode match.
6. **Normal mode, any key that is not `q`/`Esc`:** clears `confirm_quit`.
7. **Normal-mode match:**
   - `q`/`Esc` → `request_quit()` (first press with tasks running arms
     `confirm_quit` + warning message; a second press quits; no tasks → quit).
   - `Up`/`k`, `Down`/`j` → move ±1 within the visible rows; `PageUp`/`PageDown`
     ±10; `Home`/`g` → first visible row; `End`/`G` → last visible row.
   - `Right`/`l` expand the description; `Left`/`h` collapse it.
   - `Enter` → `enter()` (single target: launch if installed / install if
     missing / busy-queue messages; multi-pick: install every missing target,
     never launch).
   - `Space` → `pick_one()` (replace the pick with `sel`; toggle off if `sel`
     is the sole pick).
   - `u`/`i`/`f` → `start_action(Update|Install|Fetch)` on picks-or-`sel`.
   - `x`/`Delete` → `cancel_selected()` (kill a busy task via `engine::cancel`,
     dequeue a pending one, or report "idle"/"finished" cases).
   - `d` → `uninstall_selected()` (eligible = installed ∧ not staged; sets the
     confirm state + prompt message).
   - `p` → open the jobs popup with `scratch = max_parallel`.
   - `y` → `run_uninstall()` if a confirm is pending.
   - `U` → try_start Update on every cloned repo + "update queued" /
     "nothing to update".
   - `a` → try_start Install on every not-installed repo + message.
   - `r` → `rescan_all()` + "refreshing…".
   - `o` → toggle the attention filter, `clamp_selection`, message ("filter
     cleared" / "filter: needs attention (n shown)" / "✓ nothing needs
     attention").
   - `t` → flip `track`, `rescan_all()`, message "tracking releases|commits —
     re-checking…".
   - Anything else → ignored.

## 4. Rendering inventory (ui.rs)

- `draw(frame, app)` — min-size gate (60×15, polite message); vertical layout
  `[Length(4), Min(6), Length(7), Length(3)]`; header paragraph (title block +
  right-aligned summary spans + status line colored by tone→tint); `draw_list`;
  `draw_details`; centered `key_bar` paragraph; then overlays painted in this
  order: `draw_help` (if `show_help`) → `draw_confirm_uninstall` (if `Some`) →
  `draw_jobs_popup` (if open). Later overlays paint on top.
- `summary(app, s)` — right-aligned counters: `✓ up`, `↑ behind`, `· missing`,
  `? unchecked` (only when > 0), an `attention` badge when the filter is on, a
  `⚑ releases` badge on the releases track, and `⚙ n/max` while builds run.
  Reads `apps`, `filter_attention`, `track`, `busy`, `max_parallel`.
- `draw_list` — responsive columns (mark 3 + glyph 6 fixed, phrase reserve 22,
  repo + version share the slack; the version column hides below 52 usable
  cols); `row_state` per row; pick marks `■`/`□`; expanded rows push an
  indented description line; title ` Apps (n) · k picked · attention only `;
  `highlight_style(hl())`, selection symbol `▶ `.
- `row_state(app, a)` — busy → `◐ verb…` Accent; pending → `◌ queued`; failed →
  `✗ build failed`; installed+!cloned → `✓ installed`; behind>0 → `↑n` + "n
  commit(s|release(s)) behind" Warn; stale → `↻ update ready` Warn; behind 0 →
  `✓ up to date`; behind None → `? not checked`; cloned → `○ cloned, not
  built`; else `· not installed`.
- `details_for` — busy → title `verb — repo` + progress line + live log tail;
  failed → error line + log + hint; otherwise the state match (behind>0 "What's
  new" with commits; stale "update ready"; installed-without-clone;
  not-checked; up-to-date; cloned-not-built; not-installed), always ending in
  the footer `~/path · in app menu|no menu entry`. Helpers: `progress_line`
  (bar + "pct% · ~eta left · n compiled", or "building… ~elapsed" before a
  denominator exists), `tail_lines`, `short_dir`, `pad`, `clip`, `behind_glyph`,
  `behind_word`.
- `key_bar(s)` — three static hint lines (fitted to 80 cols; asserted by tests).
- `draw_help` — full-width centered `Clear` + `Paragraph`, scroll offset
  clamped, title " Help — j/k scroll · ? or Esc to close ".
- `draw_confirm_uninstall` — 40-col centered card, up to 5 names + "+n more",
  warning lines, "y confirms · any other key cancels".
- `draw_jobs_popup` — 50-col centered card, the scratch value, "Low-spec PC?
  Stick to 1–2.", "↑↓ or +/- change · Enter applies · Esc cancels".

## 5. Background-event inventory

`engine::Ev` variants and their state effects:

| Variant | Effect |
|---|---|
| `Done { repo, action, result }` | clear busy + progress for the repo; insert/remove `failed`; set the status message; `rescan_at` that row; `pump()` |
| `Progress { repo, progress }` | insert into `progress` |
| `Scanned { index, info }` | replace `apps[index]` (repo-guarded) |
| `Loaded(Vec<AppInfo>)` | replace all rows, `clamp_selection()`, message "checking N repo(s) for updates…" |
| `Checked` | message "ready" the first time (flips `scanning`), else "refreshed" |

`drain()` always ends with `clamp_selection()`. `refresh_log()` runs every loop
tick and re-reads the selected repo's log tail only when the file's stat
signature changed. Producers: `scan_pipeline` (app.rs:850) and
`engine::spawn`.

## 6. Deviations vs the refactor brief's hypotheses

1. **app.rs is ~60% orchestration, not just "state + keys".** Task queue
   (`try_start`/`pump`/`is_staged`), status composition, log caching, and
   `scan_pipeline` all live there. The refactor must split both ui.rs **and**
   app.rs.
2. **ui.rs is not a pile of unrelated draw functions.** It is one entry point
   `draw()` plus a tight helper graph, **and ~800 lines of render tests** (30
   tests) that construct `App` via `App::with_apps`, poke its `pub` fields, and
   call `ui::draw(f, &app)` through a `TestBackend`. This is the single largest
   refactor risk.
3. **Overlay states are NOT mutually exclusive** (contradicting the brief's
   proposed `active_overlay: Option<OverlayKind>`). `show_help`,
   `confirm_uninstall: Option<Vec<usize>>`, and `jobs_popup` are three
   independent states; key precedence is Ctrl > `?`-toggle > help > jobs >
   uninstall-confirm > normal; paint order is help < uninstall < jobs. A single
   `Option<OverlayKind>` would change behavior (e.g. `?` while a confirm is
   pending currently opens help over the still-armed confirm).
4. **`ComponentEvent` as sketched in the brief is too small.** Every key path
   today sets `self.msg`; `r`/`t` trigger rescans; `d`/`y` open/run
   uninstalls; `p` opens the jobs popup; `q` needs a two-step quit
   confirmation that reads task counts. The event enum must grow.
5. **engine.rs already provides `Action` and `Ev`** — the brief's names map
   onto them cleanly; no rename needed.
6. **theme.rs also ships `Sty`** (a per-frame style builder), not just
   `Palette`.
7. **main.rs owns the loop** (`drain` → `refresh_log` → `ui::draw` → poll →
   `on_key` → check `quit`). Only two call sites may adapt: `ui::draw(f, app)`
   → `app.render(f)`, and dropping `mod ui;`.
8. **`App::with_apps` (cfg(test)) plus every `pub` field form a de-facto test
   API.** Five app.rs tests also drive `on_key` directly.
9. **No existing test pins simultaneous overlay states** (verified by grep) —
   keeping them independent is about preserving the code path, not a
   test-forced constraint.

## 7. Risks

- **Test coupling (highest risk).** ~35 tests construct `App` and set fields
  directly; ~30 render through `ui::draw`. Moving fields into components breaks
  construction and the rendering entry point. Resolution adopted (decision 1A):
  adapt tests in place — same assertions, new construction boundary; each
  component's tests move into its own file.
- **Cross-component reads are the norm, not the exception:** `row_state` (list)
  needs busy/pending/failed (tasks); `summary`/status (header) need
  apps+busy+pending+progress+filter+track+max_parallel; details needs
  apps+tasks+log. Data must flow as method arguments — no shared context
  object; components get inherent `render` methods with explicit args, and the
  component trait is limited to key dispatch (final state; see §10). Justified
  deviation (decision 2).
- **Order-sensitive quirks** (help/`?`/Ctrl precedence, the End/G gap in help,
  the uninstall-`y` fall-through, `confirm_quit` clearing rules) must be
  replicated exactly.
- **`refresh_log`'s stat gate** and `clamp_selection` timing (after every
  drain, after filter toggle, after uninstall) are subtle and must survive the
  move.
- `Progress`/`Tone`/`coarse_eta` are shared vocabulary between app.rs and
  ui.rs; relocation must not fork them.

## 8. Verification baseline (pre-refactor)

- `cargo fmt --check` clean
- `cargo clippy --all-features --all-targets` 0 warnings
- `cargo test` 101/101
- `python3 scripts/tui_smoke.py` exit 0

## 9. Phase 2 progress log (refactor execution)

Steps land one commit each, checklist green every time (`cargo fmt`,
`cargo clippy --all-features --all-targets -- -D warnings`, `cargo test`,
`scripts/tui_smoke.py` exit 0, behavior-contract diff).

| Step | Commit | What moved | Tests |
|---|---|---|---|
| 1 | `6e7ef3e` | `HelpOverlayComponent` (`components/help.rs`): open/scroll/toggle/handle_key/render; free `render_help_card` called from ui.rs during the transition | 5 moved |
| 2 | `e401b43` | `UninstallConfirmComponent` (`components/uninstall.rs`): targets, pending, open, cancel rule, render | 3 moved |
| 3 | `a6ed099` | `JobsPopupComponent` (`components/jobs.rs`): open/scratch/open_with/limit/handle_key/render; `ComponentEvent::JobsApply` added | 4 moved |
| 4a | `5d9f91a` | `ListComponent` state + row math (`components/list.rs`); App field collapse to `list: ListComponent` | 6 added |
| 4b | `df33a11` | List rendering: `render(TaskView)`, `row_state`, `behind_*`, `pad`, `clip` out of ui.rs | 1 moved |
| 4c | `94ade44` | `pick_one`/`pick_toggle` into ListComponent returning `Msg`; plan amended (`enter*` stays on App) | 2 added |
| 5 | `bd24630` | `TaskManager` (`task/mod.rs`): `busy`/`progress`/`pending`/`failed`/`max_parallel` + `is_staged`/`has_slot`/`finish`; job consts move here (re-exported from app). `try_start`/`start_now`/`pump` stay on App, delegating (need dirs/track/tx) — plan amended | 4 added; 113 total |
| 8 | (this commit) | `src/ui.rs` deleted: layout shell + 20 remaining full-frame tests move into `App::render`/`app.rs`; `main.rs` calls `app.render(f)`; component test helpers switch to `app.render`. | 114 total |
| 7 | `a19c6f2` | `HeaderComponent` (`components/header.rs`): header block + `summary` counters, fed by a `HeaderView` struct; `KeyBarComponent` (`components/keybar.rs`): `lines` + centered `render`. `App` keeps `status_line`. 4 tests moved (3 header + 1 key bar) — plan amended (file name `keybar.rs`, `HeaderView` data-as-struct) | 114 total |
| 6 | `01eb55c` | `DetailsComponent` (`components/details.rs`): log cache (`refresh_log`), `draw_details`/`details_for`/`progress_line`/`tail_lines`/`short_dir`, `DETAIL_LINES`/`LOG_BYTES` out of ui.rs/app.rs; 7 tests moved + 1 new (`short_dir`) — plan amended (render takes `Option<&AppInfo>` + `&TaskManager` + `Track`) | 8; 114 total |

Notes: tests grew 101 → 113 with net-zero assertion change (unit tests for the
new home of each pure function). ui.rs is still the layout coordinator;
`App::render` and deletion of `ui.rs` are the final step (8).

## 10. Phase 2 final state

All eight steps landed on `refactor/component-arch`, one commit each, with
`cargo fmt`, `cargo clippy --all-features --all-targets -- -D warnings`,
`cargo test`, and `scripts/tui_smoke.py` green after every step:

```
0a8ae2c (master) … ETA/stale-install work, then:
5a748cf docs: behavior contract and refactor plan
6e7ef3e HelpOverlayComponent
e401b43 UninstallConfirmComponent
a6ed099 JobsPopupComponent
5d9f91a ListComponent state + row math
df33a11 list rendering
94ade44 pick_one/pick_toggle into ListComponent
bd24630 TaskManager
01eb55c DetailsComponent
a19c6f2 HeaderComponent + KeyBarComponent
(this)      ui.rs deleted; App::render
```

- `src/ui.rs` is gone; `App::render` owns the frame layout and paint order.
- Components live in `src/components/{help,uninstall,jobs,list,details,header,keybar}.rs`
  with a small shared key-dispatch interface (`Component`,
  `ComponentEvent`); task state in `src/task/mod.rs`.
- Tests: 115 (from 101) — net assertion change is zero; the growth is unit
  tests pinning each pure function in its new home (row math, queue
  bookkeeping, `short_dir`) and the valid `TaskManager::default()` limit.
- Behavior: unchanged throughout — the contract's quirks (key precedence,
  paint order, End/G gap, uninstall fall-through, two-step quit) are intact,
  verified by the moved tests and the pty smoke suite every step.
- Recorded deviations (all in `REFACTOR_PLAN.md`): independent overlay states
  instead of one active overlay; data passed as args/view structs;
  `enter*`/`try_start`/`pump`/`status_line`/`cancel_selected` stay on `App`
  (orchestration); `TaskView`/`HeaderView`/details args structs; `keybar.rs`
  file name; the component trait is limited to the live key-dispatch seam.

# Behavior Contract

Every entry here is current production behavior and **must** be preserved by the
component refactor. When a step in `REFACTOR_PLAN.md` is finished, its changed
surface is diffed against this document. If a change to this contract is ever
required, that is a deliberate behavior change: stop, update this file, get
approval, then proceed.

## 1. Keybindings by context

### 1.1 Ctrl chords (always processed first, even over open overlays)

| Key | Behavior |
|---|---|
| Ctrl+C, Ctrl+D | `quit = true` immediately |
| Ctrl+Space | `pick_toggle()` — add/remove the current row in the multi-pick set |
| any other Ctrl chord | ignored |

All Ctrl chords clear `confirm_quit` before returning.

### 1.2 `?` (processed second, before every overlay branch)

Toggles `show_help`. Opening resets `help_scroll = 0`. Clears `confirm_quit`.

### 1.3 Help open (after `?` and Ctrl have had their chance)

| Key | Behavior |
|---|---|
| Esc, q, Enter | close help |
| Up, k | scroll −1 |
| Down, j | scroll +1 |
| PageUp, PageDown | scroll ∓10 |
| Home, g | scroll = 0 |
| anything else | ignored |

Quirk to preserve: `End`/`G` are **not** handled inside the help branch (they
only work in normal mode).

### 1.4 Jobs popup open

| Key | Behavior |
|---|---|
| Up, + | scratch+1 (capped at 16) |
| Down, − | scratch−1 (floor 1) |
| Enter | apply: set `max_parallel = scratch`, close, message, `pump()` |
| Esc | close, message "parallel builds unchanged" |
| anything else | ignored |

### 1.5 Uninstall confirm pending (and key ≠ `y`)

Any key except `y` cancels: `confirm_uninstall = None`, message
"uninstall cancelled", clear `confirm_quit`, return. `y` falls through to the
normal-mode match and runs `run_uninstall()`.

### 1.6 Normal mode

Any key that is not `q`/`Esc` clears `confirm_quit`.

| Key | Behavior |
|---|---|
| q, Esc | `request_quit()` — first press with tasks running arms `confirm_quit` + warning message; second press quits; no tasks → quit immediately |
| Up, k | move selection −1 within the visible rows |
| Down, j | move selection +1 within the visible rows |
| PageUp, PageDown | move ±10 (clamped to visible rows) |
| Home, g | first visible row |
| End, G | last visible row |
| Right, l | expand the selected row's description |
| Left, h | collapse the selected row's description |
| Enter | `enter()` — single target: launch if installed / install if missing / busy-queue messages; multi-pick: install every missing target, never launch |
| Space | `pick_one()` — replace the pick with `sel`; toggle off if `sel` is the sole pick |
| u | `start_action(Update)` on picks-or-`sel` |
| i | `start_action(Install)` on picks-or-`sel` |
| f | `start_action(Fetch)` on picks-or-`sel` |
| x, Delete | `cancel_selected()` — kill a busy task (`engine::cancel`), dequeue a pending one, or report the idle/finished cases |
| d | `uninstall_selected()` — eligible = installed ∧ not staged; sets `confirm_uninstall` + prompt message |
| p | open the jobs popup with `scratch = max_parallel` |
| y | `run_uninstall()` if a confirm is pending |
| U | try_start Update on every cloned repo + "update queued" / "nothing to update" |
| a | try_start Install on every not-installed repo + message |
| r | `rescan_all()` + "refreshing…" |
| o | toggle `filter_attention`, `clamp_selection`, message: "filter cleared" / "filter: needs attention (n shown)" / "✓ nothing needs attention" |
| t | flip `track` (commits ↔ releases), `rescan_all()`, message "tracking releases\|commits — re-checking…" |
| anything else | ignored |

## 2. Visual elements that must remain

### 2.1 Frame layout

- Minimum size 60×15; below that a polite "too small" message replaces the UI.
- Vertical layout: header 3 rows / list `Min(6)` / details 7 rows / key bar 3
  rows.
- Overlay paint order: help < uninstall-confirm < jobs-popup (later = on top).

### 2.2 Header

- Border title " ArtCraft Suite ", one status line inside, right-aligned
  summary counters on the top border (no duplicate inner title line).
- Summary counters: `✓ up`, `↑ behind`, `· missing`; `? unchecked` only when >
  0; `attention` badge while the filter is on; `⚑ releases` badge on the
  releases track; `⚙ n/max` only while builds run.
- Status line text colored by tone→tint (message tone mapping preserved).

### 2.3 List

- Title: ` Apps (n) · k picked · attention only · /query ` (suffix parts
  conditional; a `▌` marks the search line while it has the keyboard).
- Search: `/` focuses the search line; typing filters rows by repo name
  (case-insensitive), Backspace edits, Enter keeps the filter, Esc clears it —
  while typing and, with a standing filter, in normal mode too.
- Columns: mark (3) + glyph (6) fixed, phrase reserve 22, repo + version share
  the remaining width; version column hides below 52 usable columns.
- A faint column-header row (`app` / `version` / `state`) is pinned under the
  title inside the block — it never scrolls with the rows.
- Pick marks: `■` picked, `□` not picked.
- Highlight: `▶ ` symbol, accent background, dark-ink-on-accent foreground.
- Expanded rows push an indented description line below the row.
- Every `row_state` glyph/phrase pair:
  busy → `◐ verb…`; pending → `◌ queued`; failed → `✗ build failed`;
  installed+!cloned → `✓ installed`; behind>0 → `↑n` + "n commit(s|release(s))
  behind"; stale → `↻ update ready`; behind 0 → `✓ up to date`; behind None →
  `? not checked`; cloned → `○ cloned, not built`; else `· not installed`.

### 2.4 Details pane

- Busy: title `verb — repo`, progress line, live log tail.
- Failed: error line, log, hint.
- State match: behind>0 → "What's new" with the commit list; stale → "update
  ready"; installed-without-clone; not-checked; up-to-date; cloned-not-built;
  not-installed.
- Footer: `~/path · in app menu|no menu entry`.
- Progress line: bar + "pct% · ~eta left · n compiled"; before a denominator
  exists: "building… ~elapsed".

### 2.5 Key bar and overlays

- Key bar: three static hint lines fitted to 80 columns.
- Help card: full-width centered, title " Help — j/k scroll · ? or Esc to
  close ", scroll offset clamped to the content.
- Uninstall confirm: 40-column centered card, up to 5 names + "+n more",
  warning lines, "y confirms · any other key cancels".
- Jobs popup: 50-column centered card, the scratch value, "Low-spec PC? Stick
  to 1–2.", "↑↓ or +/- change · Enter applies · Esc cancels".
- With `NO_COLOR=1` / the mono palette: no colors anywhere, reverse-video
  highlight instead of accent colors.

## 3. Background-event state changes

| Event | State change |
|---|---|
| `Done { repo, action, result }` | clear busy + progress; insert/remove `failed`; set message; `rescan_at` that row; `pump()` |
| `Progress { repo, progress }` | insert into `progress` |
| `Scanned { index, info }` | replace `apps[index]` (repo-guarded) |
| `Loaded(apps)` | replace all rows, `clamp_selection()`, message "checking N repo(s) for updates…" |
| `Checked` | message "ready" the first time (flips `scanning`), else "refreshed" |

- `drain()` ends with `clamp_selection()` every time.
- `refresh_log()` runs every loop tick, stat-gated.
- Desktop notifications fire from `engine::spawn` for Install/Update outcomes
  (success, failure, "up to date"); Fetch stays quiet. Engine-internal; do not
  touch.

## 4. Quit and safety behaviors

- Two-step quit with a task-count warning ("…they keep running") when `q` is
  pressed while builds are active.
- Ctrl+C / Ctrl+D quit immediately from any context, including open overlays.
- Uninstall is two-step: `d` arms a confirm; `y` runs it; any other key
  cancels with the "uninstall cancelled" message.
- Jobs popup `Esc` closes without applying the scratch value.
- Cancel (`x`) reports the single / multi / idle / finished cases distinctly.
- `try_start` never double-queues a repo that is already busy or pending.

## 5. Verification tools

- `cargo test` — 101 tests, including the render tests that assert the exact
  strings listed above.
- `python3 scripts/tui_smoke.py` — pty end-to-end: boots the real binary,
  sends keys, checks rendered frames; exit code must stay 0.
- Manual checklist (appendix): colors at each palette depth; light-theme
  contrast; resize behavior around the responsive-column threshold.

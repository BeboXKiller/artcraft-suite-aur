# artcraft-suite-aur

Terminal UI to install, update and launch the Craft App Suite from source — clone,
build, install and launch each app without leaving the keyboard.

## Requirements

| | |
|---|---|
| OS | Unix — process groups, signals and `kill` (Linux recommended) |
| Interactive mode | a real TTY, UTF-8, at least **60x15** columns |
| Tools on `PATH` | `git`, `cargo` |
| Network | access to `github.com` (clone/pull/fetch) |
| Non-interactive mode | none — `--status` works anywhere, including pipes and CI |

Anything beyond that is an enhancement: the brand palette is richest where the
terminal allows it — 24-bit colour, then xterm-256, then the 16 ANSI colours,
then monochrome under `NO_COLOR` — and the layout degrades gracefully down to
60 columns.

## Installation

```sh
cargo build --release
install -Dm755 target/release/artcraft-suite-aur ~/.local/bin/artcraft-suite-aur
```

or straight into Cargo's bin directory:

```sh
cargo install --path .
```

## Quick start

```sh
artcraft-suite-aur             # interactive UI (requires a terminal)
artcraft-suite-aur --status    # one line per app: repo|state|pkg|version|dir
artcraft-suite-aur --version
artcraft-suite-aur --help
```

If stdin or stdout is not a terminal (a pipe, `less`, CI), the app never enters the
full-screen UI: it prints the `--status` format and a hint on stderr, then exits 0.

### Layout

```text
┌ ArtCraft Suite ───────────────────────────────────────────── ✓ 6  ↑ 3  · 2 ┐
│ ArtCraft Suite — install · update · launch the Craft suite                 │
│ ✓ 1 update ready — u updates wordcraft                                     │   ← status / errors
└────────────────────────────────────────────────────────────────────────────┘
┌ Apps (15) · 1 picked ──────────────────────────────────────────────────────┐
│▶  ■  ✓ photocraft      0.2.0       up to date                              │   ← picked + selected
│   □  ↑3 wordcraft       0.1.42      3 commits behind                       │
│   □  ◐ deckcraft       0.3.1       updating…                               │
│   □  · cadcraft        -           not installed                           │
└────────────────────────────────────────────────────────────────────────────┘
┌ What's new in wordcraft ───────────────────────────────────────────────────┐
│ 0.1.42 · 3 commits to pull                                                 │   ← the update's contents
│ 9f8e7d6 fix .docx table borders                                            │
│ f refreshes · u updates                                                    │
└────────────────────────────────────────────────────────────────────────────┘
 [Space] pick  [Ctrl+Space] pick more  [Enter] open  [u] update  [o] attention
 [j/k] move  [U] update all  [f] fetch  [x] cancel  [?] help  [q] quit
```

Every row states its situation twice — a glyph and the same fact in words
(`✓ up to date`, `↑3 3 commits behind`, `◐ updating…`, `✗ build failed`) — so
nothing depends on colour alone. A picked row fills its square (`■`) in brand
blue — ink on the selected row so it stays visible on the accent bar — while
unpicked rows keep an empty `□`. The header counts the suite at a glance
(`✓ up to date · ↑ behind · · missing`), the status row is the feedback channel
(queued/running tasks, `✓ installed photocraft (0.2.0)`, `✗ photocraft ·
installing: build failed`), and the bottom pane answers "what would `u` even
change?" with the upstream commits waiting to be pulled — or the live build log
while a task runs. `?` opens the full key reference: every hint with a one-line
description of what it does, scrolled with `j`/`k` on short terminals.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `j` / `k`, `↑` / `↓` | move selection |
| `PgUp` / `PgDn` | jump 10 rows |
| `g` / `Home`, `G` / `End` | first / last app |
| `Space` | pick the highlighted row (press again on the only pick to clear) |
| `Ctrl+Space` | add / remove the highlighted row from the picked set |
| `Enter` | launch the selected app, or install it if missing; on a multi-pick, install every missing one |
| `u` / `i` / `f` | update / install / fetch — the picked rows, or the highlighted row when nothing is picked |
| `U` / `a` | update everything cloned / install everything missing |
| `o` | toggle **attention only** — hide the apps that are up to date |
| `x` / `Delete` | cancel the running task, or dequeue a queued one — again over the picked set |
| `r` | rescan all apps from disk (on a background thread) |
| `?` | toggle the help overlay |
| `q` / `Esc` | quit — asks a second time while tasks are running |
| `Ctrl+C` | quit immediately |

`Ctrl` chords never act as their bare letters: `Ctrl+A` will not start
"install all" by accident — only `Ctrl+Space` (pick) and `Ctrl+C`/`Ctrl+D`
(quit) do anything. Picking is a temporary selection: `u`, `i`, `f`, `x` and
`Enter` look at the picked rows first and fall back to the highlighted row when
nothing is picked, so the picks always win. `o` narrows the list to the
apps that need attention (missing, behind upstream, or failed) — the highlight
follows the filter, and the header badge says when it is on. At most 2 tasks
run at once; the rest queue up (shown as `◌` and in the status row).

## Theme

The palette is the ArtCraft brand — the same colour tokens as getartcraft.com
and the official launcher — assigned by meaning, never for decoration:

| Role | Colour (dark) | Used for |
|---|---|---|
| accent | `#2d81ff` | selection, running tasks, key hints |
| good | `#3fd6b8` (teal) | `✓ up to date`, success messages |
| warn | `#ffc14d` (amber) | `↑n commits behind`, update counts |
| bad | `#ff6259` (coral) | `✗ build failed`, error messages |
| muted / faint | warm greys | versions, hints, secondary text |

The richest tier the terminal supports is used automatically: 24-bit RGB when
`COLORTERM`/`TERM` advertise it, xterm-256 approximations otherwise, named ANSI
16 colours below that, and no colour at all under `NO_COLOR`, where bold and
reverse video still carry every state. Each state is also a glyph and a phrase
(`✓ up to date`, `↑3 commits behind`, `◐ updating…`, `✗ build failed`), so the
UI never depends on colour alone.

| Variable | Effect | Default |
|---|---|---|
| `ARTCRAFT_THEME` | `light` = darker ink for light terminal backgrounds | `dark` |

## Configuration

| Variable | Effect | Default |
|---|---|---|
| `ARTCRAFT_GIT_ORG` | GitHub org the suite is cloned from | `storytold` |
| `XDG_CACHE_HOME` | where build logs live | `~/.cache` |
| `HOME` | where repos are cloned, and `~/.local/bin` for installs | — |

Logs are written to `$XDG_CACHE_HOME/artcraft-suite-aur/<repo>.log` — git output
during clone/pull/fetch and cargo output during builds both land there (never on the
terminal, which the UI owns).

## Terminal compatibility

| Feature | Status |
|---|---|
| TTY, UTF-8, 60x15 | **required** for the UI; `--status`/`--help` need nothing |
| truecolor | brand RGB palette, when `COLORTERM`/`TERM` advertise it |
| 256-color | brand palette approximated from the xterm-256 palette |
| 16 colors | brand roles as Blue / Green / Yellow / Red |
| `NO_COLOR` | UI runs monochrome — bold and reverse video carry the states |
| `TERM=dumb` | UI not supported: it prints plain `--status` output instead |
| Resize | handled; below 60x15 you get a "terminal too small" message |
| Mouse | not used (keyboard-first) |
| BiDi / RTL (Arabic, Hebrew) | not implemented — layout is left-to-right |
| Wide chars (CJK) | fields are padded by column and truncated, so rows stay aligned |

## Troubleshooting

**"No terminal detected — printed status instead."**
The program was piped or run in CI. Run it inside a terminal, or use `--status`.

**The terminal is left in a mess** (raw mode, no cursor):

```sh
reset && stty sane
```

The app restores the terminal on normal exit and on panic; an uncatchable signal
(`kill -9`) is the case that can't be cleaned up.

**`build failed - see ~/.cache/artcraft-suite-aur/<repo>.log`**
Open the log — cargo's stderr is in there. Press `r` after fixing to rescan.

**`git pull failed (local changes?)`**
The repo has local modifications; the app only fast-forwards. Commit/stash first.

**"N task(s) still running — press q again to quit (they keep running)"**
Quitting does not kill background builds: their process groups are detached, so
they finish (or keep going) on their own. Press `q` again to leave, or `x` on the
running row to cancel first.

**An app window opened during a rescan**
Only if a suite binary ignores `--version`; probes are bounded by a 2-second timeout,
after which the version is shown as `-`.

## Development

```sh
cargo build           # build
cargo test            # unit + golden-frame tests (headless, no TTY needed)
cargo fmt --check     # formatting
cargo clippy --all-targets -- -D warnings   # lints
python3 scripts/tui_smoke.py                # end-to-end: real pty, real keys
```

Source layout — concerns are kept separate on purpose:

| File | Responsibility |
|---|---|
| `src/main.rs` | CLI flags, TTY detection, event loop, terminal init/restore |
| `src/app.rs` | state: selection, picks, task queue, filter, key → action mapping |
| `src/engine.rs` | domain: suite registry, git/cargo processes, logs, changelogs, install |
| `src/theme.rs` | brand palette + colour-depth detection (truecolor → 256 → 16 → mono) |
| `src/ui.rs` | rendering only: a pure function of state + terminal size |

### Testing

- **Golden frames** (`src/ui.rs`): render at fixed sizes with `TestBackend` and
  assert on what the user sees — header summary counts, status glyphs *and*
  their phrases (`✓ up to date`, `↑3 commits behind`), the changelog pane,
  the live-log handoff, the attention filter, picked squares, the key bar
  fitting 80 columns, help fitting 60x15, and the polite message below the
  minimum.
- **Palette** (`src/theme.rs`): every tier is asserted — monochrome yields no
  colour at all, truecolor paints the brand RGB, 256/16 fall back sensibly,
  and focus still shows via reverse video when colour is gone.
- **Interaction** (`src/app.rs`): scripted key sequences → assert state. Covers
  selection clamping, `Ctrl+C`, the help overlay swallowing keys, two-step quit,
  dequeueing, `Space`/`Ctrl+Space` picking, batch actions over the picked set
  (never the cursor row), `Enter` installing only the missing picks, the `o`
  attention filter (including clamping the highlight onto a visible row), failed
  tasks counting as attention, and status tone following the message prefix.
- **Engine** (`src/engine.rs`): children are wired to the log file (not the tty),
  `log_tail` survives invalid UTF-8, the version probe respects its timeout.
- **End-to-end** (`scripts/tui_smoke.py`): boots the real binary in a sized pty,
  sends real keys, replays the output through a minimal ANSI screen and checks
  alt-screen entry/exit, cursor restore, the summary counters, status phrases,
  the details pane, picking, the attention filter and the frames themselves.

## License

MIT OR Apache-2.0.

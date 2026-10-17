# artcraft-suite-aur

Terminal UI to install, update and launch the Craft App Suite from source — clone,
build, install and launch each app without leaving the keyboard.

## Features

- **The whole suite, from source** — 14 apps (PhotoCraft, FigCraft, WordCraft,
  FilmCraft, Craft Launcher, …): clone from GitHub, build with cargo, install to
  `~/.local/bin`, launch — all from one keyboard-driven list.
- **Auto-detection on launch** — the first frame appears immediately (the scan
  runs in the background) and the suite finds itself: clones are located up to
  three levels deep under `$HOME` or any `ARTCRAFT_REPO_ROOTS` directory (by
  name *or* by their git remote, so renamed clones and forks count),
  `.desktop` menu entries are parsed wherever they live (`~/.local/share` or
  the system data dirs), and installed binaries are recognised in
  `~/.local/bin`, `~/.cargo/bin`, `/usr/local/bin` and `/usr/bin` — so apps
  installed by the launcher or an AUR package show up as installed, with their
  real version. Every clone is then checked against its remote (a bounded
  background `git fetch`, `ARTCRAFT_FETCH=never` opts out) so "up to date"
  means the remote says so — not the cached refs, which only a fetch refreshes.
  Until a clone is checked (or if the fetch fails, e.g. offline) its row says
  `? not checked` instead of guessing. The details pane states where each clone
  lives and whether it is registered in the app menu; `r` re-runs discovery and
  the check at any time.
- **Batch actions over picks** — `Space` / `Ctrl+Space` pick any set of rows;
  `u` / `i` / `f` / `x` / `Enter` act on the picks (or the highlighted row when
  nothing is picked); `U` / `a` update / install everything.
- **Release tracking** — `--releases` / `ARTCRAFT_TRACK=releases` (or `t`
  live in the UI) measures "behind" in release tags instead of every commit,
  and `u` lands on the newest release tag (detached checkout — nothing is
  lost, a dirty tree fails loudly, and commits mode returns to the branch).
  For suites that ship tons of commits between releases, and people who want
  the stable releases only.
- **Controlled parallelism** — `-p N` / `--parallel N` / `ARTCRAFT_PARALLEL`
  cap how many apps compile at once (default 2, queue the rest as `◌`); `p`
  opens a popup to change it live; the header shows `⚙ n/limit` while builds
  run. Low-spec machines should stick to 1–2.
- **Desktop integration on every build** — the binary lands in `~/.local/bin`,
  the repo's `.desktop` entry and hicolor icons are registered in
  `~/.local/share/applications` and `~/.local/share/icons/hicolor`, caches
  refreshed — the app appears in your app menu immediately. A desktop
  notification (via `notify-send`) also fires when an install or update
  finishes — success, failure or "up to date" — so you can walk away from a
  long build; `ARTCRAFT_NOTIFY=never` silences it.
- **Uninstall** — `d` removes the binary, the desktop entry, the icons *and*
  the clone, behind a confirmation popup (`y` to confirm).
- **See before you update** — the bottom pane lists the upstream commits an
  update would pull (or the live build log while a task runs).
- **Honest "up to date"** — the install is judged against the clone it was
  built from: each build records its revision (older installs compare
  versions), so a pull whose build was cancelled or failed shows
  `↻ update ready` instead of claiming currency — and `u` rebuilds it
  rather than skipping.
- **Attention filter** — `o` hides everything that is fine (up to date); only
  missing, behind, not-checked, stale-install, or failed apps remain, with
  the header badge saying so.
- **Expandable app blurbs** — `→` / `←` open and close an indented one-line
  description (the official getartcraft.com blurb) under any row.
- **Safe by default** — cancel/dequeue with `x`, two-step quit while tasks
  run, background builds survive quitting, `r` rescans from disk at any time.
- **Clear feedback** — one status row for queued/running/success/error, every
  state as glyph *and* phrase, failed rows tracked until they succeed.
- **Measured build progress** — while a build runs, the bottom pane shows a
  real percentage weighted to work actually done (compiled crates against the
  units still unresolved), starting from 0% and advancing crate by crate, plus
  a `~`-ETA derived from the recent per-crate pace; a single running task also
  gets a compact `~2m · 54%` on the status row. Cached (`fresh`) crates count
  as resolved, never as progress — so an incremental rebuild's estimate stays
  in minutes instead of claiming hours. The denominator comes from a real
  number computed before the build: the exact unit count of the previous
  successful build (persisted per repo), or a `cargo tree` estimate on the
  first one. If that estimate runs out mid-build the bar holds at 99% with an
  `…` ETA rather than lying, and a build with no obtainable denominator shows
  elapsed time instead of a bar, never a guessed percentage.
- **Full key reference in-app** — `?` opens a scrollable overlay documenting
  every hint.
- **Brand theme, honest colour** — ArtCraft palette (the same tokens as
  getartcraft.com) auto-degraded through truecolor → 256 → 16 → mono
  (`NO_COLOR`), light-terminal variant via `ARTCRAFT_THEME=light`.
- **Plays well with scripts** — `--status` prints one line per app anywhere
  (pipes, CI), the state being `installed`, `stale` (built from an older
  revision than the clone), `cloned` or `missing`; the UI never opens without
  a real TTY; logs go to `~/.cache/artcraft-suite-aur/`, never onto the
  terminal.

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
artcraft-suite-aur -p 1        # one build at a time (low-spec machines)
artcraft-suite-aur --releases  # track release tags, not every commit
artcraft-suite-aur --status    # one line per app: repo|state|pkg|version|dir
artcraft-suite-aur --version
artcraft-suite-aur --help
```

If stdin or stdout is not a terminal (a pipe, `less`, CI), the app never enters the
full-screen UI: it prints the `--status` format and a hint on stderr, then exits 0.

### Layout

```text
┌ ArtCraft Suite ──────────────────────────────────────── ✓ 6  ↑ 3  · 2  ⚙ 2/2 ┐
│ ArtCraft Suite — install · update · launch the Craft suite                 │
│ ✓ 1 update ready — u updates wordcraft                                     │   ← status / errors
└────────────────────────────────────────────────────────────────────────────┘
┌ Apps (13) · 1 picked ──────────────────────────────────────────────────────┐
│▶  ■  ✓   photocraft              0.5.0                      up to date       │   ← picked + selected
│      The open-source image editor you already know how to use.              │   ← opened with →
│   □  ↑3  wordcraft               0.1.42                     3 commits behind │
│   □  ◐   deckcraft               0.3.1                      updating…        │
│   □  ·   cadcraft                -                          not installed    │
└────────────────────────────────────────────────────────────────────────────┘
┌ What's new in wordcraft ───────────────────────────────────────────────────┐
│ 0.1.42 · 3 commits to pull                                                 │   ← the update's contents
│ 9f8e7d6 fix .docx table borders                                            │
│ f refreshes · u updates                                                    │
└────────────────────────────────────────────────────────────────────────────┘
 [Space] pick  [Ctrl+Space] pick more  [Enter] open  [u] update  [o] attention
 [j/k] move  [→] details  [←] close  [f] fetch  [x] cancel  [?] help  [q] quit
 [U] update all  [a] install all  [d] uninstall  [p] builds  [r] rescan
```

Every row states its situation twice — a glyph and the same fact in words
(`✓ up to date`, `↑3 3 commits behind`, `? not checked`, `◐ updating…`,
`✗ build failed`) — so nothing depends on colour alone. A picked row fills its
square (`■`) in brand blue, and the highlighted row turns every character to
dark brand ink on the accent bar — focus reads as contrast, not colour. `→`
opens an indented one-line description of the highlighted app (the official
getartcraft.com blurb); `←` closes it, and several apps can stay open while you
move around. The header counts the suite at a glance
(`✓ up to date · ↑ behind · · missing · ? not checked`), the status row is the
feedback channel (queued/running tasks, `✓ installed photocraft (0.2.0) ·
added to app menu`, `✗ photocraft · installing: build failed`), and the bottom
pane answers "what would `u` even change?" with the upstream commits waiting to
be pulled — or the live build log while a task runs. `?` opens the full key
reference: every hint with a one-line description of what it does, scrolled
with `j`/`k` on short terminals.

Every install and rebuild also registers the app on your desktop: the binary
lands in `~/.local/bin`, the repo's `packaging/linux/*.desktop` entry and its
hicolor icons are copied into `~/.local/share/applications` and
`~/.local/share/icons/hicolor`, and the desktop/icon caches are refreshed — so
the app appears in your app menu right away. The success message says as much
(`· added to app menu`). `d` undoes exactly this — and goes one step further:
it removes the binary, the desktop entry, the icons *and* the clone in
`~/<repo>`, after a popup asks for a `y` confirmation first.

## Keyboard shortcuts

| Key | Action |
|---|---|
| `j` / `k`, `↑` / `↓` | move selection |
| `PgUp` / `PgDn` | jump 10 rows |
| `g` / `Home`, `G` / `End` | first / last app |
| `→` / `l` | open an indented description row under the highlighted app |
| `←` / `h` | close it again |
| `Space` | pick the highlighted row (press again on the only pick to clear) |
| `Ctrl+Space` | add / remove the highlighted row from the picked set |
| `Enter` | launch the selected app, or install it if missing; on a multi-pick, install every missing one |
| `u` / `i` / `f` | update / install / fetch — the picked rows, or the highlighted row when nothing is picked |
| `U` / `a` | update everything cloned / install everything missing |
| `d` | uninstall the picked/highlighted apps — binary, desktop entry, icons and the clone; asks for `y`, any other key cancels |
| `p` | popup to set how many builds run at once (1–16; low-spec machines should stick to 1–2) |
| `o` | toggle **attention only** — hide the apps that are up to date |
| `t` | toggle **release tracking** — rows count release tags instead of commits, and `u` lands on the newest release; every clone re-checks |
| `x` / `Delete` | cancel the running task, or dequeue a queued one — again over the picked set |
| `r` | rescan from disk (background): clone locations, menu entries, and the upstream check |
| `?` | toggle the help overlay |
| `q` / `Esc` | quit — asks a second time while tasks are running |
| `Ctrl+C` | quit immediately |

`Ctrl` chords never act as their bare letters: `Ctrl+A` will not start
"install all" by accident — only `Ctrl+Space` (pick) and `Ctrl+C`/`Ctrl+D`
(quit) do anything. Picking is a temporary selection: `u`, `i`, `f`, `d`, `x`
and `Enter` look at the picked rows first and fall back to the highlighted row
when nothing is picked, so the picks always win. `o` narrows the list to the
apps that need attention (missing, behind upstream, not yet checked, or
failed) — the highlight follows the filter, and the header badge says when it
is on. At most `-p N`
apps compile at once (default 2; `p` opens a popup to change it live, and the
header shows `⚙ n/limit` while builds run) — the rest queue up (shown as `◌`
and in the status row).

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
(`✓ up to date`, `↑3 commits behind`, `? not checked`, `◐ updating…`,
`✗ build failed`), so the UI never depends on colour alone.

| Variable | Effect | Default |
|---|---|---|
| `ARTCRAFT_THEME` | `light` = darker ink for light terminal backgrounds | `dark` |

## Configuration

| Variable | Effect | Default |
|---|---|---|
| `ARTCRAFT_GIT_ORG` | GitHub org the suite is cloned from | `storytold` |
| `ARTCRAFT_REPO_ROOTS` | extra directories searched for suite clones (colon-separated; `$HOME` is always searched) | — |
| `ARTCRAFT_PARALLEL` | how many apps compile at once (1–16; same as `-p`; low-spec: 1–2) | `2` |
| `ARTCRAFT_TRACK` | `releases` = track release tags instead of commits (same as `--releases`, or `t` in the UI); `commits` = every commit on the branch | `commits` |
| `ARTCRAFT_FETCH` | `never` (or `0`/`no`/`off`) = skip the automatic upstream check on launch and `r`; `f` / `u` still fetch | check on |
| `ARTCRAFT_NOTIFY` | `never` (or `0`/`no`/`off`) = suppress the desktop notification raised when an install or update finishes (needs `notify-send`) | notify on |
| `XDG_CACHE_HOME` | where build logs live | `~/.cache` |
| `HOME` | where repos are cloned, and `~/.local/bin` for installs | — |

Clones are discovered automatically under `$HOME` (and every `ARTCRAFT_REPO_ROOTS`
directory) up to three levels deep — `~/photocraft`, `~/src/photocraft`,
`~/Documents/GitHub/photocraft` — either by directory name or by the clone's
`github.com/<org>/<repo>` remote (which catches renamed clones and forks).
Desktop entries are read from `~/.local/share/applications` and
`$XDG_DATA_DIRS`; binaries are looked up in `~/.local/bin`, `~/.cargo/bin`,
`/usr/local/bin` and `/usr/bin`.

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

**A row said "up to date" but `git pull` brought new commits**
That was the cached refs talking: a clone's `origin/*` only moves when something
fetches, and only a fetch (or a pull) makes "behind" honest. Launch and `r` now
fetch every clone in the background (bounded, so a dead network can't wedge the
UI) before counting. When the fetch can't run — offline, or
`ARTCRAFT_FETCH=never` — the row says `? not checked` instead of guessing;
press `f` (fetch) or `u` (update) once the network is back.

**An app I have installed shows as `· missing`**
Discovery covers clones up to three levels under `$HOME` (plus any
`ARTCRAFT_REPO_ROOTS` directories), matched by directory name *or* by the
clone's `github.com/<org>/<repo>` remote (renamed clones and forks included);
menu entries are read from `~/.local/share/applications` and the system data
dirs. A clone buried deeper (`~/code/rust/projects/photocraft`) is outside the
walk — point `ARTCRAFT_REPO_ROOTS=/path/to/a/parent` at a directory above it.
Press `r` after changing anything.

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
  assert on what the user sees — header summary counts (including `? N` for
  unchecked clones), status glyphs *and* their phrases (`✓ up to date`,
  `↑3 commits behind`, `? not checked`), the changelog pane,
  the details footer (clone location + menu state), the live-log handoff, the
  attention filter, picked squares, a building row's progress bar and `~`-ETA
  (and that a build with no denominator shows elapsed time, not a fake
  percentage), the key bar
  fitting 80 columns, help fitting 60x15, and the polite message below the
  minimum.
- **Palette** (`src/theme.rs`): every tier is asserted — monochrome yields no
  colour at all, truecolor paints the brand RGB, 256/16 fall back sensibly,
  and focus still shows via reverse video when colour is gone.
- **Interaction** (`src/app.rs`): scripted key sequences → assert state. Covers
  selection clamping, `Ctrl+C`, the help overlay swallowing keys, two-step quit,
  dequeueing, `Space`/`Ctrl+Space` picking, batch actions over the picked set
  (never the cursor row), `Enter` installing only the missing picks, the `o`
  attention filter (including clamping the highlight onto a visible row and
  keeping unchecked clones), failed
  tasks counting as attention, the `d` → `y` uninstall confirmation (any other
  key cancels it; busy and not-installed rows are skipped), the `p` parallel-builds
  popup (apply promotes queued work; Esc keeps the old limit; clamped 1–16),
  status tone following the message prefix, the scan pipeline's event
  flow (`Ev::Loaded` rows + `checking N repos…`, then `Ev::Checked` settling
  on `ready`/`refreshed`; a scan with nothing to fetch waits for `Checked`),
  and build progress (`Ev::Progress` fills `progress` while a build runs,
  `Ev::Done` clears it, and a single running task earns a `~`-prefixed ETA on
  the status row).
- **Engine** (`src/engine.rs`): children are wired to the log file (not the tty),
  `log_tail` survives invalid UTF-8, the version probe respects its timeout,
  repo discovery finds suite clones by name and by git remote at every depth
  (dot-dirs skipped, shallow copies preferred), `.desktop` entries are matched
  by stem, `Exec=` and `Icon=` (with `Hidden=true` ignored), and binary lookup
  prefers `~/.local/bin` over the system paths. The upstream check is
  integration-tested against a local file remote: a commit lands on the origin
  after the clone, `behind` stays 0 on the stale refs, and
  `git_fetch_bounded` makes the count (and the commit list) real. Build
  progress is counted from cargo's `--message-format=json` stream: every
  artifact resolves an expected unit, but only non-`fresh` crates count as
  work done — and only real work (fresh-excluded crates and `build.rs` runs)
  samples the pace, so cached bursts never deflate the estimate. The
  percentage/ETA math (`Progress::pct`, `Progress::eta_secs`) is asserted
  against injected sample timestamps, including the incremental-rebuild
  regression where the old elapsed-rate formula claimed ~2h for ~10m of work.
  The percentage's denominator is seeded before the build from a persisted
  exact unit count (or a `cargo tree`+metadata estimate of first-build
  units), so it starts at 0%; an
  exhausted seed holds the bar at 99% with an `…` ETA, and a build with no
  obtainable denominator shows elapsed time rather than a bar. The line
  parser itself is fed cargo-shaped lines in a test.
- **End-to-end** (`scripts/tui_smoke.py`): boots the real binary in a sized pty,
  sends real keys (waiting for the scan *and* the background upstream check —
  the status line reaches `ready` — before the first key),
  replays the output through a minimal ANSI screen and checks
  alt-screen entry/exit, cursor restore, the summary counters, status phrases,
  the details pane, picking, the attention filter, the parallel-builds popup
  and the frames themselves.

## License

MIT OR Apache-2.0.

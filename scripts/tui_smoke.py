#!/usr/bin/env python3
"""End-to-end smoke test: run the TUI in a sized pty, drive keys, replay the
output through a minimal ANSI screen and assert on what the user actually sees."""
import fcntl
import os
import pty
import re
import select
import struct
import sys
import termios
import time

BIN = sys.argv[1] if len(sys.argv) > 1 else "./target/debug/artcraft-suite-aur"
COLS, ROWS = 80, 24


class Screen:
    def __init__(self):
        self.grid = [[" "] * COLS for _ in range(ROWS)]
        self.x = self.y = 0
        self.pending = ""

    def feed(self, text):
        text = self.pending + text
        self.pending = ""
        # an escape sequence split across two pty reads must be reassembled
        m = re.search(r"\x1b(\[[0-9;?]*)?$", text)
        if m:
            self.pending = text[m.start():]
            text = text[:m.start()]
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == "\x1b":
                m = re.match(r"\x1b\[([0-9;?]*)([A-Za-z])", text[i:])
                if not m:
                    i += 1
                    continue
                params, cmd = m.group(1), m.group(2)
                if cmd == "H":
                    nums = [int(p) if p else 1 for p in params.split(";")] if params else [1, 1]
                    self.y = nums[0] - 1
                    self.x = (nums[1] - 1) if len(nums) > 1 else 0
                elif cmd == "J" and params in ("2", "", "0"):
                    self.grid = [[" "] * COLS for _ in range(ROWS)]
                i += m.end()
                continue
            if ch == "\n":
                self.y += 1
                self.x = 0
            elif ch == "\r":
                self.x = 0
            else:
                if 0 <= self.y < ROWS and 0 <= self.x < COLS:
                    self.grid[self.y][self.x] = ch
                self.x += 1
            i += 1

    def text(self):
        return "\n".join("".join(r) for r in self.grid)

    def row(self, n):
        return "".join(self.grid[n - 1])


pid, fd = pty.fork()
if pid == 0:
    fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    os.environ["TERM"] = "xterm-256color"
    os.execv(BIN, [BIN])

raw = bytearray()
screen = Screen()
stages = {}


def pump(seconds, stage=None):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                chunk = os.read(fd, 65536)
            except OSError:
                break
            if not chunk:
                break
            raw.extend(chunk)
            screen.feed(chunk.decode("utf-8", "replace"))
    if stage:
        stages[stage] = screen.text()


def send(data):
    os.write(fd, data.encode())


def marked_line(frame):
    for line in frame.splitlines():
        if "▶" in line:
            return line
    return ""


def pump_until(pred, timeout, stage=None):
    """Pump until `pred(screen text)` holds or `timeout` passes — the first
    frame is instant, but "ready" only lands after the background upstream
    check finishes (a bounded git fetch per clone)."""
    end = time.time() + timeout
    while time.time() < end:
        pump(0.1)
        if pred(screen.text()):
            break
    if stage:
        stages[stage] = screen.text()


def settled_ready(t):
    """`ready` must survive a short settle — the status line is the last thing
    the pipeline paints, and a repaint that redraws it means the app is live."""
    if "ready" not in t or "Apps (" not in t:
        return False
    before = screen.text()
    pump(0.25)
    return "ready" in screen.text() and screen.text() == before


pump_until(settled_ready, 20.0, "startup")
send("jjj")
pump(0.4, "nav")
send("\x1b[B")  # real Down arrow, not just 'j'
pump(0.4, "arrow_down")
send("\x1b[A")  # real Up arrow
pump(0.4, "arrow_up")
send(" ")
pump(0.4, "pick_one")
send("j\x00")  # Ctrl+Space: toggle a second row
pump(0.4, "pick_two")
send("\x1b[C")  # Right: expand the description under the highlighted app
pump(0.4, "expanded")
send("k")  # move up; the description stays glued to its app
pump(0.4, "expanded_moved")
send("j")  # back onto the expanded app
pump(0.4, "expanded_return")
send("\x1b[D")  # Left: collapse it again
pump(0.4, "collapsed")
send("?")
pump(0.4, "help")
send("jjjjjjjjjj")  # scroll the help overlay to its end (offset clamps)
pump(0.4, "help_scrolled")
send("\x1b")
pump(0.4, "help_closed")
send("o")
pump(0.4, "filter_on")
send("o")
pump(0.4, "filter_off")
send("p")
pump(0.4, "jobs_popup")
send("\x1b")
pump(0.4, "jobs_closed")
send("x")
pump(0.7, "cancel_msg")
send("q")
pump(1.0, "quit")

_, status = os.waitpid(pid, 0)
os.close(fd)
code = os.waitstatus_to_exitcode(status)
text = raw.decode("utf-8", "replace")

print("===== startup (80x24) =====")
print(stages["startup"])
print("===== help overlay =====")
print(stages["help"])
print("===== after two picks =====")
for line in stages["pick_two"].splitlines()[:5]:
    print(line)
print("===== attention filter on =====")
for line in stages["filter_on"].splitlines()[:6]:
    print(line)
print("===== after x (status feedback) =====")
for line in stages["cancel_msg"].splitlines()[:4]:
    print(line)

checks = [
    ("entered alt screen", "\x1b[?1049h" in text),
    ("left alt screen", "\x1b[?1049l" in text),
    ("cursor restored", "\x1b[?25h" in text),
    ("header rendered", "ArtCraft Suite" in stages["startup"]),
    ("status line rendered", "ready" in stages["startup"]),
    ("header summary rendered", "✓ " in stages["startup"] and "↑ " in stages["startup"]),
    ("app list rendered", "Apps (" in stages["startup"]),
    ("version column uses available width", "artcraft-l…" not in stages["startup"]
     and "craft-launcher" in stages["startup"]),
    ("status phrases rendered", any(
        p in stages["startup"]
        for p in ("up to date", "not installed", "cloned, not built", "behind", "not checked")
    )),
    ("details pane rendered", any(
        p in stages["startup"]
        for p in ("What's new in", "up to date", "not installed", "cloned, not built",
                  "not checked")
    )),
    ("key bar row 1", "[Space] pick" in stages["startup"]
     and "[Ctrl+Space] pick more" in stages["startup"] and "[o] attention" in stages["startup"]),
    ("key bar row 1 not clipped", stages["startup"].splitlines()[-3].rstrip().endswith("attention")),
    ("key bar row 2", "[q] quit" in stages["startup"] and "[j/k] move" in stages["startup"]
     and "[→] details" in stages["startup"] and "[←] close" in stages["startup"]),
    ("key bar row 2 not clipped", stages["startup"].splitlines()[-2].rstrip().endswith("quit")),
    ("key bar row 3", "[U] update all" in stages["startup"] and "[a] install all" in stages["startup"]
     and "[d] remove" in stages["startup"]),
    ("key bar row 3 not clipped", stages["startup"].splitlines()[-1].rstrip().endswith("releases")),
    ("selection moved", "▶ " in stages["nav"] and stages["nav"].count("▶") == 1),
    ("Down arrow moves selection", marked_line(stages["arrow_down"]) != ""
     and marked_line(stages["arrow_down"]) != marked_line(stages["nav"])),
    ("Up arrow moves selection back", marked_line(stages["arrow_up"]) == marked_line(stages["nav"])),
    ("empty squares drawn", "□" in stages["startup"]),
    ("Space fills the square", "■" in stages["pick_one"] and "· 1 picked" in stages["pick_one"]),
    ("Ctrl+Space adds a pick", "· 2 picked" in stages["pick_two"]),
    ("right arrow expands the description", "motion graphics" in stages["expanded"]),
    ("description follows its app while moving", "motion graphics" in stages["expanded_moved"]
     and "motion graphics" in stages["expanded_return"]),
    ("left arrow collapses the description", "motion graphics" not in stages["collapsed"]),
    ("help overlay rendered", "Esc to close" in stages["help"]),
    ("help describes each hint", "pick the row; press again to unpick" in stages["help"]
     and "j/k or" in stages["help"]),
    ("help scrolls to the legend", "Reading a row" in stages["help_scrolled"]
     and "up to date" in stages["help_scrolled"]
     and "Navigation" not in stages["help_scrolled"]),
    ("help closes on Esc", "Esc to close" not in stages["help_closed"]),
    ("attention filter engages", "attention only" in stages["filter_on"]
     or "nothing needs attention" in stages["filter_on"]),
    ("attention filter clears", "attention only" not in stages["filter_off"]),
    ("jobs popup opens with the warning", "Parallel builds" in stages["jobs_popup"]
     and "Stick to 1–2" in stages["jobs_popup"]),
    ("jobs popup closes on Esc", "Parallel builds" not in stages["jobs_closed"]),
    ("idle-cancel feedback rendered", "nothing to cancel" in stages["cancel_msg"]),
    ("status survives redraw", "cadcraft" in stages["cancel_msg"] or "designcraft" in stages["cancel_msg"]),
    ("exit code 0", code == 0),
]
ok = True
for name, passed in checks:
    print(f"{'PASS' if passed else 'FAIL'}  {name}")
    ok &= passed
print(f"exit={code} bytes={len(raw)}")
sys.exit(0 if ok else 1)

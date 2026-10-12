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


pump(1.5, "startup")
send("jjj")
pump(0.4, "nav")
send(" ")
pump(0.4, "pick_one")
send("j\x00")  # Ctrl+Space: toggle a second row
pump(0.4, "pick_two")
send("?")
pump(0.4, "help")
send("\x1b")
pump(0.4, "help_closed")
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
print("===== after x (status feedback) =====")
for line in stages["cancel_msg"].splitlines()[:4]:
    print(line)

checks = [
    ("entered alt screen", "\x1b[?1049h" in text),
    ("left alt screen", "\x1b[?1049l" in text),
    ("cursor restored", "\x1b[?25h" in text),
    ("header rendered", "ArtCraft Suite" in stages["startup"]),
    ("status line rendered", "ready" in stages["startup"]),
    ("app list rendered", "Apps (15)" in stages["startup"]),
    ("dot legend rendered", "◐ busy" in stages["startup"] and "◌ queued" in stages["startup"]),
    ("key bar row 1", "[Space] pick" in stages["startup"]),
    ("key bar row 1 not clipped", stages["startup"].splitlines()[-2].rstrip().endswith("update all")),
    ("key bar row 2", "[q] quit" in stages["startup"]),
    ("selection moved", "▶ " in stages["nav"] and stages["nav"].count("▶") == 1),
    ("empty checkboxes drawn", "☐" in stages["startup"]),
    ("Space picks the row", "☑" in stages["pick_one"] and "· 1 picked" in stages["pick_one"]),
    ("Ctrl+Space adds a pick", "· 2 picked" in stages["pick_two"]),
    ("help overlay rendered", "Esc to close" in stages["help"]),
    ("help legend rendered", "commits you are behind upstream" in stages["help"]),
    ("help closes on Esc", "Esc to close" not in stages["help_closed"]),
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

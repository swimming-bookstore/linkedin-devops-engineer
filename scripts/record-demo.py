#!/usr/bin/env python3
"""Record docs/demo.mp4 of the LinkedIn guest job lookup in a real terminal.

Pixels come from ~/demo WindowRecord (xfce4-terminal + XGetImage), not a log.
"""

from __future__ import annotations

import os
import shutil
import sys
import time
from pathlib import Path

os.environ.setdefault("DISPLAY", ":0.0")
os.environ.setdefault("XAUTHORITY", str(Path.home() / ".Xauthority"))

sys.path.insert(0, os.path.expanduser("~/demo"))

from record.window import WindowRecord
from record import x11cap
from ctypes import CDLL, c_char_p, c_int, c_uint, c_ulong, c_void_p

xtst = CDLL("libXtst.so.6")
xtst.XTestFakeKeyEvent.argtypes = [c_void_p, c_uint, c_int, c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [c_void_p, c_uint, c_int, c_ulong]

ROOT = Path("/home/packer/linkedin-devops-engineer")
OUT = ROOT / "docs" / "demo.mp4"
TITLE = "linkedin-devops-jobs-demo"
CMD = "./scripts/demo-run.sh"


def wheel_scroll(rec: WindowRecord, n: int) -> None:
    """Wheel events scroll the VTE view. Page Down is delivered to the shell."""
    rec.focus()
    rec.move(540, 400)
    rec.hold(0.1)
    for _ in range(n):
        xtst.XTestFakeButtonEvent(rec._dpy, 5, 1, 0)
        xtst.XTestFakeButtonEvent(rec._dpy, 5, 0, 0)
        x11cap.x11.XFlush(rec._dpy)
        rec.hold(0.18)


def wait_for_desk(rec: WindowRecord, timeout: float) -> bool:
    """Gold header means the alternate-screen desk is up, not the fetch log.

    Sample the same composited grab the encoder uses. A raw window pixmap
    often has no VTE glyphs, so the gold header is not there.
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        frame = x11cap.grab_window(rec._dpy, rec._wid, rec._w, rec._h, False)
        if frame and _has_gold(frame):
            return True
        rec.hold(0.4)
    return False


def _has_gold(frame: bytes) -> bool:
    # grab is BGR0. Header gold is RGB(238, 178, 92) -> B=92 G=178 R=238.
    step = max(16, (len(frame) // 8000) & ~3)
    hits = 0
    for i in range(0, len(frame) - 2, step):
        b, g, r = frame[i], frame[i + 1], frame[i + 2]
        if r > 210 and 140 < g < 210 and b < 120 and r > g > b:
            hits += 1
            if hits >= 4:
                return True
    return False


def die(msg: str) -> None:
    print(msg, file=sys.stderr)
    sys.exit(1)


def main() -> None:
    bin_path = ROOT / "target" / "debug" / "linkedin-devops-jobs"
    if not bin_path.exists():
        die(f"missing {bin_path}; cargo build first")
    term = shutil.which("xfce4-terminal")
    if not term:
        die("need xfce4-terminal (xterm glyphs do not survive XGetImage)")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    inner = f"cd {ROOT} && clear && PS1='$ ' exec bash --noprofile --norc"
    rec = WindowRecord(
        TITLE,
        OUT,
        fps=12,
        max_sec=180.0,
        min_w=700,
        min_h=400,
        overlay_pointer=False,
    )
    proc = rec.spawn(
        [
            term,
            "--disable-server",
            f"--title={TITLE}",
            "--font=DejaVu Sans Mono 13",
            "--geometry=120x40+40+24",
            "-x",
            "bash",
            "-lc",
            inner,
        ],
        env={**os.environ, "TERM": "xterm-256color", "DISPLAY": ":0.0"},
    )
    rec.pid = proc.pid
    time.sleep(0.8)
    rec.attach(timeout=12)
    rec.focus()
    rec.start_capture()
    rec.hold(0.6)
    rec.focus()
    rec.type_text(CMD, delay=0.04)
    rec.hold(0.35)
    rec.focus()
    rec.key("Return")
    # Full guest walk is ~20 pages even when every id is already in SQLite.
    # Do not send desk keys until the gold header is actually on screen.
    if not wait_for_desk(rec, timeout=100.0):
        rec._stop.set()
        if rec._cap:
            rec._cap.join(timeout=5)
        rec.kill_all()
        die("desk did not appear")
    rec.hold(1.0)
    for key in ("j", "j", "j", "j", "j", "J", "J", "J", "G"):
        rec.focus()
        rec.key(key)
        rec.hold(0.8)
    rec.hold(1.2)
    rec.focus()
    rec.key("g")
    rec.hold(0.7)
    rec.focus()
    rec.key("s")
    rec.hold(1.6)
    rec.focus()
    rec.key("q")
    rec.hold(0.6)
    rec._stop.set()
    if rec._cap:
        rec._cap.join(timeout=8)
    if rec._ff and rec._ff.poll() is None:
        rec._ff.wait(timeout=20)
    rec.kill_all()
    if not OUT.exists() or OUT.stat().st_size < 20_000:
        size = OUT.stat().st_size if OUT.exists() else 0
        die(f"recording failed: {OUT} size={size} frames={rec.nframes}")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {rec.nframes} frames)")


if __name__ == "__main__":
    main()

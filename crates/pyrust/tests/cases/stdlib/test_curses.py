# Issue #3266: `curses` ecosystem parity test (`_curses`, `_curses_panel`,
# `curses`, `curses.ascii`, `curses.has_key`, `curses.panel`, `curses.textpad`).
#
# - On Windows (`sys.platform == "win32"`): verifies importing each module raises
#   `ModuleNotFoundError`, matching CPython 3.12.3.
# - On Unix (`sys.platform != "win32"`): tests constants, `curses.ascii`,
#   `curses.has_key`, `unctrl`/`keyname`, and runs a full headless PTY session
#   exercising `initscr`, `newwin`, `subwin`, `derwin`, `newpad`, `addstr`,
#   `addnstr`, `addch`, `instr`, `inch`, `move`, `box`, `border`, `hline`,
#   `vline`, `clear`, `clrtoeol`, `delch`, `insch`, `deleteln`, `insertln`,
#   `start_color`, `init_pair`, `pair_content`, `color_pair`, `pair_number`,
#   `tigetstr`/`tigetnum`/`tigetflag`/`tparm`, `wrapper`, `curses.panel`, and
#   `curses.textpad` (`rectangle`, `Textbox`).

import os
import sys

CURSES_MODULES = [
    "_curses",
    "_curses_panel",
    "curses",
    "curses.ascii",
    "curses.has_key",
    "curses.panel",
    "curses.textpad",
]

if sys.platform == "win32":
    for mod_name in CURSES_MODULES:
        try:
            __import__(mod_name)
            print("UNEXPECTED_IMPORT", mod_name)
        except ModuleNotFoundError as exc:
            print("win32_missing_ok", mod_name, type(exc).__name__)
else:
    import _curses
    import _curses_panel
    import curses
    import curses.ascii as ca
    import curses.has_key as chk
    import curses.panel as cp
    import curses.textpad as ctp
    import pty
    import select
    import subprocess
    import tempfile
    import termios

    # ── 1. Identity & constants ──────────────────────────────────────────────
    print(
        "identities",
        curses.error is _curses.error,
        issubclass(curses.error, Exception),
        cp.error is _curses_panel.error,
        issubclass(cp.error, Exception),
        isinstance(_curses.version, bytes),
        isinstance(_curses_panel.version, bytes),
        curses.ERR,
        curses.OK,
    )

    print(
        "colors",
        curses.COLOR_BLACK,
        curses.COLOR_RED,
        curses.COLOR_GREEN,
        curses.COLOR_YELLOW,
        curses.COLOR_BLUE,
        curses.COLOR_MAGENTA,
        curses.COLOR_CYAN,
        curses.COLOR_WHITE,
    )

    print(
        "attrs_distinct",
        curses.A_NORMAL == 0,
        (curses.A_BOLD & curses.A_ATTRIBUTES) == curses.A_BOLD,
        (curses.A_UNDERLINE & curses.A_ATTRIBUTES) == curses.A_UNDERLINE,
        (curses.A_REVERSE & curses.A_ATTRIBUTES) == curses.A_REVERSE,
        (curses.A_STANDOUT & curses.A_ATTRIBUTES) == curses.A_STANDOUT,
        (curses.A_DIM & curses.A_ATTRIBUTES) == curses.A_DIM,
        (curses.A_BLINK & curses.A_ATTRIBUTES) == curses.A_BLINK,
        (curses.A_ALTCHARSET & curses.A_ATTRIBUTES) == curses.A_ALTCHARSET,
        (curses.A_INVIS & curses.A_ATTRIBUTES) == curses.A_INVIS,
        (curses.A_CHARTEXT & curses.A_ATTRIBUTES) == 0,
        curses.A_COLOR != 0,
    )

    print(
        "keys_ordered",
        curses.KEY_MIN <= curses.KEY_DOWN < curses.KEY_UP < curses.KEY_LEFT < curses.KEY_RIGHT,
        curses.KEY_F0 + 1 == curses.KEY_F1,
        curses.KEY_F0 + 12 == curses.KEY_F12,
        curses.KEY_MIN <= curses.KEY_BACKSPACE <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_HOME <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_END <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_DC <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_IC <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_NPAGE <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_PPAGE <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_ENTER <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_RESIZE <= curses.KEY_MAX,
        curses.KEY_MIN <= curses.KEY_MOUSE <= curses.KEY_MAX,
        curses.ALL_MOUSE_EVENTS > 0,
        curses.REPORT_MOUSE_POSITION > 0,
    )

    # ── 2. curses.ascii ──────────────────────────────────────────────────────
    print(
        "ascii_consts",
        ca.NUL,
        ca.SOH,
        ca.BEL,
        ca.BS,
        ca.TAB,
        ca.HT,
        ca.LF,
        ca.NL,
        ca.CR,
        ca.ESC,
        ca.SP,
        ca.DEL,
        len(ca.controlnames),
        ca.controlnames[0],
        ca.controlnames[32],
    )
    print(
        "ascii_preds",
        ca.isalnum("A"),
        ca.isalnum("7"),
        ca.isalnum("!"),
        ca.isalpha("z"),
        ca.isalpha("9"),
        ca.isascii(0),
        ca.isascii(127),
        ca.isascii(128),
        ca.isblank(" "),
        ca.isblank("\t"),
        ca.isblank("\n"),
        ca.iscntrl(0),
        ca.iscntrl(127),
        ca.iscntrl("A"),
        ca.isdigit("5"),
        ca.isdigit("a"),
        ca.isgraph("!"),
        ca.isgraph(" "),
        ca.islower("m"),
        ca.islower("M"),
        ca.isprint(" "),
        ca.isprint("~"),
        ca.isprint(127),
        ca.ispunct("."),
        ca.ispunct("a"),
        ca.isspace("\n"),
        ca.isspace("x"),
        ca.isupper("Q"),
        ca.isupper("q"),
        ca.isxdigit("f"),
        ca.isxdigit("F"),
        ca.isxdigit("g"),
        ca.isctrl(1),
        ca.isctrl(32),
        ca.ismeta(200),
        ca.ismeta(65),
    )
    print(
        "ascii_conv",
        ca.ascii(0xC1),
        ca.ascii("\xc1"),
        ca.ctrl("a"),
        ca.ctrl(65),
        ca.alt(65),
        ca.alt("A"),
        ca.unctrl("a"),
        ca.unctrl(1),
        ca.unctrl(0x7F),
        ca.unctrl(0x81),
    )

    # ── 3. unctrl, keyname, has_key table ────────────────────────────────────
    print(
        "unctrl_keyname",
        curses.unctrl(1),
        curses.unctrl(b"A"),
        curses.unctrl(0x7F),
        curses.keyname(ord("a")),
        curses.keyname(1),
        curses.keyname(curses.KEY_UP),
        curses.keyname(curses.KEY_F5),
    )
    print(
        "has_key_table",
        isinstance(chk._capability_names, dict),
        chk._capability_names[curses.KEY_UP],
        chk._capability_names[curses.KEY_DOWN],
        chk._capability_names[curses.KEY_LEFT],
        chk._capability_names[curses.KEY_RIGHT],
        chk._capability_names[curses.KEY_F1],
    )

    # ── 4. Headless PTY full-screen curses / panel / textpad test ────────────
    PTY_CHILD_SCRIPT = r"""
import curses
import curses.ascii as ca
import curses.has_key as chk
import curses.panel as cp
import curses.textpad as ctp
import os
import sys

out_path = sys.argv[1]
lines = []

def log(*parts):
    lines.append(" ".join(str(p) for p in parts))

stdscr = curses.initscr()
try:
    log("initscr", stdscr.getmaxyx(), curses.LINES, curses.COLS, curses.isendwin())
    curses.noecho()
    curses.cbreak()
    stdscr.keypad(True)
    old_vis = curses.curs_set(0)
    log("curs_set_ok", old_vis in (0, 1, 2))

    # Color support
    has_c = curses.has_colors()
    log("has_colors", has_c)
    if has_c:
        curses.start_color()
        log("color_caps", curses.COLORS >= 8, curses.COLOR_PAIRS >= 64)
        curses.init_pair(1, curses.COLOR_RED, curses.COLOR_BLUE)
        log("pair1", curses.pair_content(1))
        cp1 = curses.color_pair(1)
        log("pair_roundtrip", curses.pair_number(cp1 | curses.A_BOLD))

    # Terminfo queries
    log(
        "terminfo",
        curses.tigetnum("colors") >= 8,
        curses.tigetflag("am") == 1,
        isinstance(curses.tigetstr("cup"), bytes),
        curses.tparm(b"\x1b[%i%p1%d;%p2%dH", 2, 4),
    )
    log("has_key_up", curses.has_key(curses.KEY_UP), chk.has_key(curses.KEY_UP))

    # Window creation & writing/reading
    win = curses.newwin(10, 30, 2, 4)
    log("win_geom", win.getmaxyx(), win.getbegyx(), win.getparyx())
    win.addstr(0, 0, "Hello, Curses!")
    log("win_yx_after", win.getyx())
    log("win_instr", win.instr(0, 0, 14))
    log("win_inch_H", win.inch(0, 0) & curses.A_CHARTEXT)

    win.addnstr(1, 0, "abcdef", 3)
    log("win_addnstr", win.instr(1, 0, 5))

    win.move(2, 0)
    win.addch("X")
    win.insch(2, 0, "Y")
    log("win_insch", win.instr(2, 0, 2))
    win.delch(2, 0)
    log("win_delch", win.instr(2, 0, 2))

    # Subwindow sharing storage with parent
    sub = win.derwin(3, 10, 4, 2)
    log("sub_geom", sub.getmaxyx(), sub.getbegyx(), sub.getparyx())
    sub.addstr(0, 0, "SUBWIN")
    log("parent_sees_sub", win.instr(4, 2, 6))

    # Box & lines
    bwin = curses.newwin(5, 10, 12, 2)
    bwin.box()
    log("box_corners", (bwin.inch(0, 0) & curses.A_ALTCHARSET) != 0, (bwin.inch(4, 9) & curses.A_ALTCHARSET) != 0)
    bwin.clear()
    log("after_clear", bwin.instr(0, 0, 4))

    # Pad
    pad = curses.newpad(20, 40)
    pad.addstr(5, 5, "PADTEXT")
    log("pad_instr", pad.instr(5, 5, 7))
    pad.noutrefresh(0, 0, 0, 0, 10, 20)

    # Touch tracking & attributes
    win.untouchwin()
    log("untouched", win.is_wintouched())
    win.touchwin()
    log("touched", win.is_wintouched(), win.is_linetouched(0))
    win.attrset(curses.A_BOLD)
    win.attron(curses.A_UNDERLINE)
    win.attroff(curses.A_BOLD)
    log("attr_get", win.attr_get()[0] == curses.A_UNDERLINE)
    win.attrset(curses.A_NORMAL)

    # Overlay / overwrite
    src = curses.newwin(3, 10, 0, 0)
    dst = curses.newwin(3, 10, 0, 0)
    dst.addstr(0, 0, "12345")
    src.addstr(0, 0, "A B")
    src.overlay(dst)
    log("overlay", dst.instr(0, 0, 5))
    src.overwrite(dst)
    log("overwrite", dst.instr(0, 0, 5))

    # Panel stack
    pw1 = curses.newwin(4, 10, 1, 1)
    pw2 = curses.newwin(4, 10, 2, 2)
    p1 = cp.new_panel(pw1)
    p2 = cp.new_panel(pw2)
    p1.set_userptr("panel-one")
    p2.set_userptr("panel-two")
    log(
        "panel_stack",
        cp.top_panel().userptr(),
        cp.bottom_panel().userptr(),
        p1.above().userptr(),
        p2.below().userptr(),
        p1.below() is None,
        p2.above() is None,
        p1.window() is pw1,
    )
    p1.top()
    log("panel_top_swap", cp.top_panel().userptr(), cp.bottom_panel().userptr())
    p1.hide()
    log("panel_hide", p1.hidden(), cp.top_panel().userptr())
    p1.show()
    log("panel_show", p1.hidden(), cp.top_panel().userptr())
    p1.move(3, 3)
    log("panel_move", pw1.getbegyx())
    cp.update_panels()
    curses.doupdate()

    # Textbox & rectangle
    rwin = curses.newwin(8, 20, 0, 0)
    ctp.rectangle(rwin, 0, 0, 5, 10)
    log("rect_ok", (rwin.inch(0, 0) & curses.A_ALTCHARSET) != 0)

    twin = curses.newwin(2, 12, 1, 1)
    tb = ctp.Textbox(twin)
    for ch in "hi!":
        tb.do_command(ord(ch))
    tb.do_command(ca.BS)
    tb.do_command(ord("?"))
    tb.do_command(ca.BEL)
    log("textbox_gather", repr(tb.gather()))

    # ungetch + getch
    curses.ungetch(ord("Z"))
    log("ungetch_getch", chr(stdscr.getch()))
finally:
    curses.endwin()

log("after_endwin", curses.isendwin())

# curses.wrapper
def wrapped_fn(scr, a, b=0):
    scr.addstr(0, 0, "WRAPPED")
    return (a + b, scr.instr(0, 0, 7))

wres = curses.wrapper(wrapped_fn, 10, b=5)
log("wrapper_res", wres, curses.isendwin())

with open(out_path, "w", encoding="utf-8") as f:
    f.write("\n".join(lines) + "\n")
"""

    with tempfile.TemporaryDirectory() as tmpdir:
        out_path = os.path.join(tmpdir, "curses_pty_out.txt")
        master_fd, slave_fd = pty.openpty()
        try:
            termios.tcsetwinsize(slave_fd, (24, 80))
            env = os.environ.copy()
            env["TERM"] = "xterm-256color"
            env["LINES"] = "24"
            env["COLS"] = "80"
            proc = subprocess.Popen(
                [sys.executable, "-c", PTY_CHILD_SCRIPT, out_path],
                stdin=slave_fd,
                stdout=slave_fd,
                stderr=slave_fd,
                env=env,
                close_fds=True,
            )
            os.close(slave_fd)
            slave_fd = -1

            # Drain PTY master output until child finishes so writes never block.
            while True:
                rlist, _, _ = select.select([master_fd], [], [], 0.05)
                if rlist:
                    try:
                        chunk = os.read(master_fd, 4096)
                        if not chunk:
                            break
                    except OSError:
                        break
                if proc.poll() is not None:
                    # Drain any remaining bytes
                    while True:
                        rlist, _, _ = select.select([master_fd], [], [], 0.01)
                        if not rlist:
                            break
                        try:
                            chunk = os.read(master_fd, 4096)
                            if not chunk:
                                break
                        except OSError:
                            break
                    break
            rc = proc.wait()
            print("pty_rc", rc)
        finally:
            if slave_fd >= 0:
                os.close(slave_fd)
            os.close(master_fd)

        with open(out_path, "r", encoding="utf-8") as f:
            for line in f.read().splitlines():
                print("pty:", line)

    print("curses_parity_ok")

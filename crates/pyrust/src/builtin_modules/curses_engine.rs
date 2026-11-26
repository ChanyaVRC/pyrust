//! Pure-Rust curses virtual screen, window, color, terminfo, and panel backend
//! (issue #3266).
//!
//! Zero C `libncurses` linkage: uses an in-memory character/attribute cell grid
//! with ANSI/VT100 escape output and POSIX `termios`/`TIOCGWINSZ` control when
//! attached to a real TTY (on `#[cfg(unix)]`), while operating cleanly in
//! headless/non-TTY/PTY environments.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use crate::value::Value;

// ── Return codes ─────────────────────────────────────────────────────────────

pub(crate) const ERR: i64 = -1;
pub(crate) const OK: i64 = 0;

// ── Color constants ──────────────────────────────────────────────────────────

pub(crate) const COLOR_BLACK: i64 = 0;
pub(crate) const COLOR_RED: i64 = 1;
pub(crate) const COLOR_GREEN: i64 = 2;
pub(crate) const COLOR_YELLOW: i64 = 3;
pub(crate) const COLOR_BLUE: i64 = 4;
pub(crate) const COLOR_MAGENTA: i64 = 5;
pub(crate) const COLOR_CYAN: i64 = 6;
pub(crate) const COLOR_WHITE: i64 = 7;

// ── Attribute bitmasks (matching standard ncurses 6.x) ───────────────────────

pub(crate) const A_NORMAL: u32 = 0;
pub(crate) const A_CHARTEXT: u32 = 0x0000_00ff;
pub(crate) const A_COLOR: u32 = 0x0000_ff00;
pub(crate) const A_STANDOUT: u32 = 1 << 16;
pub(crate) const A_UNDERLINE: u32 = 1 << 17;
pub(crate) const A_REVERSE: u32 = 1 << 18;
pub(crate) const A_BLINK: u32 = 1 << 19;
pub(crate) const A_DIM: u32 = 1 << 20;
pub(crate) const A_BOLD: u32 = 1 << 21;
pub(crate) const A_ALTCHARSET: u32 = 1 << 22;
pub(crate) const A_INVIS: u32 = 1 << 23;
pub(crate) const A_PROTECT: u32 = 1 << 24;
pub(crate) const A_HORIZONTAL: u32 = 1 << 25;
pub(crate) const A_LEFT: u32 = 1 << 26;
pub(crate) const A_LOW: u32 = 1 << 27;
pub(crate) const A_RIGHT: u32 = 1 << 28;
pub(crate) const A_TOP: u32 = 1 << 29;
pub(crate) const A_VERTICAL: u32 = 1 << 30;
pub(crate) const A_ITALIC: u32 = 1 << 31;
pub(crate) const A_ATTRIBUTES: u32 = 0xffff_ff00;

// ── Alternate Character Set (ACS_*) constants ────────────────────────────────

pub(crate) const ACS_ULCORNER: u32 = A_ALTCHARSET | (b'l' as u32);
pub(crate) const ACS_LLCORNER: u32 = A_ALTCHARSET | (b'm' as u32);
pub(crate) const ACS_URCORNER: u32 = A_ALTCHARSET | (b'k' as u32);
pub(crate) const ACS_LRCORNER: u32 = A_ALTCHARSET | (b'j' as u32);
pub(crate) const ACS_LTEE: u32 = A_ALTCHARSET | (b't' as u32);
pub(crate) const ACS_RTEE: u32 = A_ALTCHARSET | (b'u' as u32);
pub(crate) const ACS_BTEE: u32 = A_ALTCHARSET | (b'v' as u32);
pub(crate) const ACS_TTEE: u32 = A_ALTCHARSET | (b'w' as u32);
pub(crate) const ACS_HLINE: u32 = A_ALTCHARSET | (b'q' as u32);
pub(crate) const ACS_VLINE: u32 = A_ALTCHARSET | (b'x' as u32);
pub(crate) const ACS_PLUS: u32 = A_ALTCHARSET | (b'n' as u32);
pub(crate) const ACS_S1: u32 = A_ALTCHARSET | (b'o' as u32);
pub(crate) const ACS_S3: u32 = A_ALTCHARSET | (b'p' as u32);
pub(crate) const ACS_S7: u32 = A_ALTCHARSET | (b'r' as u32);
pub(crate) const ACS_S9: u32 = A_ALTCHARSET | (b's' as u32);
pub(crate) const ACS_DIAMOND: u32 = A_ALTCHARSET | (b'`' as u32);
pub(crate) const ACS_CKBOARD: u32 = A_ALTCHARSET | (b'a' as u32);
pub(crate) const ACS_DEGREE: u32 = A_ALTCHARSET | (b'f' as u32);
pub(crate) const ACS_PLMINUS: u32 = A_ALTCHARSET | (b'g' as u32);
pub(crate) const ACS_BULLET: u32 = A_ALTCHARSET | (b'~' as u32);
pub(crate) const ACS_LARROW: u32 = A_ALTCHARSET | (b',' as u32);
pub(crate) const ACS_RARROW: u32 = A_ALTCHARSET | (b'+' as u32);
pub(crate) const ACS_DARROW: u32 = A_ALTCHARSET | (b'.' as u32);
pub(crate) const ACS_UARROW: u32 = A_ALTCHARSET | (b'-' as u32);
pub(crate) const ACS_BOARD: u32 = A_ALTCHARSET | (b'h' as u32);
pub(crate) const ACS_LANTERN: u32 = A_ALTCHARSET | (b'i' as u32);
pub(crate) const ACS_BLOCK: u32 = A_ALTCHARSET | (b'0' as u32);
pub(crate) const ACS_LEQUAL: u32 = A_ALTCHARSET | (b'y' as u32);
pub(crate) const ACS_GEQUAL: u32 = A_ALTCHARSET | (b'z' as u32);
pub(crate) const ACS_PI: u32 = A_ALTCHARSET | (b'{' as u32);
pub(crate) const ACS_NEQUAL: u32 = A_ALTCHARSET | (b'|' as u32);
pub(crate) const ACS_STERLING: u32 = A_ALTCHARSET | (b'}' as u32);

pub(crate) const ACS_TABLE: &[(&str, u32)] = &[
    ("ACS_ULCORNER", ACS_ULCORNER),
    ("ACS_LLCORNER", ACS_LLCORNER),
    ("ACS_URCORNER", ACS_URCORNER),
    ("ACS_LRCORNER", ACS_LRCORNER),
    ("ACS_LTEE", ACS_LTEE),
    ("ACS_RTEE", ACS_RTEE),
    ("ACS_BTEE", ACS_BTEE),
    ("ACS_TTEE", ACS_TTEE),
    ("ACS_HLINE", ACS_HLINE),
    ("ACS_VLINE", ACS_VLINE),
    ("ACS_PLUS", ACS_PLUS),
    ("ACS_S1", ACS_S1),
    ("ACS_S3", ACS_S3),
    ("ACS_S7", ACS_S7),
    ("ACS_S9", ACS_S9),
    ("ACS_DIAMOND", ACS_DIAMOND),
    ("ACS_CKBOARD", ACS_CKBOARD),
    ("ACS_DEGREE", ACS_DEGREE),
    ("ACS_PLMINUS", ACS_PLMINUS),
    ("ACS_BULLET", ACS_BULLET),
    ("ACS_LARROW", ACS_LARROW),
    ("ACS_RARROW", ACS_RARROW),
    ("ACS_DARROW", ACS_DARROW),
    ("ACS_UARROW", ACS_UARROW),
    ("ACS_BOARD", ACS_BOARD),
    ("ACS_LANTERN", ACS_LANTERN),
    ("ACS_BLOCK", ACS_BLOCK),
    ("ACS_LEQUAL", ACS_LEQUAL),
    ("ACS_GEQUAL", ACS_GEQUAL),
    ("ACS_PI", ACS_PI),
    ("ACS_NEQUAL", ACS_NEQUAL),
    ("ACS_STERLING", ACS_STERLING),
    // Legacy aliases
    ("ACS_BSSB", ACS_ULCORNER),
    ("ACS_SSBB", ACS_LLCORNER),
    ("ACS_BBSS", ACS_URCORNER),
    ("ACS_SBBS", ACS_LRCORNER),
    ("ACS_SBSS", ACS_RTEE),
    ("ACS_SSSB", ACS_LTEE),
    ("ACS_SSBS", ACS_BTEE),
    ("ACS_BSSS", ACS_TTEE),
    ("ACS_BSBS", ACS_HLINE),
    ("ACS_SBSB", ACS_VLINE),
    ("ACS_SSSS", ACS_PLUS),
];

// ── Key constants (exact ncurses values) ─────────────────────────────────────

pub(crate) const KEY_MIN: i64 = 0o401;
pub(crate) const KEY_BREAK: i64 = 0o401;
pub(crate) const KEY_DOWN: i64 = 0o402;
pub(crate) const KEY_UP: i64 = 0o403;
pub(crate) const KEY_LEFT: i64 = 0o404;
pub(crate) const KEY_RIGHT: i64 = 0o405;
pub(crate) const KEY_HOME: i64 = 0o406;
pub(crate) const KEY_BACKSPACE: i64 = 0o407;
pub(crate) const KEY_F0: i64 = 0o410;
pub(crate) const KEY_DL: i64 = 0o510;
pub(crate) const KEY_IL: i64 = 0o511;
pub(crate) const KEY_DC: i64 = 0o512;
pub(crate) const KEY_IC: i64 = 0o513;
pub(crate) const KEY_EIC: i64 = 0o514;
pub(crate) const KEY_CLEAR: i64 = 0o515;
pub(crate) const KEY_EOS: i64 = 0o516;
pub(crate) const KEY_EOL: i64 = 0o517;
pub(crate) const KEY_SF: i64 = 0o520;
pub(crate) const KEY_SR: i64 = 0o521;
pub(crate) const KEY_NPAGE: i64 = 0o522;
pub(crate) const KEY_PPAGE: i64 = 0o523;
pub(crate) const KEY_STAB: i64 = 0o524;
pub(crate) const KEY_CTAB: i64 = 0o525;
pub(crate) const KEY_CATAB: i64 = 0o526;
pub(crate) const KEY_ENTER: i64 = 0o527;
pub(crate) const KEY_SRESET: i64 = 0o530;
pub(crate) const KEY_RESET: i64 = 0o531;
pub(crate) const KEY_PRINT: i64 = 0o532;
pub(crate) const KEY_LL: i64 = 0o533;
pub(crate) const KEY_A1: i64 = 0o534;
pub(crate) const KEY_A3: i64 = 0o535;
pub(crate) const KEY_B2: i64 = 0o536;
pub(crate) const KEY_C1: i64 = 0o537;
pub(crate) const KEY_C3: i64 = 0o540;
pub(crate) const KEY_BTAB: i64 = 0o541;
pub(crate) const KEY_BEG: i64 = 0o542;
pub(crate) const KEY_CANCEL: i64 = 0o543;
pub(crate) const KEY_CLOSE: i64 = 0o544;
pub(crate) const KEY_COMMAND: i64 = 0o545;
pub(crate) const KEY_COPY: i64 = 0o546;
pub(crate) const KEY_CREATE: i64 = 0o547;
pub(crate) const KEY_END: i64 = 0o550;
pub(crate) const KEY_EXIT: i64 = 0o551;
pub(crate) const KEY_FIND: i64 = 0o552;
pub(crate) const KEY_HELP: i64 = 0o553;
pub(crate) const KEY_MARK: i64 = 0o554;
pub(crate) const KEY_MESSAGE: i64 = 0o555;
pub(crate) const KEY_MOVE: i64 = 0o556;
pub(crate) const KEY_NEXT: i64 = 0o557;
pub(crate) const KEY_OPEN: i64 = 0o560;
pub(crate) const KEY_OPTIONS: i64 = 0o561;
pub(crate) const KEY_PREVIOUS: i64 = 0o562;
pub(crate) const KEY_REDO: i64 = 0o563;
pub(crate) const KEY_REFERENCE: i64 = 0o564;
pub(crate) const KEY_REFRESH: i64 = 0o565;
pub(crate) const KEY_REPLACE: i64 = 0o566;
pub(crate) const KEY_RESTART: i64 = 0o567;
pub(crate) const KEY_RESUME: i64 = 0o570;
pub(crate) const KEY_SAVE: i64 = 0o571;
pub(crate) const KEY_SBEG: i64 = 0o572;
pub(crate) const KEY_SCANCEL: i64 = 0o573;
pub(crate) const KEY_SCOMMAND: i64 = 0o574;
pub(crate) const KEY_SCOPY: i64 = 0o575;
pub(crate) const KEY_SCREATE: i64 = 0o576;
pub(crate) const KEY_SDC: i64 = 0o577;
pub(crate) const KEY_SDL: i64 = 0o600;
pub(crate) const KEY_SELECT: i64 = 0o601;
pub(crate) const KEY_SEND: i64 = 0o602;
pub(crate) const KEY_SEOL: i64 = 0o603;
pub(crate) const KEY_SEXIT: i64 = 0o604;
pub(crate) const KEY_SFIND: i64 = 0o605;
pub(crate) const KEY_SHELP: i64 = 0o606;
pub(crate) const KEY_SHOME: i64 = 0o607;
pub(crate) const KEY_SIC: i64 = 0o610;
pub(crate) const KEY_SLEFT: i64 = 0o611;
pub(crate) const KEY_SMESSAGE: i64 = 0o612;
pub(crate) const KEY_SMOVE: i64 = 0o613;
pub(crate) const KEY_SNEXT: i64 = 0o614;
pub(crate) const KEY_SOPTIONS: i64 = 0o615;
pub(crate) const KEY_SPREVIOUS: i64 = 0o616;
pub(crate) const KEY_SPRINT: i64 = 0o617;
pub(crate) const KEY_SRSUME: i64 = 0o623;
pub(crate) const KEY_SREDO: i64 = 0o620;
pub(crate) const KEY_SREPLACE: i64 = 0o621;
pub(crate) const KEY_SRIGHT: i64 = 0o622;
pub(crate) const KEY_SSAVE: i64 = 0o624;
pub(crate) const KEY_SSUSPEND: i64 = 0o625;
pub(crate) const KEY_SUNDO: i64 = 0o626;
pub(crate) const KEY_SUSPEND: i64 = 0o627;
pub(crate) const KEY_UNDO: i64 = 0o630;
pub(crate) const KEY_MOUSE: i64 = 0o631;
pub(crate) const KEY_RESIZE: i64 = 0o632;
pub(crate) const KEY_MAX: i64 = 0o777;

// ── Mouse constants (ncurses 6 ABI) ──────────────────────────────────────────

pub(crate) const BUTTON1_RELEASED: i64 = 0o000001;
pub(crate) const BUTTON1_PRESSED: i64 = 0o000002;
pub(crate) const BUTTON1_CLICKED: i64 = 0o000004;
pub(crate) const BUTTON1_DOUBLE_CLICKED: i64 = 0o000010;
pub(crate) const BUTTON1_TRIPLE_CLICKED: i64 = 0o000020;
pub(crate) const BUTTON2_RELEASED: i64 = 0o000100;
pub(crate) const BUTTON2_PRESSED: i64 = 0o000200;
pub(crate) const BUTTON2_CLICKED: i64 = 0o000400;
pub(crate) const BUTTON2_DOUBLE_CLICKED: i64 = 0o001000;
pub(crate) const BUTTON2_TRIPLE_CLICKED: i64 = 0o002000;
pub(crate) const BUTTON3_RELEASED: i64 = 0o010000;
pub(crate) const BUTTON3_PRESSED: i64 = 0o020000;
pub(crate) const BUTTON3_CLICKED: i64 = 0o040000;
pub(crate) const BUTTON3_DOUBLE_CLICKED: i64 = 0o100000;
pub(crate) const BUTTON3_TRIPLE_CLICKED: i64 = 0o200000;
pub(crate) const BUTTON4_RELEASED: i64 = 0o1000000;
pub(crate) const BUTTON4_PRESSED: i64 = 0o2000000;
pub(crate) const BUTTON4_CLICKED: i64 = 0o4000000;
pub(crate) const BUTTON4_DOUBLE_CLICKED: i64 = 0o10000000;
pub(crate) const BUTTON4_TRIPLE_CLICKED: i64 = 0o20000000;
pub(crate) const BUTTON_SHIFT: i64 = 0o0200000000;
pub(crate) const BUTTON_CTRL: i64 = 0o0100000000;
pub(crate) const BUTTON_ALT: i64 = 0o0400000000;
pub(crate) const ALL_MOUSE_EVENTS: i64 = 0o0777777777;
pub(crate) const REPORT_MOUSE_POSITION: i64 = 0o1000000000;

/// Mapping from `KEY_*` constant to terminfo capability name (mirrors
/// CPython's `Lib/curses/has_key.py`).
pub(crate) const KEY_CAPABILITY_TABLE: &[(i64, &str)] = &[
    (KEY_A1, "ka1"),
    (KEY_A3, "ka3"),
    (KEY_B2, "kb2"),
    (KEY_BACKSPACE, "kbs"),
    (KEY_BEG, "kbeg"),
    (KEY_BTAB, "kcbt"),
    (KEY_C1, "kc1"),
    (KEY_C3, "kc3"),
    (KEY_CANCEL, "kcan"),
    (KEY_CATAB, "ktbc"),
    (KEY_CLEAR, "kclr"),
    (KEY_CLOSE, "kclo"),
    (KEY_COMMAND, "kcmd"),
    (KEY_COPY, "kcpy"),
    (KEY_CREATE, "kcrt"),
    (KEY_CTAB, "kctab"),
    (KEY_DC, "kdch1"),
    (KEY_DL, "kdl1"),
    (KEY_DOWN, "kcud1"),
    (KEY_EIC, "krmir"),
    (KEY_END, "kend"),
    (KEY_ENTER, "kent"),
    (KEY_EOL, "kel"),
    (KEY_EOS, "ked"),
    (KEY_EXIT, "kext"),
    (KEY_F0, "kf0"),
    (KEY_F0 + 1, "kf1"),
    (KEY_F0 + 2, "kf2"),
    (KEY_F0 + 3, "kf3"),
    (KEY_F0 + 4, "kf4"),
    (KEY_F0 + 5, "kf5"),
    (KEY_F0 + 6, "kf6"),
    (KEY_F0 + 7, "kf7"),
    (KEY_F0 + 8, "kf8"),
    (KEY_F0 + 9, "kf9"),
    (KEY_F0 + 10, "kf10"),
    (KEY_F0 + 11, "kf11"),
    (KEY_F0 + 12, "kf12"),
    (KEY_F0 + 13, "kf13"),
    (KEY_F0 + 14, "kf14"),
    (KEY_F0 + 15, "kf15"),
    (KEY_F0 + 16, "kf16"),
    (KEY_F0 + 17, "kf17"),
    (KEY_F0 + 18, "kf18"),
    (KEY_F0 + 19, "kf19"),
    (KEY_F0 + 20, "kf20"),
    (KEY_F0 + 21, "kf21"),
    (KEY_F0 + 22, "kf22"),
    (KEY_F0 + 23, "kf23"),
    (KEY_F0 + 24, "kf24"),
    (KEY_F0 + 25, "kf25"),
    (KEY_F0 + 26, "kf26"),
    (KEY_F0 + 27, "kf27"),
    (KEY_F0 + 28, "kf28"),
    (KEY_F0 + 29, "kf29"),
    (KEY_F0 + 30, "kf30"),
    (KEY_F0 + 31, "kf31"),
    (KEY_F0 + 32, "kf32"),
    (KEY_F0 + 33, "kf33"),
    (KEY_F0 + 34, "kf34"),
    (KEY_F0 + 35, "kf35"),
    (KEY_F0 + 36, "kf36"),
    (KEY_F0 + 37, "kf37"),
    (KEY_F0 + 38, "kf38"),
    (KEY_F0 + 39, "kf39"),
    (KEY_F0 + 40, "kf40"),
    (KEY_F0 + 41, "kf41"),
    (KEY_F0 + 42, "kf42"),
    (KEY_F0 + 43, "kf43"),
    (KEY_F0 + 44, "kf44"),
    (KEY_F0 + 45, "kf45"),
    (KEY_F0 + 46, "kf46"),
    (KEY_F0 + 47, "kf47"),
    (KEY_F0 + 48, "kf48"),
    (KEY_F0 + 49, "kf49"),
    (KEY_F0 + 50, "kf50"),
    (KEY_F0 + 51, "kf51"),
    (KEY_F0 + 52, "kf52"),
    (KEY_F0 + 53, "kf53"),
    (KEY_F0 + 54, "kf54"),
    (KEY_F0 + 55, "kf55"),
    (KEY_F0 + 56, "kf56"),
    (KEY_F0 + 57, "kf57"),
    (KEY_F0 + 58, "kf58"),
    (KEY_F0 + 59, "kf59"),
    (KEY_F0 + 60, "kf60"),
    (KEY_F0 + 61, "kf61"),
    (KEY_F0 + 62, "kf62"),
    (KEY_F0 + 63, "kf63"),
    (KEY_FIND, "kfnd"),
    (KEY_HELP, "khlp"),
    (KEY_HOME, "khome"),
    (KEY_IC, "kich1"),
    (KEY_IL, "kil1"),
    (KEY_LEFT, "kcub1"),
    (KEY_LL, "kll"),
    (KEY_MARK, "kmrk"),
    (KEY_MESSAGE, "kmsg"),
    (KEY_MOVE, "kmov"),
    (KEY_NEXT, "knxt"),
    (KEY_NPAGE, "knp"),
    (KEY_OPEN, "kopn"),
    (KEY_OPTIONS, "kopt"),
    (KEY_PPAGE, "kpp"),
    (KEY_PREVIOUS, "kprv"),
    (KEY_PRINT, "kprt"),
    (KEY_REDO, "krdo"),
    (KEY_REFERENCE, "kref"),
    (KEY_REFRESH, "krfr"),
    (KEY_REPLACE, "krpl"),
    (KEY_RESTART, "krst"),
    (KEY_RESUME, "kres"),
    (KEY_RIGHT, "kcuf1"),
    (KEY_SAVE, "ksav"),
    (KEY_SBEG, "kBEG"),
    (KEY_SCANCEL, "kCAN"),
    (KEY_SCOMMAND, "kCMD"),
    (KEY_SCOPY, "kCPY"),
    (KEY_SCREATE, "kCRT"),
    (KEY_SDC, "kDC"),
    (KEY_SDL, "kDL"),
    (KEY_SELECT, "kslt"),
    (KEY_SEND, "kEND"),
    (KEY_SEOL, "kEOL"),
    (KEY_SEXIT, "kEXT"),
    (KEY_SF, "kind"),
    (KEY_SFIND, "kFND"),
    (KEY_SHELP, "kHLP"),
    (KEY_SHOME, "kHOM"),
    (KEY_SIC, "kIC"),
    (KEY_SLEFT, "kLFT"),
    (KEY_SMESSAGE, "kMSG"),
    (KEY_SMOVE, "kMOV"),
    (KEY_SNEXT, "kNXT"),
    (KEY_SOPTIONS, "kOPT"),
    (KEY_SPREVIOUS, "kPRV"),
    (KEY_SPRINT, "kPRT"),
    (KEY_SR, "kri"),
    (KEY_SREDO, "kRDO"),
    (KEY_SREPLACE, "kRPL"),
    (KEY_SRIGHT, "kRIT"),
    (KEY_SRSUME, "kRES"),
    (KEY_SSAVE, "kSAV"),
    (KEY_SSUSPEND, "kSPD"),
    (KEY_STAB, "khts"),
    (KEY_SUNDO, "kUND"),
    (KEY_SUSPEND, "kspd"),
    (KEY_UNDO, "kund"),
    (KEY_UP, "kcuu1"),
];

// ── Character cell & window representation ───────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) attr: u32,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            attr: A_NORMAL,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WinData {
    pub(crate) id: usize,
    pub(crate) height: usize,
    pub(crate) width: usize,
    pub(crate) beg_y: usize,
    pub(crate) beg_x: usize,
    pub(crate) parent_id: Option<usize>,
    pub(crate) par_y: usize,
    pub(crate) par_x: usize,
    pub(crate) is_pad: bool,
    pub(crate) cur_y: usize,
    pub(crate) cur_x: usize,
    pub(crate) attrs: u32,
    pub(crate) bkgd: Cell,
    pub(crate) cells: Vec<Vec<Cell>>,
    pub(crate) touched: Vec<bool>,
    pub(crate) keypad: bool,
    pub(crate) nodelay: bool,
    pub(crate) timeout_ms: i32,
    pub(crate) leaveok: bool,
    pub(crate) scrollok: bool,
    pub(crate) idlok: bool,
    pub(crate) immedok: bool,
    pub(crate) syncok: bool,
    pub(crate) scroll_top: usize,
    pub(crate) scroll_bot: usize,
    pub(crate) encoding: String,
}

impl WinData {
    fn new_root(
        id: usize,
        height: usize,
        width: usize,
        beg_y: usize,
        beg_x: usize,
        is_pad: bool,
    ) -> Self {
        let h = height.max(1);
        let w = width.max(1);
        let cells = vec![vec![Cell::default(); w]; h];
        let touched = vec![true; h];
        Self {
            id,
            height: h,
            width: w,
            beg_y,
            beg_x,
            parent_id: None,
            par_y: 0,
            par_x: 0,
            is_pad,
            cur_y: 0,
            cur_x: 0,
            attrs: A_NORMAL,
            bkgd: Cell::default(),
            cells,
            touched,
            keypad: false,
            nodelay: false,
            timeout_ms: -1,
            leaveok: false,
            scrollok: false,
            idlok: false,
            immedok: false,
            syncok: false,
            scroll_top: 0,
            scroll_bot: h - 1,
            encoding: "utf-8".to_string(),
        }
    }

    fn new_sub(
        id: usize,
        parent_id: usize,
        size: (usize, usize),
        beg: (usize, usize),
        par: (usize, usize),
    ) -> Self {
        let h = size.0.max(1);
        let w = size.1.max(1);
        Self {
            id,
            height: h,
            width: w,
            beg_y: beg.0,
            beg_x: beg.1,
            parent_id: Some(parent_id),
            par_y: par.0,
            par_x: par.1,
            is_pad: false,
            cur_y: 0,
            cur_x: 0,
            attrs: A_NORMAL,
            bkgd: Cell::default(),
            // Subwindows share their root ancestor's cell grid.
            cells: Vec::new(),
            touched: vec![true; h],
            keypad: false,
            nodelay: false,
            timeout_ms: -1,
            leaveok: false,
            scrollok: false,
            idlok: false,
            immedok: false,
            syncok: false,
            scroll_top: 0,
            scroll_bot: h - 1,
            encoding: "utf-8".to_string(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct PanelEntry {
    pub(crate) id: usize,
    pub(crate) win_id: usize,
    pub(crate) win_val: Value,
    pub(crate) panel_val: Value,
    pub(crate) hidden: bool,
    pub(crate) userptr: Option<Value>,
}

// ── Global per-thread curses state ───────────────────────────────────────────

pub(crate) struct CursesState {
    pub(crate) setupterm_done: bool,
    pub(crate) initialized: bool,
    pub(crate) ended: bool,
    pub(crate) term: String,
    pub(crate) out_fd: i32,
    pub(crate) lines: usize,
    pub(crate) cols: usize,
    pub(crate) colors_started: bool,
    pub(crate) default_colors: bool,
    pub(crate) max_colors: i32,
    pub(crate) max_pairs: i32,
    pub(crate) pairs: HashMap<i32, (i32, i32)>,
    pub(crate) palette: HashMap<i32, (i32, i32, i32)>,
    pub(crate) cbreak: bool,
    pub(crate) raw: bool,
    pub(crate) echo: bool,
    pub(crate) nl: bool,
    pub(crate) cursor_vis: i32,
    pub(crate) syx: (i32, i32),
    pub(crate) stdscr_id: Option<usize>,
    pub(crate) windows: HashMap<usize, WinData>,
    pub(crate) next_win_id: usize,
    pub(crate) virtual_screen: Vec<Vec<Cell>>,
    pub(crate) panels: Vec<PanelEntry>,
    pub(crate) next_panel_id: usize,
    pub(crate) ungetch_buf: VecDeque<i32>,
    #[cfg(unix)]
    pub(crate) shell_termios: Option<libc::termios>,
    #[cfg(unix)]
    pub(crate) prog_termios: Option<libc::termios>,
    #[cfg(unix)]
    pub(crate) saved_tty: Option<libc::termios>,
}

impl Default for CursesState {
    fn default() -> Self {
        let mut palette = HashMap::new();
        // Standard 8-color RGB palette (0..=1000 scale).
        palette.insert(0, (0, 0, 0));
        palette.insert(1, (680, 0, 0));
        palette.insert(2, (0, 680, 0));
        palette.insert(3, (680, 680, 0));
        palette.insert(4, (0, 0, 680));
        palette.insert(5, (680, 0, 680));
        palette.insert(6, (0, 680, 680));
        palette.insert(7, (680, 680, 680));

        let mut pairs = HashMap::new();
        pairs.insert(0, (COLOR_WHITE as i32, COLOR_BLACK as i32));

        Self {
            setupterm_done: false,
            initialized: false,
            ended: false,
            term: String::new(),
            out_fd: 1,
            lines: 24,
            cols: 80,
            colors_started: false,
            default_colors: false,
            max_colors: 0,
            max_pairs: 0,
            pairs,
            palette,
            cbreak: false,
            raw: false,
            echo: true,
            nl: true,
            cursor_vis: 1,
            syx: (-1, -1),
            stdscr_id: None,
            windows: HashMap::new(),
            next_win_id: 1,
            virtual_screen: vec![vec![Cell::default(); 80]; 24],
            panels: Vec::new(),
            next_panel_id: 1,
            ungetch_buf: VecDeque::new(),
            #[cfg(unix)]
            shell_termios: None,
            #[cfg(unix)]
            prog_termios: None,
            #[cfg(unix)]
            saved_tty: None,
        }
    }
}

thread_local! {
    static CURSES_STATE: RefCell<CursesState> = RefCell::new(CursesState::default());
}

pub(crate) fn with_state<R>(f: impl FnOnce(&CursesState) -> R) -> R {
    CURSES_STATE.with(|c| f(&c.borrow()))
}

pub(crate) fn with_state_mut<R>(f: impl FnOnce(&mut CursesState) -> R) -> R {
    CURSES_STATE.with(|c| f(&mut c.borrow_mut()))
}

// ── Terminal profile & pure-Rust terminfo database ───────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum TermProfile {
    Xterm256,
    Xterm8,
    Vt100,
    Dumb,
}

fn classify_term(term: &str) -> Option<TermProfile> {
    let t = term.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("unknown") {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if lower == "dumb" {
        return Some(TermProfile::Dumb);
    }
    if lower.contains("256color") || lower.contains("direct") || lower.contains("truecolor") {
        return Some(TermProfile::Xterm256);
    }
    if lower.starts_with("xterm")
        || lower.starts_with("screen")
        || lower.starts_with("tmux")
        || lower.starts_with("rxvt")
        || lower.starts_with("linux")
        || lower.starts_with("ansi")
        || lower.starts_with("alacritty")
        || lower.starts_with("kitty")
        || lower.starts_with("foot")
        || lower.starts_with("wezterm")
        || lower.starts_with("gnome")
        || lower.starts_with("konsole")
        || lower.starts_with("st")
        || lower.starts_with("putty")
        || lower.starts_with("cygwin")
        || lower.starts_with("eterm")
    {
        return Some(TermProfile::Xterm8);
    }
    if lower.starts_with("vt100")
        || lower.starts_with("vt102")
        || lower.starts_with("vt220")
        || lower.starts_with("vt320")
        || lower.starts_with("vt52")
    {
        return Some(TermProfile::Vt100);
    }
    None
}

pub(crate) fn query_terminal_size(fd: i32) -> (usize, usize) {
    #[cfg(unix)]
    {
        // SAFETY: `winsize` is POD and passed to `TIOCGWINSZ` ioctl.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if fd >= 0
            && unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0
            && ws.ws_row > 0
            && ws.ws_col > 0
        {
            return (ws.ws_row as usize, ws.ws_col as usize);
        }
    }
    let _ = fd;
    let lines = std::env::var("LINES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(24);
    let cols = std::env::var("COLS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(80);
    (lines, cols)
}

impl CursesState {
    pub(crate) fn setupterm(&mut self, term: Option<&str>, fd: i32) -> Result<(), String> {
        let term_name = match term {
            Some(t) => t.to_string(),
            None => std::env::var("TERM").unwrap_or_else(|_| "unknown".to_string()),
        };
        let profile = classify_term(&term_name)
            .ok_or_else(|| "setupterm: could not find terminal".to_string())?;
        self.term = term_name;
        self.out_fd = if fd < 0 { 1 } else { fd };
        let (lines, cols) = query_terminal_size(self.out_fd);
        self.lines = lines;
        self.cols = cols;
        match profile {
            TermProfile::Xterm256 => {
                self.max_colors = 256;
                self.max_pairs = 65536;
            }
            TermProfile::Xterm8 => {
                self.max_colors = 8;
                self.max_pairs = 64;
            }
            TermProfile::Vt100 | TermProfile::Dumb => {
                self.max_colors = 0;
                self.max_pairs = 0;
            }
        }
        self.virtual_screen = vec![vec![Cell::default(); cols]; lines];
        self.setupterm_done = true;
        Ok(())
    }

    pub(crate) fn initscr(&mut self) -> Result<usize, String> {
        if !self.setupterm_done {
            let term = std::env::var("TERM").unwrap_or_else(|_| "unknown".to_string());
            self.setupterm(Some(&term), 1)?;
        }
        let (lines, cols) = query_terminal_size(self.out_fd);
        self.lines = lines;
        self.cols = cols;
        self.virtual_screen = vec![vec![Cell::default(); cols]; lines];

        #[cfg(unix)]
        {
            // Save initial shell termios if attached to a tty.
            // SAFETY: `termios` is POD and `tcgetattr` fails cleanly with ENOTTY on pipes.
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(0, &mut mode) } == 0 {
                if self.shell_termios.is_none() {
                    self.shell_termios = Some(mode);
                }
                self.prog_termios = Some(mode);
            }
        }

        self.initialized = true;
        self.ended = false;
        self.syx = (0, 0);

        if let Some(id) = self.stdscr_id
            && let Some(win) = self.windows.get_mut(&id)
        {
            win.height = lines;
            win.width = cols;
            win.cells = vec![vec![Cell::default(); cols]; lines];
            win.touched = vec![true; lines];
            win.cur_y = 0;
            win.cur_x = 0;
            win.scroll_top = 0;
            win.scroll_bot = lines.saturating_sub(1);
            return Ok(id);
        }

        let id = self.alloc_window(lines, cols, 0, 0, false);
        self.stdscr_id = Some(id);
        Ok(id)
    }

    pub(crate) fn endwin(&mut self) -> Result<(), String> {
        if !self.initialized {
            return Err("must call initscr() first".to_string());
        }
        self.ended = true;
        #[cfg(unix)]
        {
            if let Some(orig) = self.shell_termios {
                // SAFETY: restoring previously captured shell termios.
                unsafe {
                    let _ = libc::tcsetattr(0, libc::TCSANOW, &orig);
                }
            }
            if unsafe { libc::isatty(self.out_fd) } == 1 {
                let reset = b"\x1b[0m\x1b[?25h";
                unsafe {
                    let _ = libc::write(self.out_fd, reset.as_ptr().cast(), reset.len());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn alloc_window(
        &mut self,
        nlines: usize,
        ncols: usize,
        begin_y: usize,
        begin_x: usize,
        is_pad: bool,
    ) -> usize {
        let id = self.next_win_id;
        self.next_win_id += 1;
        let h = if nlines == 0 {
            self.lines.saturating_sub(begin_y).max(1)
        } else {
            nlines
        };
        let w = if ncols == 0 {
            self.cols.saturating_sub(begin_x).max(1)
        } else {
            ncols
        };
        let win = WinData::new_root(id, h, w, begin_y, begin_x, is_pad);
        self.windows.insert(id, win);
        id
    }

    pub(crate) fn alloc_subwin(
        &mut self,
        parent_id: usize,
        nlines: usize,
        ncols: usize,
        begin_y: usize,
        begin_x: usize,
        is_derwin: bool,
    ) -> Result<usize, String> {
        let (p_beg_y, p_beg_x, p_h, p_w) = {
            let p = self
                .windows
                .get(&parent_id)
                .ok_or_else(|| "invalid window".to_string())?;
            (p.beg_y, p.beg_x, p.height, p.width)
        };
        let (abs_y, abs_x, par_y, par_x) = if is_derwin {
            (p_beg_y + begin_y, p_beg_x + begin_x, begin_y, begin_x)
        } else {
            if begin_y < p_beg_y || begin_x < p_beg_x {
                return Err("curses function returned NULL".to_string());
            }
            (begin_y, begin_x, begin_y - p_beg_y, begin_x - p_beg_x)
        };
        if par_y >= p_h || par_x >= p_w {
            return Err("curses function returned NULL".to_string());
        }
        let h = if nlines == 0 { p_h - par_y } else { nlines };
        let w = if ncols == 0 { p_w - par_x } else { ncols };
        if h == 0 || w == 0 || par_y + h > p_h || par_x + w > p_w {
            return Err("curses function returned NULL".to_string());
        }
        let id = self.next_win_id;
        self.next_win_id += 1;
        let win = WinData::new_sub(id, parent_id, (h, w), (abs_y, abs_x), (par_y, par_x));
        self.windows.insert(id, win);
        Ok(id)
    }

    /// Resolve `(win_id, y, x)` to `(root_win_id, root_y, root_x)` so subwindows
    /// share storage directly with their root ancestor.
    pub(crate) fn resolve_cell_coords(
        &self,
        mut win_id: usize,
        mut y: usize,
        mut x: usize,
    ) -> Option<(usize, usize, usize)> {
        let mut depth = 0;
        while depth < 64 {
            let win = self.windows.get(&win_id)?;
            if let Some(pid) = win.parent_id {
                y += win.par_y;
                x += win.par_x;
                win_id = pid;
                depth += 1;
            } else {
                return Some((win_id, y, x));
            }
        }
        None
    }

    pub(crate) fn get_cell(&self, win_id: usize, y: usize, x: usize) -> Option<Cell> {
        let (root_id, ry, rx) = self.resolve_cell_coords(win_id, y, x)?;
        let root = self.windows.get(&root_id)?;
        root.cells.get(ry)?.get(rx).copied()
    }

    pub(crate) fn set_cell(&mut self, win_id: usize, y: usize, x: usize, cell: Cell) -> bool {
        let Some((root_id, ry, rx)) = self.resolve_cell_coords(win_id, y, x) else {
            return false;
        };
        if let Some(root) = self.windows.get_mut(&root_id)
            && ry < root.cells.len()
            && rx < root.cells[ry].len()
        {
            root.cells[ry][rx] = cell;
            if ry < root.touched.len() {
                root.touched[ry] = true;
            }
        } else {
            return false;
        }
        if let Some(win) = self.windows.get_mut(&win_id)
            && y < win.touched.len()
        {
            win.touched[y] = true;
        }
        true
    }

    pub(crate) fn win_clear(&mut self, win_id: usize) {
        let Some((h, w, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.height, win.width, win.bkgd))
        else {
            return;
        };
        for y in 0..h {
            for x in 0..w {
                self.set_cell(win_id, y, x, bkgd);
            }
        }
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.cur_y = 0;
            win.cur_x = 0;
            win.touched.fill(true);
        }
    }

    pub(crate) fn win_clrtoeol(&mut self, win_id: usize) {
        let Some((cy, cx, w, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.cur_y, win.cur_x, win.width, win.bkgd))
        else {
            return;
        };
        for x in cx..w {
            self.set_cell(win_id, cy, x, bkgd);
        }
    }

    pub(crate) fn win_clrtobot(&mut self, win_id: usize) {
        let Some((cy, cx, h, w, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.cur_y, win.cur_x, win.height, win.width, win.bkgd))
        else {
            return;
        };
        for x in cx..w {
            self.set_cell(win_id, cy, x, bkgd);
        }
        for y in (cy + 1)..h {
            for x in 0..w {
                self.set_cell(win_id, y, x, bkgd);
            }
        }
    }

    pub(crate) fn win_scroll(&mut self, win_id: usize, lines: i32) -> Result<(), String> {
        let (h, w, top, bot, bkgd, scrollok) = {
            let win = self
                .windows
                .get(&win_id)
                .ok_or_else(|| "invalid window".to_string())?;
            (
                win.height,
                win.width,
                win.scroll_top.min(win.height.saturating_sub(1)),
                win.scroll_bot.min(win.height.saturating_sub(1)),
                win.bkgd,
                win.scrollok,
            )
        };
        if !scrollok {
            return Err("scroll() returned ERR".to_string());
        }
        if lines == 0 || top > bot || h == 0 {
            return Ok(());
        }
        let region_len = bot - top + 1;
        if lines > 0 {
            let shift = (lines as usize).min(region_len);
            for y in top..=(bot.saturating_sub(shift)) {
                for x in 0..w {
                    let c = self.get_cell(win_id, y + shift, x).unwrap_or(bkgd);
                    self.set_cell(win_id, y, x, c);
                }
            }
            for y in (bot + 1 - shift)..=bot {
                for x in 0..w {
                    self.set_cell(win_id, y, x, bkgd);
                }
            }
        } else {
            let shift = (lines.unsigned_abs() as usize).min(region_len);
            for y in ((top + shift)..=bot).rev() {
                for x in 0..w {
                    let c = self.get_cell(win_id, y - shift, x).unwrap_or(bkgd);
                    self.set_cell(win_id, y, x, c);
                }
            }
            for y in top..(top + shift) {
                for x in 0..w {
                    self.set_cell(win_id, y, x, bkgd);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn win_put_char(
        &mut self,
        win_id: usize,
        ch: char,
        extra_attr: u32,
    ) -> Result<(), String> {
        let (h, w, cy, cx, win_attrs, bkgd, scrollok, scroll_bot) = {
            let win = self
                .windows
                .get(&win_id)
                .ok_or_else(|| "invalid window".to_string())?;
            (
                win.height,
                win.width,
                win.cur_y,
                win.cur_x,
                win.attrs,
                win.bkgd,
                win.scrollok,
                win.scroll_bot,
            )
        };
        if h == 0 || w == 0 {
            return Err("addch() returned ERR".to_string());
        }
        match ch {
            '\n' => {
                self.win_clrtoeol(win_id);
                if cy == scroll_bot && scrollok {
                    let _ = self.win_scroll(win_id, 1);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.cur_x = 0;
                    }
                } else if cy + 1 < h {
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.cur_y = cy + 1;
                        win.cur_x = 0;
                    }
                } else if let Some(win) = self.windows.get_mut(&win_id) {
                    win.cur_x = 0;
                }
                return Ok(());
            }
            '\r' => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.cur_x = 0;
                }
                return Ok(());
            }
            '\x08' => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.cur_x = cx.saturating_sub(1);
                }
                return Ok(());
            }
            '\t' => {
                let next_tab = ((cx / 8) + 1) * 8;
                let spaces = next_tab.saturating_sub(cx);
                for _ in 0..spaces {
                    if self.windows.get(&win_id).is_some_and(|wn| wn.cur_x >= w) {
                        break;
                    }
                    self.win_put_char(win_id, ' ', extra_attr)?;
                }
                return Ok(());
            }
            _ => {}
        }

        let combined_attr = extra_attr | win_attrs | bkgd.attr;
        self.set_cell(
            win_id,
            cy,
            cx,
            Cell {
                ch,
                attr: combined_attr,
            },
        );

        if cx + 1 < w {
            if let Some(win) = self.windows.get_mut(&win_id) {
                win.cur_x = cx + 1;
            }
            Ok(())
        } else if cy + 1 < h {
            if let Some(win) = self.windows.get_mut(&win_id) {
                win.cur_y = cy + 1;
                win.cur_x = 0;
            }
            Ok(())
        } else if scrollok {
            let _ = self.win_scroll(win_id, 1);
            if let Some(win) = self.windows.get_mut(&win_id) {
                win.cur_y = h - 1;
                win.cur_x = 0;
            }
            Ok(())
        } else {
            // Matching ncurses: writing into the bottom-right cell of a non-scrolling
            // window writes the cell and returns ERR.
            Err("addwstr() returned ERR".to_string())
        }
    }

    pub(crate) fn win_delch(&mut self, win_id: usize) {
        let Some((cy, cx, w, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.cur_y, win.cur_x, win.width, win.bkgd))
        else {
            return;
        };
        if w == 0 || cx >= w {
            return;
        }
        for x in cx..(w - 1) {
            let next_c = self.get_cell(win_id, cy, x + 1).unwrap_or(bkgd);
            self.set_cell(win_id, cy, x, next_c);
        }
        self.set_cell(win_id, cy, w - 1, bkgd);
    }

    pub(crate) fn win_insch(&mut self, win_id: usize, ch: char, attr: u32) {
        let Some((cy, cx, w, win_attrs, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.cur_y, win.cur_x, win.width, win.attrs, win.bkgd))
        else {
            return;
        };
        if w == 0 || cx >= w {
            return;
        }
        for x in ((cx + 1)..w).rev() {
            let prev_c = self.get_cell(win_id, cy, x - 1).unwrap_or(bkgd);
            self.set_cell(win_id, cy, x, prev_c);
        }
        self.set_cell(
            win_id,
            cy,
            cx,
            Cell {
                ch,
                attr: attr | win_attrs | bkgd.attr,
            },
        );
    }

    pub(crate) fn win_deleteln(&mut self, win_id: usize) {
        let Some((cy, h, w, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.cur_y, win.height, win.width, win.bkgd))
        else {
            return;
        };
        if h == 0 || cy >= h {
            return;
        }
        for y in cy..(h - 1) {
            for x in 0..w {
                let c = self.get_cell(win_id, y + 1, x).unwrap_or(bkgd);
                self.set_cell(win_id, y, x, c);
            }
        }
        for x in 0..w {
            self.set_cell(win_id, h - 1, x, bkgd);
        }
    }

    pub(crate) fn win_insertln(&mut self, win_id: usize) {
        let Some((cy, h, w, bkgd)) = self
            .windows
            .get(&win_id)
            .map(|win| (win.cur_y, win.height, win.width, win.bkgd))
        else {
            return;
        };
        if h == 0 || cy >= h {
            return;
        }
        for y in ((cy + 1)..h).rev() {
            for x in 0..w {
                let c = self.get_cell(win_id, y - 1, x).unwrap_or(bkgd);
                self.set_cell(win_id, y, x, c);
            }
        }
        for x in 0..w {
            self.set_cell(win_id, cy, x, bkgd);
        }
    }

    pub(crate) fn win_noutrefresh(
        &mut self,
        win_id: usize,
        pad_rect: Option<(usize, usize, usize, usize, usize, usize)>,
    ) -> Result<(), String> {
        let (h, w, beg_y, beg_x, cy, cx, leaveok, is_pad) = {
            let win = self
                .windows
                .get(&win_id)
                .ok_or_else(|| "invalid window".to_string())?;
            (
                win.height,
                win.width,
                win.beg_y,
                win.beg_x,
                win.cur_y,
                win.cur_x,
                win.leaveok,
                win.is_pad,
            )
        };
        if is_pad {
            let Some((pminrow, pmincol, sminrow, smincol, smaxrow, smaxcol)) = pad_rect else {
                return Err("pad refresh requires 6 arguments".to_string());
            };
            if sminrow > smaxrow || smincol > smaxcol {
                return Err("prefresh() returned ERR".to_string());
            }
            let max_rows = (smaxrow - sminrow + 1)
                .min(h.saturating_sub(pminrow))
                .min(self.lines.saturating_sub(sminrow));
            let max_cols = (smaxcol - smincol + 1)
                .min(w.saturating_sub(pmincol))
                .min(self.cols.saturating_sub(smincol));
            for dy in 0..max_rows {
                for dx in 0..max_cols {
                    if let Some(cell) = self.get_cell(win_id, pminrow + dy, pmincol + dx) {
                        self.virtual_screen[sminrow + dy][smincol + dx] = cell;
                    }
                }
            }
        } else {
            for y in 0..h {
                let sy = beg_y + y;
                if sy >= self.lines {
                    break;
                }
                for x in 0..w {
                    let sx = beg_x + x;
                    if sx >= self.cols {
                        break;
                    }
                    if let Some(cell) = self.get_cell(win_id, y, x) {
                        self.virtual_screen[sy][sx] = cell;
                    }
                }
            }
            if !leaveok {
                self.syx = ((beg_y + cy) as i32, (beg_x + cx) as i32);
            }
        }
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.touched.fill(false);
        }
        Ok(())
    }

    pub(crate) fn doupdate(&mut self) {
        self.ended = false;
        #[cfg(unix)]
        {
            if unsafe { libc::isatty(self.out_fd) } == 1 {
                let mut out = String::new();
                for (row_idx, row) in self.virtual_screen.iter().enumerate() {
                    out.push_str(&format!("\x1b[{};1H", row_idx + 1));
                    let mut cur_attr: u32 = A_NORMAL;
                    for cell in row {
                        if cell.attr != cur_attr {
                            self.emit_sgr(&mut out, cell.attr);
                            cur_attr = cell.attr;
                        }
                        if (cell.attr & A_ALTCHARSET) != 0 {
                            out.push(acs_to_unicode(cell.ch));
                        } else {
                            out.push(cell.ch);
                        }
                    }
                    if cur_attr != A_NORMAL {
                        out.push_str("\x1b[0m");
                    }
                }
                if self.syx.0 >= 0 && self.syx.1 >= 0 {
                    out.push_str(&format!("\x1b[{};{}H", self.syx.0 + 1, self.syx.1 + 1));
                }
                let bytes = out.as_bytes();
                unsafe {
                    let _ = libc::write(self.out_fd, bytes.as_ptr().cast(), bytes.len());
                }
            }
        }
    }

    #[cfg(unix)]
    fn emit_sgr(&self, out: &mut String, attr: u32) {
        out.push_str("\x1b[0");
        if (attr & A_BOLD) != 0 {
            out.push_str(";1");
        }
        if (attr & A_DIM) != 0 {
            out.push_str(";2");
        }
        if (attr & A_ITALIC) != 0 {
            out.push_str(";3");
        }
        if (attr & A_UNDERLINE) != 0 {
            out.push_str(";4");
        }
        if (attr & A_BLINK) != 0 {
            out.push_str(";5");
        }
        if (attr & (A_REVERSE | A_STANDOUT)) != 0 {
            out.push_str(";7");
        }
        if (attr & A_INVIS) != 0 {
            out.push_str(";8");
        }
        let pair_num = ((attr & A_COLOR) >> 8) as i32;
        if pair_num > 0
            && let Some(&(fg, bg)) = self.pairs.get(&pair_num)
        {
            if (0..=7).contains(&fg) {
                out.push_str(&format!(";{}", 30 + fg));
            } else if fg >= 8 {
                out.push_str(&format!(";38;5;{fg}"));
            }
            if (0..=7).contains(&bg) {
                out.push_str(&format!(";{}", 40 + bg));
            } else if bg >= 8 {
                out.push_str(&format!(";48;5;{bg}"));
            }
        }
        out.push('m');
    }

    pub(crate) fn apply_termios_modes(&mut self) {
        #[cfg(unix)]
        {
            // SAFETY: `termios` is POD; `tcgetattr` fails cleanly with ENOTTY if stdin is not a tty.
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(0, &mut mode) } != 0 {
                return;
            }
            if self.echo {
                mode.c_lflag |= libc::ECHO;
            } else {
                mode.c_lflag &= !libc::ECHO;
            }
            if self.raw {
                mode.c_lflag &= !(libc::ICANON | libc::ISIG | libc::IEXTEN);
                mode.c_iflag &=
                    !(libc::IXON | libc::ICRNL | libc::BRKINT | libc::INPCK | libc::ISTRIP);
                mode.c_cc[libc::VMIN] = 1;
                mode.c_cc[libc::VTIME] = 0;
            } else if self.cbreak {
                mode.c_lflag &= !libc::ICANON;
                mode.c_lflag |= libc::ISIG;
                mode.c_cc[libc::VMIN] = 1;
                mode.c_cc[libc::VTIME] = 0;
            } else {
                mode.c_lflag |= libc::ICANON | libc::ISIG;
            }
            if self.nl {
                mode.c_iflag |= libc::ICRNL;
                mode.c_oflag |= libc::ONLCR;
            } else {
                mode.c_iflag &= !libc::ICRNL;
                mode.c_oflag &= !libc::ONLCR;
            }
            unsafe {
                let _ = libc::tcsetattr(0, libc::TCSANOW, &mode);
            }
            self.prog_termios = Some(mode);
        }
    }

    pub(crate) fn read_key(&mut self, win_id: usize) -> i32 {
        if let Some(ch) = self.ungetch_buf.pop_front() {
            return ch;
        }
        let (nodelay, timeout_ms, keypad) = match self.windows.get(&win_id) {
            Some(w) => (w.nodelay, w.timeout_ms, w.keypad),
            None => return -1,
        };
        let poll_timeout = if nodelay { 0 } else { timeout_ms };
        let Some(first) = read_byte_with_timeout(0, poll_timeout) else {
            return -1;
        };
        if !keypad {
            return first as i32;
        }
        if first == 0x7f {
            return KEY_BACKSPACE as i32;
        }
        if first == 0x1b {
            // Try to read an ANSI escape sequence with a short 25ms follow-up timeout.
            let Some(second) = read_byte_with_timeout(0, 25) else {
                return 0x1b;
            };
            if second == b'[' {
                let Some(third) = read_byte_with_timeout(0, 25) else {
                    return 0x1b;
                };
                match third {
                    b'A' => return KEY_UP as i32,
                    b'B' => return KEY_DOWN as i32,
                    b'C' => return KEY_RIGHT as i32,
                    b'D' => return KEY_LEFT as i32,
                    b'H' => return KEY_HOME as i32,
                    b'F' => return KEY_END as i32,
                    b'Z' => return KEY_BTAB as i32,
                    b'1'..=b'9' => {
                        let mut seq = vec![third];
                        while let Some(b) = read_byte_with_timeout(0, 25) {
                            seq.push(b);
                            if b == b'~' || b.is_ascii_alphabetic() || seq.len() >= 8 {
                                break;
                            }
                        }
                        return match seq.as_slice() {
                            b"1~" | b"7~" => KEY_HOME as i32,
                            b"2~" => KEY_IC as i32,
                            b"3~" => KEY_DC as i32,
                            b"4~" | b"8~" => KEY_END as i32,
                            b"5~" => KEY_PPAGE as i32,
                            b"6~" => KEY_NPAGE as i32,
                            b"15~" => (KEY_F0 + 5) as i32,
                            b"17~" => (KEY_F0 + 6) as i32,
                            b"18~" => (KEY_F0 + 7) as i32,
                            b"19~" => (KEY_F0 + 8) as i32,
                            b"20~" => (KEY_F0 + 9) as i32,
                            b"21~" => (KEY_F0 + 10) as i32,
                            b"23~" => (KEY_F0 + 11) as i32,
                            b"24~" => (KEY_F0 + 12) as i32,
                            _ => 0x1b,
                        };
                    }
                    _ => return 0x1b,
                }
            } else if second == b'O' {
                let Some(third) = read_byte_with_timeout(0, 25) else {
                    return 0x1b;
                };
                return match third {
                    b'P' => (KEY_F0 + 1) as i32,
                    b'Q' => (KEY_F0 + 2) as i32,
                    b'R' => (KEY_F0 + 3) as i32,
                    b'S' => (KEY_F0 + 4) as i32,
                    b'H' => KEY_HOME as i32,
                    b'F' => KEY_END as i32,
                    _ => 0x1b,
                };
            } else {
                self.ungetch_buf.push_front(second as i32);
                return 0x1b;
            }
        }
        first as i32
    }

    pub(crate) fn update_panels(&mut self) {
        if let Some(stdscr_id) = self.stdscr_id {
            let _ = self.win_noutrefresh(stdscr_id, None);
        }
        let visible_wins: Vec<usize> = self
            .panels
            .iter()
            .filter(|p| !p.hidden)
            .map(|p| p.win_id)
            .collect();
        for wid in visible_wins {
            let _ = self.win_noutrefresh(wid, None);
        }
    }
}

fn read_byte_with_timeout(fd: i32, timeout_ms: i32) -> Option<u8> {
    #[cfg(unix)]
    {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pfd` is a valid stack `libc::pollfd`.
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if rc <= 0 || (pfd.revents & libc::POLLIN) == 0 {
            return None;
        }
        let mut b: u8 = 0;
        let n = unsafe { libc::read(fd, (&mut b as *mut u8).cast(), 1) };
        if n == 1 { Some(b) } else { None }
    }
    #[cfg(not(unix))]
    {
        let _ = (fd, timeout_ms);
        None
    }
}

fn acs_to_unicode(ch: char) -> char {
    match ch {
        'l' => '┌',
        'm' => '└',
        'k' => '┐',
        'j' => '┘',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'q' => '─',
        'x' => '│',
        'n' => '┼',
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        '~' => '·',
        '0' => '█',
        other => other,
    }
}

// ── Terminfo capability lookups ──────────────────────────────────────────────

pub(crate) fn tigetflag(term: &str, capname: &str) -> i64 {
    let profile = classify_term(term).unwrap_or(TermProfile::Xterm256);
    match capname {
        "am" => 1,
        "bce" | "km" | "mir" | "msgr" | "xenl" | "npc" => i64::from(profile != TermProfile::Dumb),
        "ccc" => i64::from(profile == TermProfile::Xterm256),
        "bw" | "eo" | "gn" | "hc" | "hs" | "hz" | "in" | "os" | "ul" | "xon" => 0,
        _ => -1,
    }
}

pub(crate) fn tigetnum(term: &str, capname: &str, lines: usize, cols: usize) -> i64 {
    let profile = classify_term(term).unwrap_or(TermProfile::Xterm256);
    match capname {
        "colors" => match profile {
            TermProfile::Xterm256 => 256,
            TermProfile::Xterm8 => 8,
            TermProfile::Vt100 | TermProfile::Dumb => -1,
        },
        "pairs" => match profile {
            TermProfile::Xterm256 => 65536,
            TermProfile::Xterm8 => 64,
            TermProfile::Vt100 | TermProfile::Dumb => -1,
        },
        "cols" => cols as i64,
        "lines" => lines as i64,
        "it" => 8,
        "ncv" => -1,
        _ => -2,
    }
}

pub(crate) fn tigetstr(term: &str, capname: &str) -> Option<&'static [u8]> {
    let profile = classify_term(term).unwrap_or(TermProfile::Xterm256);
    if profile == TermProfile::Dumb {
        return match capname {
            "bel" => Some(b"\x07"),
            "cr" => Some(b"\r"),
            "cud1" | "ind" => Some(b"\n"),
            _ => None,
        };
    }
    match capname {
        // Screen & cursor capabilities
        "bel" => Some(b"\x07"),
        "cr" => Some(b"\r"),
        "cud1" | "ind" => Some(b"\n"),
        "clear" => Some(b"\x1b[H\x1b[2J"),
        "home" => Some(b"\x1b[H"),
        "cup" => Some(b"\x1b[%i%p1%d;%p2%dH"),
        "el" => Some(b"\x1b[K"),
        "ed" => Some(b"\x1b[J"),
        "bold" => Some(b"\x1b[1m"),
        "dim" => Some(b"\x1b[2m"),
        "sitm" => Some(b"\x1b[3m"),
        "ritm" => Some(b"\x1b[23m"),
        "smul" => Some(b"\x1b[4m"),
        "rmul" => Some(b"\x1b[24m"),
        "blink" => Some(b"\x1b[5m"),
        "rev" | "smso" => Some(b"\x1b[7m"),
        "rmso" => Some(b"\x1b[27m"),
        "invis" => Some(b"\x1b[8m"),
        "sgr0" => Some(b"\x1b(B\x1b[m"),
        "civis" => Some(b"\x1b[?25l"),
        "cnorm" | "cvvis" => Some(b"\x1b[?12l\x1b[?25h"),
        "smcup" => Some(b"\x1b[?1049h"),
        "rmcup" => Some(b"\x1b[?1049l"),
        "flash" => Some(b"\x1b[?5h$<100/>\x1b[?5l"),
        "csr" => Some(b"\x1b[%i%p1%d;%p2%dr"),
        "il1" => Some(b"\x1b[L"),
        "dl1" => Some(b"\x1b[M"),
        "ich1" => Some(b"\x1b[@"),
        "dch1" => Some(b"\x1b[P"),
        "setaf" => match profile {
            TermProfile::Xterm256 => Some(b"\x1b[38;5;%p1%dm"),
            TermProfile::Xterm8 => Some(b"\x1b[3%p1%dm"),
            _ => None,
        },
        "setab" => match profile {
            TermProfile::Xterm256 => Some(b"\x1b[48;5;%p1%dm"),
            TermProfile::Xterm8 => Some(b"\x1b[4%p1%dm"),
            _ => None,
        },
        "op" => Some(b"\x1b[39;49m"),
        // Key capabilities (used by `curses.has_key`)
        "kcuu1" => Some(b"\x1bOA"),
        "kcud1" => Some(b"\x1bOB"),
        "kcuf1" => Some(b"\x1bOC"),
        "kcub1" => Some(b"\x1bOD"),
        "khome" => Some(b"\x1bOH"),
        "kend" => Some(b"\x1bOF"),
        "kbs" => Some(b"\x7f"),
        "kdch1" => Some(b"\x1b[3~"),
        "kich1" => Some(b"\x1b[2~"),
        "knp" => Some(b"\x1b[6~"),
        "kpp" => Some(b"\x1b[5~"),
        "kcbt" => Some(b"\x1b[Z"),
        "kent" => Some(b"\x1bOM"),
        "kf1" => Some(b"\x1bOP"),
        "kf2" => Some(b"\x1bOQ"),
        "kf3" => Some(b"\x1bOR"),
        "kf4" => Some(b"\x1bOS"),
        "kf5" => Some(b"\x1b[15~"),
        "kf6" => Some(b"\x1b[17~"),
        "kf7" => Some(b"\x1b[18~"),
        "kf8" => Some(b"\x1b[19~"),
        "kf9" => Some(b"\x1b[20~"),
        "kf10" => Some(b"\x1b[21~"),
        "kf11" => Some(b"\x1b[23~"),
        "kf12" => Some(b"\x1b[24~"),
        "kb2" => Some(b"\x1bOE"),
        "kLFT" => Some(b"\x1b[1;2D"),
        "kRIT" => Some(b"\x1b[1;2C"),
        "kHOM" => Some(b"\x1b[1;2H"),
        "kEND" => Some(b"\x1b[1;2F"),
        "kDC" => Some(b"\x1b[3;2~"),
        "kIC" => Some(b"\x1b[2;2~"),
        "kNXT" => Some(b"\x1b[6;2~"),
        "kPRV" => Some(b"\x1b[5;2~"),
        "kind" => Some(b"\x1b[1;2B"),
        "kri" => Some(b"\x1b[1;2A"),
        _ => None,
    }
}

/// Pure-Rust terminfo `tparm` parameter evaluator supporting standard `%` codes.
pub(crate) fn eval_tparm(fmt: &[u8], raw_params: &[i64]) -> Vec<u8> {
    let mut params = [0i64; 9];
    for (dst, &src) in params.iter_mut().zip(raw_params.iter()) {
        *dst = src;
    }
    let mut out = Vec::with_capacity(fmt.len() + 16);
    let mut stack: Vec<i64> = Vec::new();
    let mut i = 0;
    let mut dyn_vars = [0i64; 26];
    let mut static_vars = [0i64; 26];

    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt.len() {
            break;
        }
        match fmt[i] {
            b'%' => {
                out.push(b'%');
                i += 1;
            }
            b'i' => {
                params[0] += 1;
                params[1] += 1;
                i += 1;
            }
            b'p' if i + 1 < fmt.len() && fmt[i + 1].is_ascii_digit() => {
                let idx = (fmt[i + 1] - b'0') as usize;
                if (1..=9).contains(&idx) {
                    stack.push(params[idx - 1]);
                }
                i += 2;
            }
            b'd' => {
                let val = stack.pop().unwrap_or(0);
                out.extend_from_slice(val.to_string().as_bytes());
                i += 1;
            }
            b'c' => {
                let val = stack.pop().unwrap_or(0) as u8;
                out.push(if val == 0 { 0x80 } else { val });
                i += 1;
            }
            b'{' => {
                i += 1;
                let start = i;
                while i < fmt.len() && fmt[i] != b'}' {
                    i += 1;
                }
                let s = std::str::from_utf8(&fmt[start..i]).unwrap_or("0");
                let n = s.parse::<i64>().unwrap_or(0);
                stack.push(n);
                if i < fmt.len() {
                    i += 1;
                }
            }
            b'\'' if i + 2 < fmt.len() && fmt[i + 2] == b'\'' => {
                stack.push(fmt[i + 1] as i64);
                i += 3;
            }
            b'P' if i + 1 < fmt.len() => {
                let v = stack.pop().unwrap_or(0);
                let c = fmt[i + 1];
                if c.is_ascii_lowercase() {
                    dyn_vars[(c - b'a') as usize] = v;
                } else if c.is_ascii_uppercase() {
                    static_vars[(c - b'A') as usize] = v;
                }
                i += 2;
            }
            b'g' if i + 1 < fmt.len() => {
                let c = fmt[i + 1];
                if c.is_ascii_lowercase() {
                    stack.push(dyn_vars[(c - b'a') as usize]);
                } else if c.is_ascii_uppercase() {
                    stack.push(static_vars[(c - b'A') as usize]);
                }
                i += 2;
            }
            b'+' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(a.wrapping_add(b));
                i += 1;
            }
            b'-' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(a.wrapping_sub(b));
                i += 1;
            }
            b'*' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(a.wrapping_mul(b));
                i += 1;
            }
            b'/' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(if b == 0 { 0 } else { a / b });
                i += 1;
            }
            b'm' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(if b == 0 { 0 } else { a % b });
                i += 1;
            }
            b'&' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(a & b);
                i += 1;
            }
            b'|' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(a | b);
                i += 1;
            }
            b'^' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(a ^ b);
                i += 1;
            }
            b'=' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(i64::from(a == b));
                i += 1;
            }
            b'>' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(i64::from(a > b));
                i += 1;
            }
            b'<' => {
                let b = stack.pop().unwrap_or(0);
                let a = stack.pop().unwrap_or(0);
                stack.push(i64::from(a < b));
                i += 1;
            }
            b'!' => {
                let a = stack.pop().unwrap_or(0);
                stack.push(i64::from(a == 0));
                i += 1;
            }
            b'~' => {
                let a = stack.pop().unwrap_or(0);
                stack.push(!a);
                i += 1;
            }
            b'?' | b';' => {
                i += 1;
            }
            b't' => {
                i += 1;
                let cond = stack.pop().unwrap_or(0);
                if cond == 0 {
                    // Skip until matching %e or %;
                    let mut depth = 0usize;
                    while i + 1 < fmt.len() {
                        if fmt[i] == b'%' {
                            match fmt[i + 1] {
                                b'?' => depth += 1,
                                b';' => {
                                    if depth == 0 {
                                        i += 2;
                                        break;
                                    }
                                    depth -= 1;
                                }
                                b'e' if depth == 0 => {
                                    i += 2;
                                    break;
                                }
                                _ => {}
                            }
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                }
            }
            b'e' => {
                // Reached %e after executing %t branch: skip until matching %;
                i += 1;
                let mut depth = 0usize;
                while i + 1 < fmt.len() {
                    if fmt[i] == b'%' {
                        match fmt[i + 1] {
                            b'?' => depth += 1,
                            b';' => {
                                if depth == 0 {
                                    i += 2;
                                    break;
                                }
                                depth -= 1;
                            }
                            _ => {}
                        }
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

// ── Keyname & unctrl helpers ─────────────────────────────────────────────────

pub(crate) fn unctrl_bytes(ch: u32) -> Vec<u8> {
    let b = (ch & 0xff) as u8;
    if b == 0x7f {
        b"^?".to_vec()
    } else if (0x20..=0x7e).contains(&b) {
        vec![b]
    } else if b < 0x20 {
        vec![b'^', b + 0x40]
    } else if b == 0xff {
        b"M-^?".to_vec()
    } else if b >= 0xa0 {
        vec![b'M', b'-', b & 0x7f]
    } else {
        vec![b'M', b'-', b'^', (b & 0x7f) + 0x40]
    }
}

pub(crate) fn keyname_bytes(k: i64) -> Vec<u8> {
    if k < 0 {
        return Vec::new();
    }
    if k < 256 {
        return unctrl_bytes(k as u32);
    }
    if (KEY_F0..=KEY_F0 + 63).contains(&k) {
        return format!("KEY_F({})", k - KEY_F0).into_bytes();
    }
    let name: Option<&str> = match k {
        KEY_BREAK => Some("KEY_BREAK"),
        KEY_DOWN => Some("KEY_DOWN"),
        KEY_UP => Some("KEY_UP"),
        KEY_LEFT => Some("KEY_LEFT"),
        KEY_RIGHT => Some("KEY_RIGHT"),
        KEY_HOME => Some("KEY_HOME"),
        KEY_BACKSPACE => Some("KEY_BACKSPACE"),
        KEY_DL => Some("KEY_DL"),
        KEY_IL => Some("KEY_IL"),
        KEY_DC => Some("KEY_DC"),
        KEY_IC => Some("KEY_IC"),
        KEY_EIC => Some("KEY_EIC"),
        KEY_CLEAR => Some("KEY_CLEAR"),
        KEY_EOS => Some("KEY_EOS"),
        KEY_EOL => Some("KEY_EOL"),
        KEY_SF => Some("KEY_SF"),
        KEY_SR => Some("KEY_SR"),
        KEY_NPAGE => Some("KEY_NPAGE"),
        KEY_PPAGE => Some("KEY_PPAGE"),
        KEY_STAB => Some("KEY_STAB"),
        KEY_CTAB => Some("KEY_CTAB"),
        KEY_CATAB => Some("KEY_CATAB"),
        KEY_ENTER => Some("KEY_ENTER"),
        KEY_SRESET => Some("KEY_SRESET"),
        KEY_RESET => Some("KEY_RESET"),
        KEY_PRINT => Some("KEY_PRINT"),
        KEY_LL => Some("KEY_LL"),
        KEY_A1 => Some("KEY_A1"),
        KEY_A3 => Some("KEY_A3"),
        KEY_B2 => Some("KEY_B2"),
        KEY_C1 => Some("KEY_C1"),
        KEY_C3 => Some("KEY_C3"),
        KEY_BTAB => Some("KEY_BTAB"),
        KEY_BEG => Some("KEY_BEG"),
        KEY_CANCEL => Some("KEY_CANCEL"),
        KEY_CLOSE => Some("KEY_CLOSE"),
        KEY_COMMAND => Some("KEY_COMMAND"),
        KEY_COPY => Some("KEY_COPY"),
        KEY_CREATE => Some("KEY_CREATE"),
        KEY_END => Some("KEY_END"),
        KEY_EXIT => Some("KEY_EXIT"),
        KEY_FIND => Some("KEY_FIND"),
        KEY_HELP => Some("KEY_HELP"),
        KEY_MARK => Some("KEY_MARK"),
        KEY_MESSAGE => Some("KEY_MESSAGE"),
        KEY_MOVE => Some("KEY_MOVE"),
        KEY_NEXT => Some("KEY_NEXT"),
        KEY_OPEN => Some("KEY_OPEN"),
        KEY_OPTIONS => Some("KEY_OPTIONS"),
        KEY_PREVIOUS => Some("KEY_PREVIOUS"),
        KEY_REDO => Some("KEY_REDO"),
        KEY_REFERENCE => Some("KEY_REFERENCE"),
        KEY_REFRESH => Some("KEY_REFRESH"),
        KEY_REPLACE => Some("KEY_REPLACE"),
        KEY_RESTART => Some("KEY_RESTART"),
        KEY_RESUME => Some("KEY_RESUME"),
        KEY_SAVE => Some("KEY_SAVE"),
        KEY_SELECT => Some("KEY_SELECT"),
        KEY_SUSPEND => Some("KEY_SUSPEND"),
        KEY_UNDO => Some("KEY_UNDO"),
        KEY_MOUSE => Some("KEY_MOUSE"),
        KEY_RESIZE => Some("KEY_RESIZE"),
        _ => None,
    };
    name.map(|s| s.as_bytes().to_vec()).unwrap_or_default()
}

pub(crate) fn color_pair(n: i64) -> u32 {
    ((n as u32) & 0xff) << 8
}

pub(crate) fn pair_number(attr: u32) -> i64 {
    ((attr & A_COLOR) >> 8) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tparm_cup_and_color() {
        assert_eq!(eval_tparm(b"\x1b[%i%p1%d;%p2%dH", &[0, 0]), b"\x1b[1;1H");
        assert_eq!(eval_tparm(b"\x1b[%i%p1%d;%p2%dH", &[4, 9]), b"\x1b[5;10H");
        assert_eq!(eval_tparm(b"\x1b[38;5;%p1%dm", &[42]), b"\x1b[38;5;42m");
    }

    #[test]
    fn window_and_subwin_share_cells() {
        let mut st = CursesState::default();
        st.setupterm(Some("xterm-256color"), 1).unwrap();
        let stdscr = st.initscr().unwrap();
        let win = st.alloc_window(10, 20, 2, 3, false);
        let sub = st.alloc_subwin(win, 4, 8, 1, 2, true).unwrap();

        st.windows.get_mut(&sub).unwrap().cur_y = 0;
        st.windows.get_mut(&sub).unwrap().cur_x = 0;
        for ch in "Hello".chars() {
            st.win_put_char(sub, ch, A_BOLD).unwrap();
        }
        // Parent sees subwindow writes at (1, 2..7)
        let mut read_back = String::new();
        for x in 2..7 {
            let cell = st.get_cell(win, 1, x).unwrap();
            assert_eq!(cell.attr & A_BOLD, A_BOLD);
            read_back.push(cell.ch);
        }
        assert_eq!(read_back, "Hello");
        let _ = stdscr;
    }

    #[test]
    fn keyname_and_unctrl_basics() {
        assert_eq!(keyname_bytes(b'a' as i64), b"a");
        assert_eq!(keyname_bytes(1), b"^A");
        assert_eq!(keyname_bytes(KEY_UP), b"KEY_UP");
        assert_eq!(keyname_bytes(KEY_F0 + 5), b"KEY_F(5)");
        assert_eq!(unctrl_bytes(1), b"^A");
        assert_eq!(unctrl_bytes(0x7f), b"^?");
        assert_eq!(unctrl_bytes(b'Z' as u32), b"Z");
    }

    #[test]
    fn color_pair_roundtrip() {
        let cp = color_pair(5);
        assert_eq!(cp, 5 << 8);
        assert_eq!(pair_number(cp | A_BOLD), 5);
    }
}

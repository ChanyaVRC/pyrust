use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::environment::EnvRef;

// ─────────────────────────────────────────────────────────────────────────────
// Traceback frame tracking
// ─────────────────────────────────────────────────────────────────────────────

/// Strong ownership handle for the root namespace exposed as a traceback
/// frame's `f_globals`.
///
/// Tracebacks can outlive the interpreter object that executed an imported
/// module (failed imports are the important case), so a namespace id or a weak
/// reference is insufficient: the frame must keep the root environment and its
/// globals provider alive until the traceback is materialised.  Keeping only
/// the root, rather than the active lexical child, is also the minimum handle
/// required for that ownership contract.
#[derive(Debug, Clone)]
pub struct FrameGlobals {
    root: EnvRef,
}

impl FrameGlobals {
    /// Resolve `env` to its root/module environment and retain it strongly.
    pub fn for_environment(env: &EnvRef) -> Self {
        let mut root = Rc::clone(env);
        loop {
            let parent = root.borrow().parent.clone();
            match parent {
                Some(parent) => root = parent,
                None => return Self { root },
            }
        }
    }

    /// Root environment whose live globals mapping belongs to this frame.
    pub fn environment(&self) -> &EnvRef {
        &self.root
    }
}

/// A single frame in a Python-style traceback.
///
/// Populated by the interpreter when errors propagate out of
/// `run_bytecode`.
#[derive(Debug, Clone)]
pub struct FrameInfo {
    /// Path to the source file that contains this frame.  Stored as `Arc<str>`
    /// so that cloning a frame (e.g. when snapshotting `CAPTURED_ERROR_FRAMES`)
    /// is a cheap reference-count bump rather than a heap allocation.
    pub filename: Arc<str>,
    /// 1-based source line that raised the error, or `None` when no line
    /// table is available.
    pub lineno: Option<u32>,
    /// Verbatim text of the source line pointed to by `lineno`, or `None`.
    /// Leading whitespace is preserved; trailing whitespace is stripped.
    /// Stored as `Arc<str>` so that cloning is a cheap reference-count bump.
    pub source_line: Option<Arc<str>>,
    /// Function or method name.  `"<module>"` for module-scope code.
    /// `Arc<str>` so cloning a frame is a reference-count bump, not a heap alloc.
    pub funcname: Arc<str>,
    /// Root namespace that owns this frame's Python-visible `f_globals`.
    ///
    /// Runtime-captured frames always carry `Some`.  `None` is reserved for
    /// display-only frames reconstructed from an already-materialised traceback;
    /// those frames are fed only to [`format_traceback`] and never become Python
    /// frame objects again.
    pub globals: Option<FrameGlobals>,
    /// PEP 657 fine-grained caret anchor:
    /// `(full_start, prim_start, prim_end, full_end)` (0-based **char** offsets
    /// into the *un-dedented* source line) of the sub-expression that raised,
    /// when the compiler tracked a column span for the raising instruction.
    /// `[full_start, full_end)` is underlined; the "primary" sub-range
    /// `[prim_start, prim_end)` gets `^`, the rest `~` (binary operands /
    /// subscript object).  When `full == prim` the whole span is `^` (bare name,
    /// call).  `None` when no column information is available — the formatter
    /// then omits the caret row (issues #2426 / #2411 plumb only the
    /// highest-value forms; everything else stays `None` and caret-free, never
    /// wrong).
    ///
    /// The offsets are measured against the source line *before* its own
    /// leading whitespace is stripped for display; the formatter re-bases them
    /// against the dedented line when rendering.
    pub col_span: Option<(u32, u32, u32, u32)>,
}

thread_local! {
    /// The traceback frame chain for the error currently unwinding through the
    /// call stack, built **lazily** as the error propagates outward.
    ///
    /// The no-exception call path no longer touches any traceback thread-local
    /// (the previous design pushed/popped a per-call frame eagerly, which cost
    /// a heap `Arc::from` allocation + two thread-local borrows on *every*
    /// call — see `perf: reduce per-call overhead`).  Instead, each call site
    /// records its own `FrameInfo` here **only when its body returned `Err`**,
    /// inserting at the front so the chain ends up outermost-first /
    /// innermost-last (matching the old full-stack snapshot order).
    ///
    /// Reset to `None` at the top of each `try_exec_vm_script_with_index` run
    /// and cleared on normal exception-handler exit (`vm.rs` `PopExcContext`).
    static CAPTURED_ERROR_FRAMES: RefCell<Option<Vec<FrameInfo>>> = const { RefCell::new(None) };

    /// The 1-based source line number of the most recently executed instruction
    /// in the innermost VM dispatch loop.  Updated on every instruction whose
    /// `lineno_table` entry is non-zero.  Read by the traceback builder after
    /// `run_bytecode` returns an error, to fill in the `<module>` frame's `lineno`.
    ///
    /// Reset to 0 at the start of each top-level script execution.
    static CURRENT_VM_LINE: RefCell<u32> = const { RefCell::new(0) };

    /// The PEP 657 caret anchor `(col_offset, end_col_offset)` of the
    /// instruction at which the unwinding error was raised in the innermost
    /// VM dispatch loop (issue #2426).  Unlike `CURRENT_VM_LINE`, this is NOT
    /// updated per-instruction — the VM publishes it **only on the error path**
    /// (when an exception escapes the frame), from the raising instruction's
    /// `col_table` entry, so the per-instruction hot path stays untouched.
    ///
    /// `None` when the raising instruction carried no column span (the common
    /// case for statement forms not yet plumbed in stage 1).  Reset to `None`
    /// at the start of each top-level script execution.
    static CURRENT_VM_COL_SPAN: RefCell<Option<(u32, u32, u32, u32)>> = const { RefCell::new(None) };
}

/// Record a traceback frame for an error unwinding out of a user-function body.
///
/// Called by the call and execution domains **only when the function body
/// returned `Err`**
/// (the no-error common path skips this entirely).  Frames are inserted at the
/// front so that, as the error propagates from innermost to outermost call, the
/// resulting chain is ordered outermost-first / innermost-last — identical to
/// the order the previous eager full-stack snapshot produced.
#[inline]
pub fn record_traceback_frame(frame: FrameInfo) {
    CAPTURED_ERROR_FRAMES.with(|captured| {
        captured
            .borrow_mut()
            .get_or_insert_with(Vec::new)
            .insert(0, frame);
    });
}

/// Take the captured error frame snapshot, leaving the thread-local as `None`.
///
/// Called once by `try_exec_vm_script_with_index` after `run_bytecode`
/// returns an error, to build the traceback header.  Also called at the start
/// of each script run to reset any stale snapshot from a previous run.
#[inline]
pub fn take_captured_error_frames() -> Option<Vec<FrameInfo>> {
    CAPTURED_ERROR_FRAMES.with(|c| c.borrow_mut().take())
}

/// Clone the captured error frame snapshot without consuming it.
///
/// Used by the VM to build the Python-visible traceback object chain
/// (`exc.__traceback__`) when an exception is caught — the snapshot must
/// remain in place so that, if the same error later escapes the handler, the
/// stderr traceback formatter still sees the full frame list.  Returns an
/// empty `Vec` when no frames have been captured.
#[inline]
pub fn clone_captured_error_frames() -> Vec<FrameInfo> {
    CAPTURED_ERROR_FRAMES.with(|c| c.borrow().clone().unwrap_or_default())
}

/// Length of the captured error frame snapshot without cloning it.
///
/// The catch-site `__traceback__` reuse check (issue #2359) only needs the
/// frame count to compare against an existing materialised chain; cloning the
/// whole `Vec<FrameInfo>` for that would put an allocation on every caught
/// exception.  Returns 0 when no frames have been captured.
#[inline]
pub fn captured_error_frames_len() -> usize {
    CAPTURED_ERROR_FRAMES.with(|c| c.borrow().as_ref().map_or(0, |v| v.len()))
}

/// Clear the captured error frame snapshot (reset between script runs).
#[inline]
pub fn reset_captured_error_frames() {
    CAPTURED_ERROR_FRAMES.with(|c| *c.borrow_mut() = None);
}

/// Update the current VM line counter.  Called by the VM dispatch loop when
/// the `lineno_table` entry for the current instruction is non-zero.
#[inline]
pub fn set_current_vm_line(lineno: u32) {
    CURRENT_VM_LINE.with(|c| *c.borrow_mut() = lineno);
}

/// Read the current VM line counter.  Called by `try_exec_vm_script_with_index`
/// after `run_bytecode` returns an error, to fill in the `<module>` frame.
#[inline]
pub fn get_current_vm_line() -> u32 {
    CURRENT_VM_LINE.with(|c| *c.borrow())
}

/// Reset the current VM line counter.  Called at the start of each top-level
/// script execution (via `reset_captured_error_frames`).
#[inline]
pub fn reset_current_vm_line() {
    CURRENT_VM_LINE.with(|c| *c.borrow_mut() = 0);
    CURRENT_VM_COL_SPAN.with(|c| *c.borrow_mut() = None);
}

/// Publish the PEP 657 caret anchor of the raising instruction (issue #2426).
///
/// Called by the VM **only on the error-escape path**, with the `col_table`
/// entry of the instruction that raised.  `None` clears any stale span so a
/// later raise on an unplumbed instruction does not inherit a previous anchor.
#[inline]
pub fn set_current_vm_col_span(span: Option<(u32, u32, u32, u32)>) {
    CURRENT_VM_COL_SPAN.with(|c| *c.borrow_mut() = span);
}

/// Read the current VM caret anchor.  Called by `try_exec_vm_script_with_index`
/// after `run_bytecode` returns an error, to fill in the `<module>` frame's
/// `col_span` (issue #2426).
#[inline]
pub fn get_current_vm_col_span() -> Option<(u32, u32, u32, u32)> {
    CURRENT_VM_COL_SPAN.with(|c| *c.borrow())
}

/// Build the PEP 657 caret underline row for one frame (issue #2426), or
/// `None` when no caret row should be printed.
///
/// `stripped` is the dedented display line (its own leading whitespace already
/// removed).  `leading` is the count of leading-whitespace *chars* that were
/// stripped — i.e. the offset to rebase the anchor's column from the original
/// line onto the dedented line.  `col_span` is the
/// `(full_start, prim_start, prim_end, full_end)` 0-based char anchor measured
/// against the *original* line (see [`FrameInfo::col_span`]).
///
/// The whole `[full_start, full_end)` range is underlined: the primary
/// sub-range `[prim_start, prim_end)` with `^`, the rest with `~`.  When
/// `full == prim` the whole span is `^` (bare name / call).
///
/// Returns `None` (omit the row) when:
///  * there is no `col_span` (form not plumbed), or
///  * the anchor (a `full == prim` whole-`^` span) covers the whole stripped
///    line — CPython omits the caret row for a bare name / call / `raise X(...)`
///    that spans the line, or
///  * the anchor is degenerate / out of range after rebasing.
fn caret_row(
    stripped: &str,
    leading: usize,
    col_span: Option<(u32, u32, u32, u32)>,
) -> Option<String> {
    let (full_start, prim_start, prim_end, full_end) = col_span?;
    // Multi-line binary-op anchor (issue #2571): the expression straddles
    // physical lines, so its operator / right-operand columns belong to a later
    // line than the displayed one.  The parser marks this with the
    // `ast::MULTILINE_FULL_END` (== `u32::MAX`) sentinel; clamp the underline to
    // the end of the displayed (dedented) line and draw solid `^` from
    // `full_start`, matching CPython 3.12.
    if full_end == u32::MAX {
        let full_start = full_start as usize;
        if full_start < leading {
            return None;
        }
        let f_start = full_start - leading;
        let line_len = stripped.chars().count();
        if f_start >= line_len {
            return None;
        }
        let mut row = String::from("    ");
        for _ in 0..f_start {
            row.push(' ');
        }
        for _ in f_start..line_len {
            row.push('^');
        }
        row.push('\n');
        return Some(row);
    }
    let (full_start, prim_start, prim_end, full_end) = (
        full_start as usize,
        prim_start as usize,
        prim_end as usize,
        full_end as usize,
    );
    // Sanity-check the nesting + non-degeneracy of the anchor.  Anchors inside
    // the stripped leading whitespace (full_start < leading) are not expected
    // for the plumbed forms; guard against underflow defensively and omit.
    if !(full_start <= prim_start && prim_start < prim_end && prim_end <= full_end)
        || full_start < leading
    {
        return None;
    }
    // Rebase onto the dedented line.
    let f_start = full_start - leading;
    let f_end = full_end - leading;
    let p_start = prim_start - leading;
    let p_end = prim_end - leading;
    let line_len = stripped.chars().count();
    if f_end > line_len {
        return None;
    }
    // Whole-line `^` anchor (full == prim, no `~` context): CPython omits it.
    let whole_line_caret = f_start == p_start && f_end == p_end;
    if whole_line_caret && f_start == 0 && f_end == line_len {
        return None;
    }
    let mut row = String::from("    ");
    for _ in 0..f_start {
        row.push(' ');
    }
    for col in f_start..f_end {
        if col >= p_start && col < p_end {
            row.push('^');
        } else {
            row.push('~');
        }
    }
    row.push('\n');
    Some(row)
}

/// Format a traceback chain as CPython does, returning it as a `String`.
///
/// `frames` is the list produced by `snapshot_traceback_frames()` (innermost
/// last) with the `<module>` frame prepended by the caller.
///
/// Output format:
/// ```text
/// Traceback (most recent call last):
///   File "test.py", line 3, in <module>
///     source_line
///     ^^^^^^^^^^^
/// SomeError: message
/// ```
///
/// When `lineno` is `None` the `line N` part is omitted.  When `source_line`
/// is `None` the source echo is omitted.
///
/// ## Source-line indentation (issue #2418)
///
/// CPython strips the displayed source line's own leading whitespace and emits
/// it under a fixed 4-space traceback indent, so an indented statement is not
/// over-indented.  We `trim_start()` the line before applying our indent.
///
/// ## PEP 657 caret (`^^^`) underlines (issue #2426)
///
/// CPython 3.12's underlines are *fine-grained*: they point at the precise
/// sub-expression (anchor) that raised, using its column span — e.g.
/// `x = undefined` underlines only `undefined`.  When the anchor covers the
/// **entire stripped source line**, CPython **omits the caret row entirely**
/// (a bare `name`, `f()`, `raise X(...)`, etc. print no carets).
///
/// We render the caret row only when the frame carries a `col_span` (the
/// compiler tracked a column anchor for the raising instruction — stage 1
/// plumbs the highest-value forms).  Frames without a span omit the row — which
/// is byte-exact with CPython for the whole-line-anchor class and strictly
/// safer than a wrong/over-wide caret for the rest (#2426 "a wrong caret is
/// worse than no caret").
///
/// Rules applied (against the *dedented* display line):
///  1. **Omission:** if the anchor spans the whole stripped line, no caret row.
///  2. **Carets:** `^` under `[col_offset, end_col_offset)` of the dedented line.
///
/// `~` context marks (rule 3: binary ops / subscripts) and multi-line clamping
/// (rule 4) are deferred to later stages; until then those forms carry no span
/// and stay caret-free.
pub fn format_traceback(frames: &[FrameInfo], error_line: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("Traceback (most recent call last):\n");
    for frame in frames {
        match frame.lineno {
            Some(n) => {
                let _ = writeln!(
                    out,
                    "  File \"{}\", line {}, in {}",
                    frame.filename, n, frame.funcname
                );
            }
            None => {
                let _ = writeln!(out, "  File \"{}\", in {}", frame.filename, frame.funcname);
            }
        }
        // Emit the source line, dedented to a fixed 4-space indent (#2418),
        // then a PEP 657 caret row when this frame carries a column anchor
        // (#2426).  Carets are placed against the dedented line.
        if let Some(src) = &frame.source_line {
            let stripped = src.trim_start();
            if stripped.is_empty() {
                continue;
            }
            let leading = src.chars().count() - stripped.chars().count();
            let _ = writeln!(out, "    {stripped}");
            if let Some(caret) = caret_row(stripped, leading, frame.col_span) {
                out.push_str(&caret);
            }
        }
    }
    out.push_str(error_line);
    out
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::FrameGlobals;
    use crate::environment::Environment;

    #[test]
    fn frame_globals_owns_the_root_after_lexical_envs_are_dropped() {
        let root = Environment::new(None);
        let child = Environment::new(Some(Rc::clone(&root)));
        let weak_root = Rc::downgrade(&root);

        let globals = FrameGlobals::for_environment(&child);
        assert!(Rc::ptr_eq(globals.environment(), &root));

        drop(child);
        drop(root);
        assert!(
            weak_root.upgrade().is_some(),
            "traceback globals must outlive the executing environment"
        );

        drop(globals);
        assert!(weak_root.upgrade().is_none());
    }
}

// `tracemalloc` module — 100% native Rust implementation of Python 3.12's
// `tracemalloc` stdlib module (issue #3246).
//
// Provides `start`, `stop`, `is_tracing`, `clear_traces`, `get_traced_memory`,
// `reset_peak`, `get_traceback_limit`, `get_tracemalloc_memory`,
// `get_object_traceback`, `take_snapshot`, `Frame`, `Traceback`, `Trace`,
// `Statistic`, `StatisticDiff`, `DomainFilter`, `Filter`, `Snapshot`, and
// `_format_size`.
//
// Reference: <https://docs.python.org/3.12/library/tracemalloc.html>

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_type_name_str};
use crate::value::{Value, ValueKind};

// ── Shared helpers ───────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn tm_class(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let mod_val = interp.load_module("tracemalloc")?;
    interp.get_attr(&mod_val, name)
}

pub(crate) fn format_size(size: f64, sign: bool) -> String {
    let mut cur = size;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB"] {
        if cur.abs() < 100.0 && unit != "B" {
            return if sign {
                format!("{cur:+.1} {unit}")
            } else {
                format!("{cur:.1} {unit}")
            };
        }
        if cur.abs() < 10.0 * 1024.0 || unit == "TiB" {
            let rounded = cur.round();
            let rounded = if rounded == 0.0 { 0.0 } else { rounded };
            return if sign {
                format!("{rounded:+.0} {unit}")
            } else {
                format!("{rounded:.0} {unit}")
            };
        }
        cur /= 1024.0;
    }
    unreachable!()
}

fn hash_i64<T: Hash>(v: &T) -> i64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut hasher);
    let h = hasher.finish() as i64;
    if h == -1 { -2 } else { h }
}

fn normalize_path(filename: &str) -> String {
    let mut s = if cfg!(windows) {
        filename.replace('/', "\\").to_lowercase()
    } else {
        filename.to_string()
    };
    if s.ends_with(".pyc") {
        s.truncate(s.len() - 1);
    }
    s
}

/// Simple glob/fnmatch matching (`*`, `?`, `[...]`) matching CPython's
/// `fnmatch.fnmatchcase` for `Filter` patterns.
fn fnmatch_case(name: &str, pattern: &str) -> bool {
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    fnmatch_rec(&n, &p)
}

fn fnmatch_rec(n: &[char], p: &[char]) -> bool {
    let (mut ni, mut pi) = (0usize, 0usize);
    let (mut star_pi, mut star_ni) = (None, 0usize);
    while ni < n.len() {
        if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_ni = ni;
            pi += 1;
        } else if pi < p.len() && p[pi] == '?' {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '[' {
            if let Some((matched, next_pi)) = match_char_class(n[ni], p, pi) {
                if matched {
                    ni += 1;
                    pi = next_pi;
                } else if let Some(sp) = star_pi {
                    star_ni += 1;
                    ni = star_ni;
                    pi = sp + 1;
                } else {
                    return false;
                }
            } else if n[ni] == '[' {
                ni += 1;
                pi += 1;
            } else if let Some(sp) = star_pi {
                star_ni += 1;
                ni = star_ni;
                pi = sp + 1;
            } else {
                return false;
            }
        } else if pi < p.len() && p[pi] == n[ni] {
            pi += 1;
            ni += 1;
        } else if let Some(sp) = star_pi {
            star_ni += 1;
            ni = star_ni;
            pi = sp + 1;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn match_char_class(ch: char, p: &[char], start: usize) -> Option<(bool, usize)> {
    let mut i = start + 1;
    if i >= p.len() {
        return None;
    }
    let negate = p[i] == '!' || p[i] == '^';
    if negate {
        i += 1;
    }
    let mut first = true;
    let mut matched = false;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((if negate { !matched } else { matched }, i + 1));
        }
        first = false;
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if ch >= p[i] && ch <= p[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if ch == p[i] {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

// ── Native state types ───────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct FrameState {
    filename: String,
    lineno: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TracebackState {
    frames: Vec<(String, i64)>,
    total_nframe: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TraceState {
    domain: i64,
    size: i64,
    frames: Vec<(String, i64)>,
    total_nframe: Option<i64>,
}

#[derive(Clone, Debug)]
struct StatisticState {
    traceback: Value,
    frames_key: Vec<(String, i64)>,
    size: i64,
    count: i64,
}

#[derive(Clone, Debug)]
struct StatisticDiffState {
    traceback: Value,
    frames_key: Vec<(String, i64)>,
    size: i64,
    size_diff: i64,
    count: i64,
    count_diff: i64,
}

#[derive(Clone, Debug)]
struct FilterState {
    inclusive: bool,
    domain: Option<i64>,
    filename_pattern: Option<String>,
    norm_pattern: Option<String>,
    lineno: Option<i64>,
    all_frames: bool,
}

#[derive(Clone, Debug)]
struct SnapshotState {
    traces: Vec<TraceState>,
    traceback_limit: i64,
}

fn extract_frame_pair(interp: &mut Interpreter, val: &Value) -> Result<(String, i64)> {
    if crate::builtin_modules::macros::native_state::has_native_state::<FrameState>(val) {
        return crate::builtin_modules::macros::native_state::with_native_state::<FrameState, _>(
            val,
            "Frame",
            |st| Ok((st.filename.clone(), st.lineno)),
        );
    }
    let items = interp.collect_iterable(val)?;
    if items.len() != 2 {
        return Err(PyError::named(
            "ValueError",
            format!(
                "frame must be a (filename, lineno) 2-tuple, got length {}",
                items.len()
            ),
        ));
    }
    let fname = items[0]
        .as_str()
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "filename must be str, not {}",
                    value_type_name_str(&items[0])
                ),
            )
        })?
        .to_owned();
    let lineno = PyIndex::try_from_value_with_interp(interp, &items[1], "Frame", "lineno")?
        .expect_i64("Frame", "lineno")?;
    Ok((fname, lineno))
}

fn extract_raw_trace(interp: &mut Interpreter, val: &Value) -> Result<TraceState> {
    if crate::builtin_modules::macros::native_state::has_native_state::<TraceState>(val) {
        return crate::builtin_modules::macros::native_state::with_native_state::<TraceState, _>(
            val,
            "Trace",
            |st| Ok(st.clone()),
        );
    }
    let items = interp.collect_iterable(val)?;
    if items.len() != 3 && items.len() != 4 {
        return Err(PyError::named(
            "ValueError",
            format!("trace tuple must have 3 or 4 items, got {}", items.len()),
        ));
    }
    let domain = PyIndex::try_from_value_with_interp(interp, &items[0], "Trace", "domain")?
        .expect_i64("Trace", "domain")?;
    let size = PyIndex::try_from_value_with_interp(interp, &items[1], "Trace", "size")?
        .expect_i64("Trace", "size")?;
    let frame_items = interp.collect_iterable(&items[2])?;
    let mut frames = Vec::with_capacity(frame_items.len());
    for f in &frame_items {
        frames.push(extract_frame_pair(interp, f)?);
    }
    let total_nframe = if items.len() == 4 && !items[3].is_none() {
        Some(
            PyIndex::try_from_value_with_interp(interp, &items[3], "Trace", "total_nframe")?
                .expect_i64("Trace", "total_nframe")?,
        )
    } else {
        None
    };
    Ok(TraceState {
        domain,
        size,
        frames,
        total_nframe,
    })
}

fn make_frame_obj(interp: &mut Interpreter, filename: &str, lineno: i64) -> Result<Value> {
    let cls = tm_class(interp, "Frame")?;
    let pair = Value::tuple(vec![Value::string(filename), Value::int(lineno)]);
    call_pos(interp, cls, &[pair])
}

fn make_traceback_obj(
    interp: &mut Interpreter,
    frames: &[(String, i64)],
    total_nframe: Option<i64>,
) -> Result<Value> {
    let cls = tm_class(interp, "Traceback")?;
    let frame_tuples = Value::tuple(
        frames
            .iter()
            .map(|(f, l)| Value::tuple(vec![Value::string(f), Value::int(*l)]))
            .collect(),
    );
    match total_nframe {
        Some(n) => call_pos(interp, cls, &[frame_tuples, Value::int(n)]),
        None => call_pos(interp, cls, &[frame_tuples]),
    }
}

fn make_trace_obj(interp: &mut Interpreter, tr: &TraceState) -> Result<Value> {
    let cls = tm_class(interp, "Trace")?;
    let raw = trace_to_raw_tuple(tr);
    call_pos(interp, cls, &[raw])
}

fn trace_to_raw_tuple(tr: &TraceState) -> Value {
    let frames_tuple = Value::tuple(
        tr.frames
            .iter()
            .map(|(f, l)| Value::tuple(vec![Value::string(f), Value::int(*l)]))
            .collect(),
    );
    match tr.total_nframe {
        Some(n) => Value::tuple(vec![
            Value::int(tr.domain),
            Value::int(tr.size),
            frames_tuple,
            Value::int(n),
        ]),
        None => Value::tuple(vec![
            Value::int(tr.domain),
            Value::int(tr.size),
            frames_tuple,
        ]),
    }
}

fn make_statistic_obj(
    interp: &mut Interpreter,
    frames: &[(String, i64)],
    size: i64,
    count: i64,
) -> Result<Value> {
    let tb = make_traceback_obj(interp, frames, None)?;
    let cls = tm_class(interp, "Statistic")?;
    call_pos(interp, cls, &[tb, Value::int(size), Value::int(count)])
}

fn make_statistic_diff_obj(
    interp: &mut Interpreter,
    frames: &[(String, i64)],
    size: i64,
    size_diff: i64,
    count: i64,
    count_diff: i64,
) -> Result<Value> {
    let tb = make_traceback_obj(interp, frames, None)?;
    let cls = tm_class(interp, "StatisticDiff")?;
    call_pos(
        interp,
        cls,
        &[
            tb,
            Value::int(size),
            Value::int(size_diff),
            Value::int(count),
            Value::int(count_diff),
        ],
    )
}

fn filter_matches_frame_impl(st: &FilterState, filename: &str, lineno: i64) -> bool {
    let Some(ref pat) = st.norm_pattern else {
        return true;
    };
    let norm = normalize_path(filename);
    if !fnmatch_case(&norm, pat) {
        return false;
    }
    match st.lineno {
        None => true,
        Some(target) => lineno == target,
    }
}

fn filter_matches_traceback(st: &FilterState, frames: &[(String, i64)]) -> bool {
    if st.all_frames {
        let hit = frames
            .iter()
            .any(|(f, l)| filter_matches_frame_impl(st, f, *l));
        if hit { st.inclusive } else { !st.inclusive }
    } else if let Some((f, l)) = frames.first() {
        filter_matches_frame_impl(st, f, *l) ^ (!st.inclusive)
    } else {
        !st.inclusive
    }
}

fn filter_matches_trace(st: &FilterState, tr: &TraceState) -> bool {
    if let Some(d) = st.domain {
        if st.filename_pattern.is_none() {
            return (tr.domain == d) ^ (!st.inclusive);
        }
        if st.inclusive {
            if tr.domain != d {
                return false;
            }
        } else if tr.domain != d {
            return true;
        }
    }
    if st.filename_pattern.is_some() {
        filter_matches_traceback(st, &tr.frames)
    } else {
        st.inclusive
    }
}

type FrameKey = Vec<(String, i64)>;
type TraceGroupMap = HashMap<FrameKey, (i64, i64)>;
type StatRow = (FrameKey, i64, i64);
type StatDiffRow = (FrameKey, i64, i64, i64, i64);

fn group_traces(traces: &[TraceState], key_type: &str, cumulative: bool) -> Result<TraceGroupMap> {
    if !matches!(key_type, "traceback" | "filename" | "lineno") {
        return Err(PyError::named(
            "ValueError",
            format!("unknown key_type: '{key_type}'"),
        ));
    }
    if cumulative && !matches!(key_type, "lineno" | "filename") {
        return Err(PyError::named(
            "ValueError",
            format!("cumulative mode cannot by used with key type '{key_type}'"),
        ));
    }

    let mut groups: HashMap<Vec<(String, i64)>, (i64, i64)> = HashMap::new();
    for tr in traces {
        if !cumulative {
            let key: Vec<(String, i64)> = match key_type {
                "traceback" => tr.frames.clone(),
                "lineno" => tr.frames.first().cloned().into_iter().collect(),
                "filename" => tr
                    .frames
                    .first()
                    .map(|(f, _)| (f.clone(), 0))
                    .into_iter()
                    .collect(),
                _ => unreachable!(),
            };
            let entry = groups.entry(key).or_insert((0, 0));
            entry.0 += tr.size;
            entry.1 += 1;
        } else {
            let mut seen: HashSet<Vec<(String, i64)>> = HashSet::new();
            for (f, l) in &tr.frames {
                let key = if key_type == "lineno" {
                    vec![(f.clone(), *l)]
                } else {
                    vec![(f.clone(), 0)]
                };
                if seen.insert(key.clone()) {
                    let entry = groups.entry(key).or_insert((0, 0));
                    entry.0 += tr.size;
                    entry.1 += 1;
                }
            }
        }
    }
    Ok(groups)
}

fn compare_frames_ord(a: &[(String, i64)], b: &[(String, i64)]) -> Ordering {
    a.cmp(b)
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("DomainFilter"),
            Value::string("Filter"),
            Value::string("Frame"),
            Value::string("Snapshot"),
            Value::string("Statistic"),
            Value::string("StatisticDiff"),
            Value::string("Trace"),
            Value::string("Traceback"),
            Value::string("clear_traces"),
            Value::string("get_object_traceback"),
            Value::string("get_traceback_limit"),
            Value::string("get_traced_memory"),
            Value::string("get_tracemalloc_memory"),
            Value::string("is_tracing"),
            Value::string("reset_peak"),
            Value::string("start"),
            Value::string("stop"),
            Value::string("take_snapshot"),
        ]),
    }

    fn start(#[default(None)] nframe: Option<PyIndex>) -> Result<Value> {
        let limit = match nframe {
            Some(v) => v.expect_i64("start", "nframe")?,
            None => 1,
        };
        crate::builtin_modules::_tracemalloc::start_tracing(_interp, limit)?;
        Ok(Value::none())
    }

    fn stop() -> Result<Value> {
        crate::builtin_modules::_tracemalloc::stop_tracing(_interp);
        Ok(Value::none())
    }

    fn is_tracing() -> Result<Value> {
        Ok(Value::bool_(_interp.features().tracemalloc_tracing()))
    }

    fn clear_traces() -> Result<Value> {
        crate::builtin_modules::_tracemalloc::clear_all_traces(_interp);
        Ok(Value::none())
    }

    fn get_traced_memory() -> Result<Value> {
        let st = _interp.features().tracemalloc();
        if !st.tracing {
            return Ok(Value::tuple(vec![Value::int(0), Value::int(0)]));
        }
        Ok(Value::tuple(vec![
            Value::int(st.current_memory as i64),
            Value::int(st.peak_memory as i64),
        ]))
    }

    fn reset_peak() -> Result<Value> {
        let mut st = _interp.features().tracemalloc_mut();
        if st.tracing {
            st.peak_memory = st.current_memory;
        }
        Ok(Value::none())
    }

    fn get_traceback_limit() -> Result<Value> {
        Ok(Value::int(
            _interp.features().tracemalloc().traceback_limit as i64,
        ))
    }

    fn get_tracemalloc_memory() -> Result<Value> {
        Ok(Value::int(
            crate::builtin_modules::_tracemalloc::tracemalloc_metadata_bytes(_interp),
        ))
    }

    fn get_object_traceback(#[positional_only] obj: PyValue) -> Result<Value> {
        let raw = crate::builtin_modules::_tracemalloc::raw_object_traceback(_interp, &obj.0);
        if raw.is_none() {
            return Ok(Value::none());
        }
        let tb_cls = tm_class(_interp, "Traceback")?;
        call_pos(_interp, tb_cls, &[raw])
    }

    fn take_snapshot() -> Result<Value> {
        let (tracing, limit) = {
            let st = _interp.features().tracemalloc();
            (st.tracing, st.traceback_limit as i64)
        };
        if !tracing {
            return Err(PyError::named(
                "RuntimeError",
                "the tracemalloc module must be tracing memory allocations to take a snapshot",
            ));
        }
        let traces = Value::list(crate::builtin_modules::_tracemalloc::raw_traces_list(_interp));
        let snap_cls = tm_class(_interp, "Snapshot")?;
        call_pos(_interp, snap_cls, &[traces, Value::int(limit)])
    }

    fn _format_size(size: PyValue, sign: PyBool) -> Result<Value> {
        let val = crate::interpreter::builtin_args::PyNumber::try_from_value_with_interp(
            _interp,
            &size.0,
            "_format_size",
            "size",
        )?
        .as_f64();
        Ok(Value::string(format_size(val, sign.0)))
    }

    // ── Frame ────────────────────────────────────────────────────────────────

    #[native_state(FrameState)]
    class Frame {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, frame: PyValue) -> Result<Value> {
            let (filename, lineno) = extract_frame_pair(_interp, &frame.0)?;
            set_state(self_val.0, FrameState { filename, lineno })?;
            Ok(Value::none())
        }

        #[getter]
        fn filename(#[state] st: &FrameState) -> Result<Value> {
            Ok(Value::string(&st.filename))
        }

        #[getter]
        fn lineno(#[state] st: &FrameState) -> Result<Value> {
            Ok(Value::int(st.lineno))
        }

        fn __eq__(#[state] st: &FrameState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st == o)))
        }

        fn __ne__(#[state] st: &FrameState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st != o)))
        }

        fn __lt__(#[state] st: &FrameState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st < o)))
        }

        fn __le__(#[state] st: &FrameState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st <= o)))
        }

        fn __gt__(#[state] st: &FrameState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st > o)))
        }

        fn __ge__(#[state] st: &FrameState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st >= o)))
        }

        fn __hash__(#[state] st: &FrameState) -> Result<Value> {
            Ok(Value::int(hash_i64(st)))
        }

        fn __str__(#[state] st: &FrameState) -> Result<Value> {
            Ok(Value::string(format!("{}:{}", st.filename, st.lineno)))
        }

        fn __repr__(#[state] st: &FrameState) -> Result<Value> {
            let fn_repr = render_value_repr(_interp, &Value::string(&st.filename))?;
            Ok(Value::string(format!(
                "<Frame filename={fn_repr} lineno={}>",
                st.lineno
            )))
        }
    }

    // ── Traceback ────────────────────────────────────────────────────────────

    #[native_state(TracebackState)]
    class Traceback {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            frames: PyValue,
            #[default(OptionalArg::Omitted)] total_nframe: OptValueRef<'_>,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&frames.0)?;
            let mut parsed = Vec::with_capacity(items.len());
            for item in &items {
                parsed.push(extract_frame_pair(_interp, item)?);
            }
            // CPython 3.12: `_tracemalloc` produces frames most-recent-first,
            // and `Traceback.__init__` reverses them to oldest-first.
            parsed.reverse();
            let total = match total_nframe.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(
                    PyIndex::try_from_value_with_interp(_interp, v, "Traceback", "total_nframe")?
                        .expect_i64("Traceback", "total_nframe")?,
                ),
            };
            set_state(
                self_val.0,
                TracebackState {
                    frames: parsed,
                    total_nframe: total,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn total_nframe(#[state] st: &TracebackState) -> Result<Value> {
            Ok(match st.total_nframe {
                Some(n) => Value::int(n),
                None => Value::none(),
            })
        }

        fn __len__(#[state] st: &TracebackState) -> Result<Value> {
            Ok(Value::int(st.frames.len() as i64))
        }

        fn __bool__(#[state] st: &TracebackState) -> Result<Value> {
            Ok(Value::bool_(!st.frames.is_empty()))
        }

        fn __getitem__(#[positional_only] self_val: PyValueRef<'_>, index: PyValue) -> Result<Value> {
            let frames = with_state(self_val.0, |st| Ok(st.frames.clone()))?;
            if let Some((start, stop, step)) = pyrust_builtins::slice::slice_fields(&index.0) {
                let (lo, hi, st) = pyrust_builtins::slice::compute_indices(
                    frames.len() as i64,
                    &start,
                    &stop,
                    &step,
                )?;
                let mut out = Vec::new();
                let mut i = lo;
                if st > 0 {
                    while i < hi {
                        let (ref f, l) = frames[i as usize];
                        out.push(make_frame_obj(_interp, f, l)?);
                        i += st;
                    }
                } else {
                    while i > hi {
                        let (ref f, l) = frames[i as usize];
                        out.push(make_frame_obj(_interp, f, l)?);
                        i += st;
                    }
                }
                return Ok(Value::tuple(out));
            }
            let idx = PyIndex::try_from_value_with_interp(_interp, &index.0, "__getitem__", "index")?
                .expect_i64("__getitem__", "index")?;
            let len = frames.len() as i64;
            let norm = if idx < 0 { idx + len } else { idx };
            if norm < 0 || norm >= len {
                return Err(PyError::named("IndexError", "tuple index out of range"));
            }
            let (ref f, l) = frames[norm as usize];
            make_frame_obj(_interp, f, l)
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let frames = with_state(self_val.0, |st| Ok(st.frames.clone()))?;
            let mut objs = Vec::with_capacity(frames.len());
            for (f, l) in &frames {
                objs.push(make_frame_obj(_interp, f, *l)?);
            }
            call_pos(_interp, Value::builtin_function("iter"), &[Value::tuple(objs)])
        }

        fn __contains__(#[state] st: &TracebackState, frame: PyValue) -> Result<Value> {
            if !crate::builtin_modules::macros::native_state::has_native_state::<FrameState>(&frame.0) {
                return Ok(Value::bool_(false));
            }
            let target = crate::builtin_modules::macros::native_state::with_native_state::<FrameState, _>(
                &frame.0,
                "Frame",
                |fs| Ok((fs.filename.clone(), fs.lineno)),
            )?;
            Ok(Value::bool_(st.frames.contains(&target)))
        }

        fn __eq__(#[state] st: &TracebackState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st.frames == o.frames)))
        }

        fn __ne__(#[state] st: &TracebackState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st.frames != o.frames)))
        }

        fn __lt__(#[state] st: &TracebackState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st.frames < o.frames)))
        }

        fn __le__(#[state] st: &TracebackState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st.frames <= o.frames)))
        }

        fn __gt__(#[state] st: &TracebackState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st.frames > o.frames)))
        }

        fn __ge__(#[state] st: &TracebackState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st.frames >= o.frames)))
        }

        fn __hash__(#[state] st: &TracebackState) -> Result<Value> {
            Ok(Value::int(hash_i64(&st.frames)))
        }

        fn __str__(#[state] st: &TracebackState) -> Result<Value> {
            match st.frames.first() {
                Some((f, l)) => Ok(Value::string(format!("{f}:{l}"))),
                None => Ok(Value::string("")),
            }
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (frames, total) = with_state(self_val.0, |st| Ok((st.frames.clone(), st.total_nframe)))?;
            let mut objs = Vec::with_capacity(frames.len());
            for (f, l) in &frames {
                objs.push(make_frame_obj(_interp, f, *l)?);
            }
            let tuple_repr = render_value_repr(_interp, &Value::tuple(objs))?;
            if let Some(n) = total {
                Ok(Value::string(format!(
                    "<Traceback {tuple_repr} total_nframe={n}>"
                )))
            } else {
                Ok(Value::string(format!("<Traceback {tuple_repr}>")))
            }
        }

        fn format(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] limit: OptValueRef<'_>,
            #[default(false.into())] most_recent_first: PyBool,
        ) -> Result<Value> {
            let frames = with_state(self_val.0, |st| Ok(st.frames.clone()))?;
            let limit_opt = match limit.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(
                    PyIndex::try_from_value_with_interp(_interp, v, "format", "limit")?
                        .expect_i64("format", "limit")?,
                ),
            };
            let mut selected: Vec<(String, i64)> = match limit_opt {
                None => frames,
                Some(lim) if lim > 0 => {
                    let take_n = (lim as usize).min(frames.len());
                    frames[frames.len() - take_n..].to_vec()
                }
                Some(lim) => {
                    let drop_n = ((-lim) as usize).min(frames.len());
                    frames[..frames.len() - drop_n].to_vec()
                }
            };
            if most_recent_first.0 {
                selected.reverse();
            }

            let getline_fn = _interp
                .load_module("linecache")
                .ok()
                .and_then(|m| _interp.get_attr(&m, "getline").ok());

            let mut lines = Vec::new();
            for (fname, lineno) in selected {
                lines.push(Value::string(format!("  File \"{fname}\", line {lineno}")));
                if let Some(ref gl) = getline_fn
                    && let Ok(src) = call_pos(
                        _interp,
                        gl.clone(),
                        &[Value::string(&fname), Value::int(lineno)],
                    )
                    && let Some(s) = src.as_str()
                {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        lines.push(Value::string(format!("    {trimmed}")));
                    }
                }
            }
            Ok(Value::list(lines))
        }
    }

    // ── Trace ────────────────────────────────────────────────────────────────

    #[native_state(TraceState)]
    class Trace {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, trace: PyValue) -> Result<Value> {
            let state = extract_raw_trace(_interp, &trace.0)?;
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn domain(#[state] st: &TraceState) -> Result<Value> {
            Ok(Value::int(st.domain))
        }

        #[getter]
        fn size(#[state] st: &TraceState) -> Result<Value> {
            Ok(Value::int(st.size))
        }

        #[getter]
        fn traceback(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (frames, total) = with_state(self_val.0, |st| Ok((st.frames.clone(), st.total_nframe)))?;
            make_traceback_obj(_interp, &frames, total)
        }

        fn __eq__(#[state] st: &TraceState, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            with_state(&other.0, |o| Ok(Value::bool_(st == o)))
        }

        fn __hash__(#[state] st: &TraceState) -> Result<Value> {
            Ok(Value::int(hash_i64(st)))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (frames, total, size) = with_state(self_val.0, |st| {
                Ok((st.frames.clone(), st.total_nframe, st.size))
            })?;
            let tb = make_traceback_obj(_interp, &frames, total)?;
            let tb_str_val = call_pos(_interp, Value::builtin_function("str"), &[tb])?;
            let tb_str = tb_str_val.as_str().unwrap_or("");
            Ok(Value::string(format!(
                "{tb_str}: {}",
                format_size(size as f64, false)
            )))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (domain, size, frames, total) = with_state(self_val.0, |st| {
                Ok((st.domain, st.size, st.frames.clone(), st.total_nframe))
            })?;
            let tb = make_traceback_obj(_interp, &frames, total)?;
            let tb_repr = render_value_repr(_interp, &tb)?;
            Ok(Value::string(format!(
                "<Trace domain={domain} size={}, traceback={tb_repr}>",
                format_size(size as f64, false)
            )))
        }
    }

    // ── Statistic ────────────────────────────────────────────────────────────

    #[native_state(StatisticState)]
    class Statistic {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            traceback: PyValue,
            size: PyIndex,
            count: PyIndex,
        ) -> Result<Value> {
            let size_i = size.expect_i64("Statistic", "size")?;
            let count_i = count.expect_i64("Statistic", "count")?;
            let frames_key = if crate::builtin_modules::macros::native_state::has_native_state::<TracebackState>(&traceback.0) {
                crate::builtin_modules::macros::native_state::with_native_state::<TracebackState, _>(
                    &traceback.0,
                    "Traceback",
                    |tb| Ok(tb.frames.clone()),
                )?
            } else {
                Vec::new()
            };
            set_state(
                self_val.0,
                StatisticState {
                    traceback: traceback.0,
                    frames_key,
                    size: size_i,
                    count: count_i,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn traceback(#[state] st: &StatisticState) -> Result<Value> {
            Ok(st.traceback.clone())
        }

        #[getter]
        fn size(#[state] st: &StatisticState) -> Result<Value> {
            Ok(Value::int(st.size))
        }

        #[getter]
        fn count(#[state] st: &StatisticState) -> Result<Value> {
            Ok(Value::int(st.count))
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            let (tb1, s1, c1) =
                with_state(self_val.0, |st| Ok((st.traceback.clone(), st.size, st.count)))?;
            let (tb2, s2, c2) =
                with_state(&other.0, |st| Ok((st.traceback.clone(), st.size, st.count)))?;
            if s1 != s2 || c1 != c2 {
                return Ok(Value::bool_(false));
            }
            let eq = _interp.values_richcompare_eq(&tb1, &tb2)?;
            Ok(Value::bool_(eq))
        }

        fn __hash__(#[state] st: &StatisticState) -> Result<Value> {
            Ok(Value::int(hash_i64(&(&st.frames_key, st.size, st.count))))
        }

        fn _sort_key(#[state] st: &StatisticState) -> Result<Value> {
            Ok(Value::tuple(vec![
                Value::int(st.size),
                Value::int(st.count),
                st.traceback.clone(),
            ]))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (tb, size, count) =
                with_state(self_val.0, |st| Ok((st.traceback.clone(), st.size, st.count)))?;
            let tb_str_val = call_pos(_interp, Value::builtin_function("str"), &[tb])?;
            let tb_str = tb_str_val.as_str().unwrap_or("");
            let mut text = format!(
                "{tb_str}: size={}, count={count}",
                format_size(size as f64, false)
            );
            if count != 0 {
                let avg = size as f64 / count as f64;
                text.push_str(&format!(", average={}", format_size(avg, false)));
            }
            Ok(Value::string(text))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (tb, size, count) =
                with_state(self_val.0, |st| Ok((st.traceback.clone(), st.size, st.count)))?;
            let tb_repr = render_value_repr(_interp, &tb)?;
            Ok(Value::string(format!(
                "<Statistic traceback={tb_repr} size={size} count={count}>"
            )))
        }
    }

    // ── StatisticDiff ────────────────────────────────────────────────────────

    #[native_state(StatisticDiffState)]
    class StatisticDiff {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            traceback: PyValue,
            size: PyIndex,
            size_diff: PyIndex,
            count: PyIndex,
            count_diff: PyIndex,
        ) -> Result<Value> {
            let size_i = size.expect_i64("StatisticDiff", "size")?;
            let size_diff_i = size_diff.expect_i64("StatisticDiff", "size_diff")?;
            let count_i = count.expect_i64("StatisticDiff", "count")?;
            let count_diff_i = count_diff.expect_i64("StatisticDiff", "count_diff")?;
            let frames_key = if crate::builtin_modules::macros::native_state::has_native_state::<TracebackState>(&traceback.0) {
                crate::builtin_modules::macros::native_state::with_native_state::<TracebackState, _>(
                    &traceback.0,
                    "Traceback",
                    |tb| Ok(tb.frames.clone()),
                )?
            } else {
                Vec::new()
            };
            set_state(
                self_val.0,
                StatisticDiffState {
                    traceback: traceback.0,
                    frames_key,
                    size: size_i,
                    size_diff: size_diff_i,
                    count: count_i,
                    count_diff: count_diff_i,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn traceback(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(st.traceback.clone())
        }

        #[getter]
        fn size(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(Value::int(st.size))
        }

        #[getter]
        fn size_diff(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(Value::int(st.size_diff))
        }

        #[getter]
        fn count(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(Value::int(st.count))
        }

        #[getter]
        fn count_diff(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(Value::int(st.count_diff))
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            if !has_state(&other.0) {
                return Ok(Value::not_implemented());
            }
            let (tb1, s1, sd1, c1, cd1) = with_state(self_val.0, |st| {
                Ok((
                    st.traceback.clone(),
                    st.size,
                    st.size_diff,
                    st.count,
                    st.count_diff,
                ))
            })?;
            let (tb2, s2, sd2, c2, cd2) = with_state(&other.0, |st| {
                Ok((
                    st.traceback.clone(),
                    st.size,
                    st.size_diff,
                    st.count,
                    st.count_diff,
                ))
            })?;
            if s1 != s2 || sd1 != sd2 || c1 != c2 || cd1 != cd2 {
                return Ok(Value::bool_(false));
            }
            let eq = _interp.values_richcompare_eq(&tb1, &tb2)?;
            Ok(Value::bool_(eq))
        }

        fn __hash__(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(Value::int(hash_i64(&(
                &st.frames_key,
                st.size,
                st.size_diff,
                st.count,
                st.count_diff,
            ))))
        }

        fn _sort_key(#[state] st: &StatisticDiffState) -> Result<Value> {
            Ok(Value::tuple(vec![
                Value::int(st.size_diff.abs()),
                Value::int(st.size),
                Value::int(st.count_diff.abs()),
                Value::int(st.count),
                st.traceback.clone(),
            ]))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (tb, size, size_diff, count, count_diff) = with_state(self_val.0, |st| {
                Ok((
                    st.traceback.clone(),
                    st.size,
                    st.size_diff,
                    st.count,
                    st.count_diff,
                ))
            })?;
            let tb_str_val = call_pos(_interp, Value::builtin_function("str"), &[tb])?;
            let tb_str = tb_str_val.as_str().unwrap_or("");
            let mut text = format!(
                "{tb_str}: size={} ({}), count={count} ({count_diff:+})",
                format_size(size as f64, false),
                format_size(size_diff as f64, true),
            );
            if count != 0 {
                let avg = size as f64 / count as f64;
                text.push_str(&format!(", average={}", format_size(avg, false)));
            }
            Ok(Value::string(text))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (tb, size, size_diff, count, count_diff) = with_state(self_val.0, |st| {
                Ok((
                    st.traceback.clone(),
                    st.size,
                    st.size_diff,
                    st.count,
                    st.count_diff,
                ))
            })?;
            let tb_repr = render_value_repr(_interp, &tb)?;
            Ok(Value::string(format!(
                "<StatisticDiff traceback={tb_repr} size={size} ({size_diff:+}) count={count} ({count_diff:+})>"
            )))
        }
    }

    // ── DomainFilter & Filter ────────────────────────────────────────────────

    #[native_state(FilterState)]
    class DomainFilter {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            inclusive: PyBool,
            domain: PyIndex,
        ) -> Result<Value> {
            let dom = domain.expect_i64("DomainFilter", "domain")?;
            set_state(
                self_val.0,
                FilterState {
                    inclusive: inclusive.0,
                    domain: Some(dom),
                    filename_pattern: None,
                    norm_pattern: None,
                    lineno: None,
                    all_frames: false,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn inclusive(#[state] st: &FilterState) -> Result<Value> {
            Ok(Value::bool_(st.inclusive))
        }

        #[getter]
        fn domain(#[state] st: &FilterState) -> Result<Value> {
            Ok(match st.domain {
                Some(d) => Value::int(d),
                None => Value::none(),
            })
        }

        fn _match(#[state] st: &FilterState, trace: PyValue) -> Result<Value> {
            let tr = extract_raw_trace(_interp, &trace.0)?;
            Ok(Value::bool_(filter_matches_trace(st, &tr)))
        }
    }

    #[native_state(FilterState)]
    class Filter("DomainFilter") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            inclusive: PyBool,
            filename_pattern: PyStr<'_>,
            #[default(OptionalArg::Omitted)] lineno: OptValueRef<'_>,
            #[default(false.into())] all_frames: PyBool,
            #[default(OptionalArg::Omitted)] domain: OptValueRef<'_>,
        ) -> Result<Value> {
            let lineno_opt = match lineno.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(
                    PyIndex::try_from_value_with_interp(_interp, v, "Filter", "lineno")?
                        .expect_i64("Filter", "lineno")?,
                ),
            };
            let domain_opt = match domain.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(
                    PyIndex::try_from_value_with_interp(_interp, v, "Filter", "domain")?
                        .expect_i64("Filter", "domain")?,
                ),
            };
            let pat = filename_pattern.0.into_owned();
            let norm = normalize_path(&pat);
            set_state(
                self_val.0,
                FilterState {
                    inclusive: inclusive.0,
                    domain: domain_opt,
                    filename_pattern: Some(pat),
                    norm_pattern: Some(norm),
                    lineno: lineno_opt,
                    all_frames: all_frames.0,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn filename_pattern(#[state] st: &FilterState) -> Result<Value> {
            Ok(Value::string(st.filename_pattern.as_deref().unwrap_or("")))
        }

        #[getter]
        fn lineno(#[state] st: &FilterState) -> Result<Value> {
            Ok(match st.lineno {
                Some(n) => Value::int(n),
                None => Value::none(),
            })
        }

        #[getter]
        fn all_frames(#[state] st: &FilterState) -> Result<Value> {
            Ok(Value::bool_(st.all_frames))
        }

        fn _match_frame_impl(
            #[state] st: &FilterState,
            filename: PyStr<'_>,
            lineno: PyIndex,
        ) -> Result<Value> {
            let l = lineno.expect_i64("_match_frame_impl", "lineno")?;
            Ok(Value::bool_(filter_matches_frame_impl(st, &filename.0, l)))
        }

        fn _match_frame(
            #[state] st: &FilterState,
            filename: PyStr<'_>,
            lineno: PyIndex,
        ) -> Result<Value> {
            let l = lineno.expect_i64("_match_frame", "lineno")?;
            Ok(Value::bool_(
                filter_matches_frame_impl(st, &filename.0, l) ^ (!st.inclusive),
            ))
        }

        fn _match_traceback(#[state] st: &FilterState, traceback: PyValue) -> Result<Value> {
            let items = _interp.collect_iterable(&traceback.0)?;
            let mut frames = Vec::with_capacity(items.len());
            for item in &items {
                frames.push(extract_frame_pair(_interp, item)?);
            }
            Ok(Value::bool_(filter_matches_traceback(st, &frames)))
        }

        fn _match(#[state] st: &FilterState, trace: PyValue) -> Result<Value> {
            let tr = extract_raw_trace(_interp, &trace.0)?;
            Ok(Value::bool_(filter_matches_trace(st, &tr)))
        }
    }

    // ── Snapshot ─────────────────────────────────────────────────────────────

    #[native_state(SnapshotState)]
    class Snapshot {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            traces: PyValue,
            traceback_limit: PyIndex,
        ) -> Result<Value> {
            let limit = traceback_limit.expect_i64("Snapshot", "traceback_limit")?;
            let items = _interp.collect_iterable(&traces.0)?;
            let mut parsed = Vec::with_capacity(items.len());
            for item in &items {
                parsed.push(extract_raw_trace(_interp, item)?);
            }
            set_state(
                self_val.0,
                SnapshotState {
                    traces: parsed,
                    traceback_limit: limit,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn traces(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let raw = with_state(self_val.0, |st| Ok(st.traces.clone()))?;
            let mut out = Vec::with_capacity(raw.len());
            for tr in &raw {
                out.push(make_trace_obj(_interp, tr)?);
            }
            Ok(Value::list(out))
        }

        #[getter]
        fn traceback_limit(#[state] st: &SnapshotState) -> Result<Value> {
            Ok(Value::int(st.traceback_limit))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (traces, limit) = with_state(self_val.0, |st| {
                let raw: Vec<Value> = st.traces.iter().map(trace_to_raw_tuple).collect();
                Ok((Value::list(raw), st.traceback_limit))
            })?;
            let cls = tm_class(_interp, "Snapshot")?;
            Ok(Value::tuple(vec![
                cls,
                Value::tuple(vec![traces, Value::int(limit)]),
            ]))
        }

        fn dump(#[positional_only] self_val: PyValueRef<'_>, filename: PyValue) -> Result<Value> {
            let open_fn = Value::builtin_function("open");
            let fp = call_pos(_interp, open_fn, &[filename.0, Value::string("wb")])?;
            let pickle_mod = _interp.load_module("pickle")?;
            let dump_fn = _interp.get_attr(&pickle_mod, "dump")?;
            let highest = _interp.get_attr(&pickle_mod, "HIGHEST_PROTOCOL")?;
            let res = call_pos(_interp, dump_fn, &[self_val.0.clone(), fp.clone(), highest]);
            if let Ok(close_fn) = _interp.get_attr(&fp, "close") {
                let _ = call_pos(_interp, close_fn, &[]);
            }
            res?;
            Ok(Value::none())
        }

        #[classmethod]
        fn load(#[positional_only] _cls: PyValueRef<'_>, filename: PyValue) -> Result<Value> {
            let open_fn = Value::builtin_function("open");
            let fp = call_pos(_interp, open_fn, &[filename.0, Value::string("rb")])?;
            let pickle_mod = _interp.load_module("pickle")?;
            let load_fn = _interp.get_attr(&pickle_mod, "load")?;
            let res = call_pos(_interp, load_fn, std::slice::from_ref(&fp));
            if let Ok(close_fn) = _interp.get_attr(&fp, "close") {
                let _ = call_pos(_interp, close_fn, &[]);
            }
            res
        }

        fn filter_traces(
            #[positional_only] self_val: PyValueRef<'_>,
            filters: PyValue,
        ) -> Result<Value> {
            if !matches!(
                filters.0.kind(),
                ValueKind::List(_) | ValueKind::Tuple(_)
            ) {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "filters must be a list of filters, not {}",
                        value_type_name_str(&filters.0)
                    ),
                ));
            }
            let filter_vals = _interp.collect_iterable(&filters.0)?;
            let mut inc = Vec::new();
            let mut exc = Vec::new();
            for fv in &filter_vals {
                if !crate::builtin_modules::macros::native_state::has_native_state::<FilterState>(fv) {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "filter must be a DomainFilter or Filter, not {}",
                            value_type_name_str(fv)
                        ),
                    ));
                }
                let fs = crate::builtin_modules::macros::native_state::with_native_state::<FilterState, _>(
                    fv,
                    "Filter",
                    |s| Ok(s.clone()),
                )?;
                if fs.inclusive {
                    inc.push(fs);
                } else {
                    exc.push(fs);
                }
            }
            let (traces, limit) =
                with_state(self_val.0, |st| Ok((st.traces.clone(), st.traceback_limit)))?;
            let mut kept = Vec::new();
            for tr in traces {
                let included = inc.is_empty() || inc.iter().any(|f| filter_matches_trace(f, &tr));
                let excluded = exc.iter().any(|f| !filter_matches_trace(f, &tr));
                if included && !excluded {
                    kept.push(trace_to_raw_tuple(&tr));
                }
            }
            let snap_cls = tm_class(_interp, "Snapshot")?;
            call_pos(_interp, snap_cls, &[Value::list(kept), Value::int(limit)])
        }

        fn statistics(
            #[positional_only] self_val: PyValueRef<'_>,
            key_type: PyStr<'_>,
            #[default(false.into())] cumulative: PyBool,
        ) -> Result<Value> {
            let traces = with_state(self_val.0, |st| Ok(st.traces.clone()))?;
            let groups = group_traces(&traces, &key_type.0, cumulative.0)?;
            let mut rows: Vec<StatRow> = groups
                .into_iter()
                .map(|(k, (sz, cnt))| (k, sz, cnt))
                .collect();
            rows.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| b.2.cmp(&a.2))
                    .then_with(|| compare_frames_ord(&b.0, &a.0))
            });
            let mut out = Vec::with_capacity(rows.len());
            for (frames, size, count) in rows {
                out.push(make_statistic_obj(_interp, &frames, size, count)?);
            }
            Ok(Value::list(out))
        }

        fn compare_to(
            #[positional_only] self_val: PyValueRef<'_>,
            old_snapshot: PyValue,
            key_type: PyStr<'_>,
            #[default(false.into())] cumulative: PyBool,
        ) -> Result<Value> {
            if !has_state(&old_snapshot.0) {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "old_snapshot must be a Snapshot, not {}",
                        value_type_name_str(&old_snapshot.0)
                    ),
                ));
            }
            let new_traces = with_state(self_val.0, |st| Ok(st.traces.clone()))?;
            let old_traces = with_state(&old_snapshot.0, |st| Ok(st.traces.clone()))?;
            let new_group = group_traces(&new_traces, &key_type.0, cumulative.0)?;
            let mut old_group = group_traces(&old_traces, &key_type.0, cumulative.0)?;

            let mut rows: Vec<StatDiffRow> = Vec::new();
            for (k, (new_sz, new_cnt)) in new_group {
                let (old_sz, old_cnt) = old_group.remove(&k).unwrap_or((0, 0));
                rows.push((k, new_sz, new_sz - old_sz, new_cnt, new_cnt - old_cnt));
            }
            for (k, (old_sz, old_cnt)) in old_group {
                rows.push((k, 0, -old_sz, 0, -old_cnt));
            }
            rows.sort_by(|a, b| {
                b.2.abs()
                    .cmp(&a.2.abs())
                    .then_with(|| b.1.cmp(&a.1))
                    .then_with(|| b.4.abs().cmp(&a.4.abs()))
                    .then_with(|| b.3.cmp(&a.3))
                    .then_with(|| compare_frames_ord(&b.0, &a.0))
            });
            let mut out = Vec::with_capacity(rows.len());
            for (frames, size, size_diff, count, count_diff) in rows {
                out.push(make_statistic_diff_obj(
                    _interp, &frames, size, size_diff, count, count_diff,
                )?);
            }
            Ok(Value::list(out))
        }
    }
}

//! `csv.Sniffer` — CPython 3.12.3 `Lib/csv.py` dialect / header heuristics.
//!
//! `_guess_quote_and_delimiter` evaluates CPython's four fixed
//! quote/delimiter regular expressions (and the doublequote probe) with
//! dedicated non-backtracking scanners that reproduce `sre`'s leftmost-match,
//! lazy `.*?` and greedy ` ?` semantics, `findall`'s resume-at-match-end rule
//! and the Unicode `\w` class; a general backtracking engine would recurse
//! once per character of a lazy span and is not needed for these shapes.  The
//! frequency-table delimiter guesser and the header vote are transcribed
//! operation for operation (dict insertion orders, the first-maximum rule of
//! `max`, and the float `consistency` steps).

use indexmap::IndexMap;

use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, is_stop_iteration_error};
use crate::value::{Value, ValueKind};

fn contains(interp: &mut Interpreter, container: &Value, item: &str) -> Result<bool> {
    let found = interp.eval_in(container.clone(), Value::string(item))?;
    interp.truthy_value(&found)
}

/// `sre`'s Unicode `\w`: `_` or `str.isalnum()`.
fn is_word(c: char) -> bool {
    c == '_' || pyrust_builtins::string::is_python_alnum(c)
}

/// `[^\w\n"\']` — the `delim` group's class.
fn is_delim_class(c: char) -> bool {
    !is_word(c) && !matches!(c, '\n' | '"' | '\'')
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'')
}

/// One `findall` match: its `quote`, `delim` and `space` groups.
struct QuoteMatch {
    quote: char,
    delim: Option<char>,
    space: bool,
}

/// How the `.*?(?P=quote)` span of a pattern must end.
#[derive(Clone, Copy)]
enum Closing {
    /// `(?P=delim)`: the opening delimiter again.
    SameDelim,
    /// `(?P<delim>[^\w\n"\'])(?P<space> ?)`: any delimiter-class character.
    AnyDelim,
    /// `(?:$|\n)` (MULTILINE): end of string or before a newline.
    LineEnd,
}

/// The four patterns, in the order `_guess_quote_and_delimiter` tries them:
/// `(leading delimiter?, closing)`.
const PATTERNS: [(bool, Closing); 4] = [
    // (?P<delim>[^\w\n"\'])(?P<space> ?)(?P<quote>["\']).*?(?P=quote)(?P=delim)
    (true, Closing::SameDelim),
    // (?:^|\n)(?P<quote>["\']).*?(?P=quote)(?P<delim>[^\w\n"\'])(?P<space> ?)
    (false, Closing::AnyDelim),
    // (?P<delim>[^\w\n"\'])(?P<space> ?)(?P<quote>["\']).*?(?P=quote)(?:$|\n)
    (true, Closing::LineEnd),
    // (?:^|\n)(?P<quote>["\']).*?(?P=quote)(?:$|\n)
    (false, Closing::LineEnd),
];

/// Lazy `.*?(?P=quote)<closing>` (DOTALL) from the character after the
/// opening quote at `qpos`: the end of the shortest successful match.
fn close_span(
    c: &[char],
    qpos: usize,
    quote: char,
    closing: Closing,
    open_delim: Option<char>,
) -> Option<(usize, Option<char>, bool)> {
    let n = c.len();
    for j in qpos + 1..n {
        if c[j] != quote {
            continue;
        }
        match closing {
            Closing::SameDelim => {
                if j + 1 < n && Some(c[j + 1]) == open_delim {
                    return Some((j + 2, None, false));
                }
            }
            Closing::AnyDelim => {
                if j + 1 < n && is_delim_class(c[j + 1]) {
                    let space = c.get(j + 2) == Some(&' ');
                    return Some((j + 2 + usize::from(space), Some(c[j + 1]), space));
                }
            }
            Closing::LineEnd => {
                if j + 1 == n || c[j + 1] == '\n' {
                    return Some((j + 1, None, false));
                }
            }
        }
    }
    None
}

/// Try one pattern anchored at `i`: the match end and its groups.
fn match_at(c: &[char], i: usize, leading: bool, closing: Closing) -> Option<(usize, QuoteMatch)> {
    if leading {
        // (?P<delim>...)(?P<space> ?)(?P<quote>...): greedy ` ?` first.
        let delim = *c.get(i)?;
        if !is_delim_class(delim) {
            return None;
        }
        for space in [true, false] {
            if space && c.get(i + 1) != Some(&' ') {
                continue;
            }
            let qpos = i + 1 + usize::from(space);
            let Some(&quote) = c.get(qpos).filter(|q| is_quote(**q)) else {
                continue;
            };
            if let Some((end, _, _)) = close_span(c, qpos, quote, closing, Some(delim)) {
                return Some((
                    end,
                    QuoteMatch {
                        quote,
                        delim: Some(delim),
                        space,
                    },
                ));
            }
        }
        return None;
    }
    // (?:^|\n): the zero-width MULTILINE `^` first, then a consumed `\n`.
    let at_line_start = i == 0 || c[i - 1] == '\n';
    for qpos in [
        at_line_start.then_some(i),
        (c.get(i) == Some(&'\n')).then_some(i + 1),
    ]
    .into_iter()
    .flatten()
    {
        let Some(&quote) = c.get(qpos).filter(|q| is_quote(**q)) else {
            continue;
        };
        if let Some((end, delim, space)) = close_span(c, qpos, quote, closing, None) {
            return Some((
                end,
                QuoteMatch {
                    quote,
                    delim,
                    space,
                },
            ));
        }
    }
    None
}

/// `regexp.findall(data)` for one of [`PATTERNS`].
fn findall(c: &[char], leading: bool, closing: Closing) -> Vec<QuoteMatch> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= c.len() {
        // Leftmost match at or after `pos` (`search`); `findall` resumes at
        // its end (one past an empty match).
        let Some((start, (end, m))) =
            (pos..=c.len()).find_map(|start| Some((start, match_at(c, start, leading, closing)?)))
        else {
            break;
        };
        out.push(m);
        pos = if end == start { end + 1 } else { end };
    }
    out
}

/// `re.compile(r"((D)|^)\W*Q[^D\n]*Q[^D\n]*Q\W*((D)|$)", re.M).search(data)`
/// with `D = re.escape(delim)` and `Q = quotechar`: three quotes inside one
/// run free of the delimiter and newlines, the first reachable through
/// non-word characters from a delimiter or a line start, the last likewise
/// from a delimiter, a newline, or the end of the data.
fn doublequote_search(c: &[char], delim: Option<char>, quote: char) -> bool {
    let n = c.len();
    let is_sep = |ch: char| ch == '\n' || Some(ch) == delim;
    // An empty delimiter makes both `(()|^)` and `(()|$)` match anywhere.
    let left_ok = |p: usize| {
        if delim.is_none() {
            return true;
        }
        let mut k = p;
        while k > 0 && !is_word(c[k - 1]) {
            if is_sep(c[k - 1]) {
                return true;
            }
            k -= 1;
        }
        k == 0
    };
    let right_ok = |p: usize| {
        if delim.is_none() {
            return true;
        }
        let mut k = p + 1;
        while k < n && !is_word(c[k]) {
            if is_sep(c[k]) {
                return true;
            }
            k += 1;
        }
        k == n
    };
    let mut i = 0;
    while i < n {
        if is_sep(c[i]) {
            i += 1;
            continue;
        }
        let seg_start = i;
        while i < n && !is_sep(c[i]) {
            i += 1;
        }
        let quotes: Vec<usize> = (seg_start..i).filter(|&p| c[p] == quote).collect();
        if quotes.len() < 3 {
            continue;
        }
        if let Some(first) = quotes.iter().position(|&p| left_ok(p))
            && quotes
                .get(first + 2..)
                .is_some_and(|rest| rest.iter().any(|&p| right_ok(p)))
        {
            return true;
        }
    }
    false
}

/// The `TypeError` `re` raises for a non-`str` subject of a `str` pattern.
fn not_a_string(data: &Value) -> PyError {
    let bytes_like = matches!(data.kind(), ValueKind::Bytes(_))
        || matches!(super::type_name(data).as_str(), "bytearray" | "memoryview");
    if bytes_like {
        super::args::type_error("cannot use a string pattern on a bytes-like object")
    } else {
        super::args::type_error(format!(
            "expected string or bytes-like object, got '{}'",
            super::type_name(data)
        ))
    }
}

/// The first key with the largest count (`max(d, key=d.get)`).
fn max_key<K: Clone>(counts: &IndexMap<K, i64>) -> Option<K> {
    let mut best: Option<(&K, i64)> = None;
    for (key, &count) in counts {
        if best.is_none_or(|(_, b)| count > b) {
            best = Some((key, count));
        }
    }
    best.map(|(key, _)| key.clone())
}

/// `a, b, ... = value`.
fn unpack(interp: &mut Interpreter, value: &Value, expected: usize) -> Result<Vec<Value>> {
    let items = interp.collect_iterable(value)?;
    if items.len() < expected {
        return Err(PyError::named(
            "ValueError",
            format!(
                "not enough values to unpack (expected {expected}, got {})",
                items.len()
            ),
        ));
    }
    if items.len() > expected {
        return Err(PyError::named(
            "ValueError",
            format!("too many values to unpack (expected {expected})"),
        ));
    }
    Ok(items)
}

/// `Sniffer.sniff(sample, delimiters=None)`.
pub(super) fn sniff(
    interp: &mut Interpreter,
    this: &Value,
    sample: &Value,
    delimiters: &Value,
) -> Result<Value> {
    let guessed = super::call_method(
        interp,
        this,
        "_guess_quote_and_delimiter",
        &[sample.clone(), delimiters.clone()],
    )?;
    let parts = unpack(interp, &guessed, 4)?;
    let (quotechar, doublequote) = (parts[0].clone(), parts[1].clone());
    let (mut delimiter, mut skipinitialspace) = (parts[2].clone(), parts[3].clone());
    if !interp.truthy_value(&delimiter)? {
        let guessed = super::call_method(
            interp,
            this,
            "_guess_delimiter",
            &[sample.clone(), delimiters.clone()],
        )?;
        let parts = unpack(interp, &guessed, 2)?;
        delimiter = parts[0].clone();
        skipinitialspace = parts[1].clone();
    }
    if !interp.truthy_value(&delimiter)? {
        return Err(super::csv_error("Could not determine delimiter"));
    }

    // class dialect(Dialect):
    //     _name = "sniffed"
    //     lineterminator = '\r\n'
    //     quoting = QUOTE_MINIMAL
    let mut ns = crate::value::PyDict::default();
    for (key, value) in [
        ("__module__", Value::string("csv")),
        (
            "__qualname__",
            Value::string("Sniffer.sniff.<locals>.dialect"),
        ),
        ("_name", Value::string("sniffed")),
        ("lineterminator", Value::string("\r\n")),
        ("quoting", Value::int(0)),
    ] {
        interp.dict_insert(&mut ns, crate::value::PyKey::str_from(key), value)?;
    }
    let base = Value::py_class(super::state()?.py_dialect);
    let dialect = super::call_builtin(
        interp,
        "type",
        &[
            Value::string("dialect"),
            Value::tuple(vec![base]),
            Value::dict(ns),
        ],
    )?;
    interp.assign_attr(dialect.clone(), "doublequote", doublequote)?;
    interp.assign_attr(dialect.clone(), "delimiter", delimiter)?;
    // _csv.reader won't accept a quotechar of ''
    let quotechar = if interp.truthy_value(&quotechar)? {
        quotechar
    } else {
        Value::string("\"")
    };
    interp.assign_attr(dialect.clone(), "quotechar", quotechar)?;
    interp.assign_attr(dialect.clone(), "skipinitialspace", skipinitialspace)?;
    Ok(dialect)
}

/// `Sniffer._guess_quote_and_delimiter(data, delimiters)`.
pub(super) fn guess_quote_and_delimiter(
    interp: &mut Interpreter,
    data: &Value,
    delimiters: &Value,
) -> Result<Value> {
    let Some(text) = super::str_text(data) else {
        return Err(not_a_string(data));
    };
    let chars: Vec<char> = text.chars().collect();
    let mut matches = Vec::new();
    for (leading, closing) in PATTERNS {
        matches = findall(&chars, leading, closing);
        if !matches.is_empty() {
            break;
        }
    }

    if matches.is_empty() {
        // (quotechar, doublequote, delimiter, skipinitialspace)
        return Ok(Value::tuple(vec![
            Value::string(""),
            Value::bool_(false),
            Value::none(),
            Value::int(0),
        ]));
    }
    let mut quotes: IndexMap<String, i64> = IndexMap::new();
    let mut delims: IndexMap<String, i64> = IndexMap::new();
    let mut spaces = 0i64;
    for m in &matches {
        *quotes.entry(m.quote.to_string()).or_insert(0) += 1;
        // The last pattern has no `delim` / `space` groups (`KeyError`).
        let Some(delim) = m.delim else {
            continue;
        };
        let key = delim.to_string();
        if delimiters.is_none() || contains(interp, delimiters, &key)? {
            *delims.entry(key).or_insert(0) += 1;
        }
        if m.space {
            spaces += 1;
        }
    }

    let Some(quotechar) = max_key(&quotes) else {
        return Err(PyError::named(
            "ValueError",
            "max() iterable argument is empty".to_string(),
        ));
    };
    let (delim, skipinitialspace) = match max_key(&delims) {
        Some(delim) => {
            let skip = delims[&delim] == spaces;
            // most likely a file with a single column
            let delim = if delim == "\n" { String::new() } else { delim };
            (delim, Value::bool_(skip))
        }
        // there is *no* delimiter, it's a single column of quoted data
        None => (String::new(), Value::int(0)),
    };

    // if we see an extra quote between delimiters, we've got a
    // double quoted format
    let quote = quotechar.chars().next().unwrap_or('"');
    let doublequote = doublequote_search(&chars, delim.chars().next(), quote);

    Ok(Value::tuple(vec![
        Value::string(quotechar),
        Value::bool_(doublequote),
        Value::string(delim),
        skipinitialspace,
    ]))
}

/// `s.count(needle)` (non-overlapping).
fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// `data[0].count(d) == data[0].count("%c " % d)`.
fn skipinitialspace_for(first_line: &str, delim: &str) -> bool {
    count(first_line, delim) == count(first_line, &format!("{delim} "))
}

/// `Sniffer._guess_delimiter(data, delimiters)`.
pub(super) fn guess_delimiter(
    interp: &mut Interpreter,
    this: &Value,
    data: &Value,
    delimiters: &Value,
) -> Result<Value> {
    // data = list(filter(None, data.split('\n')))
    let parts = super::call_method(interp, data, "split", &[Value::string("\n")])?;
    let mut lines: Vec<String> = Vec::new();
    for part in interp.collect_iterable(&parts)? {
        if !interp.truthy_value(&part)? {
            continue;
        }
        let Some(text) = super::str_text(&part) else {
            return Err(super::args::type_error(format!(
                "must be str, not {}",
                super::type_name(&part)
            )));
        };
        lines.push(text.into_owned());
    }

    let n = lines.len();
    // build frequency tables
    let chunk_length = n.min(10);
    let mut iteration = 0usize;
    // char (7-bit ASCII code) -> {frequency: number of lines}
    let mut char_frequency: IndexMap<u8, IndexMap<usize, i64>> = IndexMap::new();
    let mut modes: IndexMap<u8, (usize, i64)> = IndexMap::new();
    let mut delims: IndexMap<u8, (usize, i64)> = IndexMap::new();
    let (mut start, mut end) = (0usize, chunk_length);
    while start < n {
        iteration += 1;
        for line in &lines[start..end.min(n)] {
            let mut counts = [0usize; 127];
            for c in line.chars() {
                if let Ok(code) = u8::try_from(u32::from(c))
                    && code < 127
                {
                    counts[usize::from(code)] += 1;
                }
            }
            for (code, &freq) in (0u8..).zip(counts.iter()) {
                // must count even if frequency is 0; value is the mode
                *char_frequency
                    .entry(code)
                    .or_default()
                    .entry(freq)
                    .or_insert(0) += 1;
            }
        }

        for (&code, meta) in &char_frequency {
            let items: Vec<(usize, i64)> = meta.iter().map(|(&f, &c)| (f, c)).collect();
            if items.len() == 1 && items[0].0 == 0 {
                continue;
            }
            // get the mode of the frequencies
            if items.len() > 1 {
                let mut best = 0;
                for (i, item) in items.iter().enumerate().skip(1) {
                    if item.1 > items[best].1 {
                        best = i;
                    }
                }
                // adjust the mode - subtract the sum of all other frequencies
                let others: i64 = items
                    .iter()
                    .enumerate()
                    .filter(|&(i, _)| i != best)
                    .map(|(_, item)| item.1)
                    .sum();
                modes.insert(code, (items[best].0, items[best].1 - others));
            } else {
                modes.insert(code, items[0]);
            }
        }

        // build a list of possible delimiters
        let total = (chunk_length * iteration).min(n) as f64;
        // (rows of consistent data) / (number of rows) = 100%
        let mut consistency = 1.0f64;
        // minimum consistency threshold
        let threshold = 0.9f64;
        while delims.is_empty() && consistency >= threshold {
            for (&code, &v) in &modes {
                if v.0 > 0
                    && v.1 > 0
                    && (v.1 as f64 / total) >= consistency
                    && (delimiters.is_none()
                        || contains(interp, delimiters, &char::from(code).to_string())?)
                {
                    delims.insert(code, v);
                }
            }
            consistency -= 0.01;
        }

        if delims.len() == 1 {
            let delim = char::from(*delims.keys().next().unwrap_or(&0)).to_string();
            let skip = skipinitialspace_for(&lines[0], &delim);
            return Ok(Value::tuple(vec![Value::string(delim), Value::bool_(skip)]));
        }

        // analyze another chunkLength lines
        start = end;
        end += chunk_length;
    }

    if delims.is_empty() {
        return Ok(Value::tuple(vec![Value::string(""), Value::int(0)]));
    }

    // if there's more than one, fall back to a 'preferred' list
    if delims.len() > 1 {
        let preferred = interp.get_attr(this, "preferred")?;
        for d in interp.collect_iterable(&preferred)? {
            let Some(text) = super::str_text(&d).map(|t| t.into_owned()) else {
                // `d in delims.keys()` still hashes a non-str candidate.
                super::call_builtin(interp, "hash", std::slice::from_ref(&d))?;
                continue;
            };
            let mut chars = text.chars();
            let known = match (chars.next(), chars.next()) {
                (Some(c), None) => u8::try_from(u32::from(c))
                    .ok()
                    .is_some_and(|code| delims.contains_key(&code)),
                _ => false,
            };
            if known {
                let skip = skipinitialspace_for(&lines[0], &text);
                return Ok(Value::tuple(vec![d, Value::bool_(skip)]));
            }
        }
    }

    // nothing else indicates a preference, pick the character that
    // dominates(?)
    let mut items: Vec<((usize, i64), u8)> = delims.iter().map(|(&k, &v)| (v, k)).collect();
    items.sort();
    let delim = char::from(items.last().map_or(0, |item| item.1)).to_string();
    let skip = skipinitialspace_for(&lines[0], &delim);
    Ok(Value::tuple(vec![Value::string(delim), Value::bool_(skip)]))
}

/// One column's inferred type in `has_header`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    /// `None`: no consistent row seen yet.
    Unset,
    /// `complex`: every cell parsed as a number.
    Complex,
    /// An `int`: every cell had this length.
    Length(i64),
}

fn parses_as_complex(interp: &mut Interpreter, cell: &Value, also: &str) -> Result<bool> {
    match super::call_builtin(interp, "complex", std::slice::from_ref(cell)) {
        Ok(_) => Ok(true),
        Err(err) if err.class_name_is("ValueError") || err.class_name_is(also) => Ok(false),
        Err(err) => Err(err),
    }
}

fn cell_len(interp: &mut Interpreter, cell: &Value) -> Result<i64> {
    let len = super::call_builtin(interp, "len", std::slice::from_ref(cell))?;
    len.as_int()
        .ok_or_else(|| PyError::runtime("len() did not return an int".to_string()))
}

/// `Sniffer.has_header(sample)`.
pub(super) fn has_header(interp: &mut Interpreter, this: &Value, sample: &Value) -> Result<Value> {
    // Creates a dictionary of types of data in each column. If any
    // column is of a single type (say, integers), *except* for the first
    // row, then the first row is presumed to be labels. If the type
    // can't be determined, it is assumed to be a string in which case
    // the length of the string is the determining factor: if all of the
    // rows except for the first are the same length, it's a header.
    // Finally, a 'vote' is taken at the end for each column, adding or
    // subtracting from the likelihood of the first row being a header.
    let dialect = super::call_method(interp, this, "sniff", std::slice::from_ref(sample))?;
    let io = interp.load_module("io")?;
    let string_io = interp.get_attr(&io, "StringIO")?;
    let stream = super::call_value(interp, string_io, std::slice::from_ref(sample))?;
    let rdr = super::make_reader(interp, &super::expand_args(&[stream, dialect], &[]))?;

    let header = interp.call_next(&rdr, None)?; // assume first row is header
    let header = interp.collect_iterable(&header)?;

    let columns = header.len();
    let mut column_types: IndexMap<usize, ColumnType> =
        (0..columns).map(|i| (i, ColumnType::Unset)).collect();

    let mut checked = 0;
    loop {
        let row = match interp.call_next(&rdr, None) {
            Ok(row) => row,
            Err(err) if is_stop_iteration_error(&err) => break,
            Err(err) => return Err(err),
        };
        // arbitrary number of rows to check, to keep it sane
        if checked > 20 {
            break;
        }
        checked += 1;
        let row = interp.collect_iterable(&row)?;
        if row.len() != columns {
            continue; // skip rows that have irregular number of columns
        }
        let cols: Vec<usize> = column_types.keys().copied().collect();
        for col in cols {
            let this_type = if parses_as_complex(interp, &row[col], "OverflowError")? {
                ColumnType::Complex
            } else {
                // fallback to length of string
                ColumnType::Length(cell_len(interp, &row[col])?)
            };
            let current = column_types[&col];
            if this_type != current {
                if current == ColumnType::Unset {
                    // add new column type
                    column_types.insert(col, this_type);
                } else {
                    // type is inconsistent, remove column from consideration
                    column_types.shift_remove(&col);
                }
            }
        }
    }

    // finally, compare results against first row and "vote"
    // on whether it's a header
    let mut votes = 0i64;
    for (&col, &column_type) in &column_types {
        match column_type {
            // it's a length
            ColumnType::Length(len) => {
                if cell_len(interp, &header[col])? != len {
                    votes += 1;
                } else {
                    votes -= 1;
                }
            }
            // attempt typecast
            ColumnType::Complex => {
                if parses_as_complex(interp, &header[col], "TypeError")? {
                    votes -= 1;
                } else {
                    votes += 1;
                }
            }
            // `None(header[col])` raises `TypeError`
            ColumnType::Unset => votes += 1,
        }
    }
    Ok(Value::bool_(votes > 0))
}

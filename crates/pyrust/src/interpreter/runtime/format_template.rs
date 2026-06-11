// Template-parse cache for `str.format` / `str.format_map` (issue #2353).
//
// `format_str_template` previously re-scanned the template byte-by-byte on
// every call — locating `{…}` fields, splitting field-name / conversion /
// spec, and classifying the head as auto / index / named — so a hot
// `"…".format(…)` loop paid the full parse on each iteration.
//
// This module parses a template **once** into a `Vec<TemplateSeg>` of literal
// spans and pre-classified replacement fields, caches it keyed by the template
// content, and lets the renderer walk the cached descriptors doing only the
// per-call work (value resolution, conversion, spec expansion, `__format__`
// dispatch).
//
// ## Invisibility
//
// The cache is behaviourally invisible: a cached render produces byte-identical
// output **and identical errors** to a fresh parse, because the descriptor list
// reproduces the original scanner's exact decisions and ordering:
//
//  * Top-level structural errors (`Single '{'` / `Single '}'`) the scanner
//    raised mid-scan are stored as a trailing `TemplateSeg::Raise` placed at
//    the failure point, so earlier complete fields still render (and may raise
//    their own arg-dependent errors) first — matching CPython's left-to-right
//    `"{5} {".format()` → `IndexError` (not the trailing `Single '{'`).
//  * Accessor (`.attr` / `[key]`) handling is left to the existing
//    `apply_field_accessors`, fed the raw suffix, so every accessor error
//    (`Missing ']'`, attribute errors, subscript overflow) keeps its current
//    message and its current ordering relative to base resolution.
//  * Conversion (`!r`/`!s`/`!a`) and the unknown-conversion error fire during
//    render in field order, exactly as before.
//  * Nested-spec expansion (`{:{width}}`) is delegated to the existing
//    `expand_format_spec_positional` only when the spec actually contains `{`.
//
// ## Memory bound
//
// A per-thread content-keyed map bounded to [`TEMPLATE_CACHE_MAX`] entries.
// Each entry owns its parsed descriptors (plain `String`s + enums, no `Value`
// references), so the cache holds no interpreter state and frees cleanly. On
// reaching the cap the map is cleared wholesale (templates in real programs are
// few; the cap only guards pathological generators of distinct templates).

/// How the head segment of a replacement field selects its base value.
enum FieldHead {
    /// `{}` / `{:spec}` — auto-numbered positional.
    Auto,
    /// `{0}` / `{2.x}` — explicit positional index.
    Index(usize),
    /// `{name}` — keyword / mapping key.
    Named(String),
}

/// A pre-classified replacement field.  Everything here is template-derived and
/// call-invariant; only value resolution + spec expansion happen per call.
struct ParsedField {
    head: FieldHead,
    /// Accessor suffix (`.attr` / `[key]` chain) fed verbatim to
    /// `apply_field_accessors`.  Empty when the field has no accessors.
    accessors: String,
    /// Conversion flag (`r`/`s`/`a`, or an invalid char raised at render time).
    conversion: Option<char>,
    /// Raw format spec (text after the field's `:`).
    spec: String,
    /// Whether `spec` contains a `{`, i.e. needs per-call nested expansion.
    spec_has_braces: bool,
}

/// One token of a parsed template.
enum TemplateSeg {
    /// Literal text with `{{`/`}}` already un-escaped.
    Literal(String),
    /// A replacement field.
    Field(ParsedField),
    /// A structural error the original scanner raised at this point.  Rendering
    /// raises it here (after earlier segments ran), preserving error ordering.
    Raise(&'static str),
}

/// A fully parsed template: an ordered list of segments.
struct ParsedTemplate {
    segs: Vec<TemplateSeg>,
}

/// Maximum number of distinct templates cached per thread.  Real programs use
/// few distinct templates; this only bounds pathological distinct-template
/// generators.  On overflow the cache is cleared wholesale.
const TEMPLATE_CACHE_MAX: usize = 512;

thread_local! {
    /// Per-thread `template content -> parsed descriptors` cache.  Entries own
    /// their data (no `Value`/interpreter references), so the table is safe to
    /// hold strong across calls and frees independently.
    static TEMPLATE_CACHE: RefCell<HashMap<Box<str>, Rc<ParsedTemplate>>> =
        RefCell::new(HashMap::new());
}

/// Return the parsed descriptors for `template`, parsing + caching on first use.
fn get_or_parse_template(template: &str) -> Rc<ParsedTemplate> {
    TEMPLATE_CACHE.with(|cache| {
        if let Some(parsed) = cache.borrow().get(template) {
            return Rc::clone(parsed);
        }
        let parsed = Rc::new(parse_template(template));
        let mut map = cache.borrow_mut();
        if map.len() >= TEMPLATE_CACHE_MAX {
            map.clear();
        }
        map.insert(template.into(), Rc::clone(&parsed));
        parsed
    })
}

/// Parse `template` into [`TemplateSeg`]s, mirroring `format_str_template`'s
/// scanner exactly.  Structural errors become a trailing [`TemplateSeg::Raise`]
/// at the failure point rather than aborting the parse, so the renderer can
/// preserve left-to-right error ordering.
fn parse_template(template: &str) -> ParsedTemplate {
    let bytes = template.as_bytes();
    let mut segs: Vec<TemplateSeg> = Vec::new();
    let mut lit = String::new();
    let mut i = 0;

    macro_rules! flush_lit {
        () => {
            if !lit.is_empty() {
                segs.push(TemplateSeg::Literal(std::mem::take(&mut lit)));
            }
        };
    }

    while i < bytes.len() {
        let c = bytes[i];
        if c == b'{' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                lit.push('{');
                i += 2;
                continue;
            }
            // Find the matching '}', tracking nested braces (e.g. "{:{w}}").
            let mut depth = 1;
            let mut j = i + 1;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                j += 1;
            }
            if depth != 0 {
                flush_lit!();
                segs.push(TemplateSeg::Raise(
                    "Single '{' encountered in format string",
                ));
                return ParsedTemplate { segs };
            }
            let field = &template[i + 1..j];
            i = j + 1;

            let (field_name_full, spec) = split_field_and_spec(field);
            let (field_name, conversion) = match field_name_full.rsplit_once('!') {
                Some((name, conv)) if conv.len() == 1 => {
                    (name, Some(conv.chars().next().unwrap()))
                }
                _ => (field_name_full, None),
            };

            let (head_str, rest) = split_head_and_accessors(field_name);
            let head = if head_str.is_empty() {
                FieldHead::Auto
            } else if let Ok(n) = head_str.parse::<usize>() {
                FieldHead::Index(n)
            } else {
                FieldHead::Named(head_str.to_string())
            };

            flush_lit!();
            segs.push(TemplateSeg::Field(ParsedField {
                head,
                accessors: rest.to_string(),
                conversion,
                spec: spec.to_string(),
                spec_has_braces: spec.contains('{'),
            }));
        } else if c == b'}' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                lit.push('}');
                i += 2;
            } else {
                flush_lit!();
                segs.push(TemplateSeg::Raise(
                    "Single '}' encountered in format string",
                ));
                return ParsedTemplate { segs };
            }
        } else {
            // Walk one UTF-8 char (start byte + continuation bytes).
            let ch_start = i;
            i += 1;
            while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
                i += 1;
            }
            lit.push_str(&template[ch_start..i]);
        }
    }
    flush_lit!();
    ParsedTemplate { segs }
}

impl Interpreter {
    /// Resolve the base value for a positional/keyword field head, advancing the
    /// auto-numbering state machine.  Mirrors `format_str_template`'s inline
    /// logic so error messages and ordering are byte-identical.
    fn resolve_field_base(
        &self,
        head: &FieldHead,
        positional: &[Value],
        keyword: &[(String, Value)],
        auto_idx: &mut Option<usize>,
        saw_manual: &mut bool,
    ) -> Result<Value> {
        match head {
            FieldHead::Auto => {
                if *saw_manual {
                    return Err(pyrust_core::value_err!("cannot switch from manual field specification to automatic field numbering"));
                }
                let Some(idx) = *auto_idx else { unreachable!() };
                *auto_idx = Some(idx + 1);
                positional.get(idx).cloned().ok_or_else(|| {
                    pyrust_core::index_err!(
                        "Replacement index {idx} out of range for positional args tuple"
                    )
                })
            }
            FieldHead::Index(n) => {
                if auto_idx.is_some() && *auto_idx != Some(0) {
                    return Err(pyrust_core::value_err!("cannot switch from automatic field numbering to manual field specification"));
                }
                *saw_manual = true;
                *auto_idx = None;
                let n = *n;
                positional.get(n).cloned().ok_or_else(|| {
                    pyrust_core::index_err!(
                        "Replacement index {n} out of range for positional args tuple"
                    )
                })
            }
            FieldHead::Named(name) => keyword
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| PyError::key_error(Value::string(name))),
        }
    }

    /// Cached implementation of `str.format`.  Walks the parsed template's
    /// descriptors; behaviour is identical to the inline scanner it replaces.
    pub(crate) fn format_str_template(
        &mut self,
        template: &str,
        positional: &[Value],
        keyword: &[(String, Value)],
    ) -> Result<Value> {
        let parsed = get_or_parse_template(template);
        let mut out = String::with_capacity(template.len());
        let mut auto_idx: Option<usize> = Some(0);
        let mut saw_manual = false;

        for seg in &parsed.segs {
            match seg {
                TemplateSeg::Literal(s) => out.push_str(s),
                TemplateSeg::Raise(msg) => {
                    return Err(pyrust_core::value_err!(msg.to_string()));
                }
                TemplateSeg::Field(f) => {
                    let base = self.resolve_field_base(
                        &f.head,
                        positional,
                        keyword,
                        &mut auto_idx,
                        &mut saw_manual,
                    )?;
                    let value = apply_field_accessors(self, base, &f.accessors)?;
                    let value = self.apply_field_conversion(value, f.conversion)?;

                    let expanded_spec;
                    let spec = if f.spec_has_braces {
                        expanded_spec = expand_format_spec_positional(
                            &f.spec,
                            positional,
                            keyword,
                            &mut auto_idx,
                            &mut saw_manual,
                        )?;
                        expanded_spec.as_str()
                    } else {
                        f.spec.as_str()
                    };

                    let formatted = self.dispatch_dunder_format(&value, spec)?;
                    out.push_str(&extract_str_value(&formatted));
                }
            }
        }
        Ok(Value::string(out))
    }

    /// Cached implementation of `str.format_map`.  Like
    /// [`Self::format_str_template`] but resolves every field through
    /// `mapping[name]` and rejects positional fields, matching CPython 3.12.
    pub(crate) fn format_str_template_map(
        &mut self,
        template: &str,
        mapping: Value,
    ) -> Result<Value> {
        let parsed = get_or_parse_template(template);
        let mut out = String::with_capacity(template.len());

        for seg in &parsed.segs {
            match seg {
                TemplateSeg::Literal(s) => out.push_str(s),
                TemplateSeg::Raise(msg) => {
                    return Err(pyrust_core::value_err!(msg.to_string()));
                }
                TemplateSeg::Field(f) => {
                    // format_map does not support positional fields.
                    let name = match &f.head {
                        FieldHead::Named(name) => name,
                        FieldHead::Auto | FieldHead::Index(_) => {
                            return Err(pyrust_core::value_err!(
                                "Format string contains positional fields"
                            ));
                        }
                    };
                    let base = self.eval_index(&mapping, Value::string(name))?;
                    let value = apply_field_accessors(self, base, &f.accessors)?;
                    let value = self.apply_field_conversion(value, f.conversion)?;

                    let expanded_spec;
                    let spec = if f.spec_has_braces {
                        expanded_spec = self.expand_format_spec_map(&f.spec, &mapping)?;
                        expanded_spec.as_str()
                    } else {
                        f.spec.as_str()
                    };

                    let formatted = self.dispatch_dunder_format(&value, spec)?;
                    out.push_str(&extract_str_value(&formatted));
                }
            }
        }
        Ok(Value::string(out))
    }

    /// Apply a `str.format` conversion flag (`!r`/`!s`/`!a`) to a value.  Shared
    /// by the `format` and `format_map` renderers.  Takes `value` by value so
    /// the common no-conversion path returns it without an extra clone.
    fn apply_field_conversion(&mut self, value: Value, conversion: Option<char>) -> Result<Value> {
        Ok(match conversion {
            Some('r') => Value::string(render_instance_repr(self, &value)?),
            Some('s') => Value::string(self.render_value_as_str(&value)?),
            Some('a') => Value::string(ascii_repr_interp(self, &value)?),
            Some(c) => {
                return Err(pyrust_core::value_err!("Unknown conversion specifier {c}"));
            }
            None => value,
        })
    }

    /// Expand `{name}` references inside a `format_map` field's spec (PEP 3101
    /// one-level nesting).  Named keys only — positional fields raise.  Mirrors
    /// the inline spec-expansion that previously lived in `format_str_template_map`.
    fn expand_format_spec_map(&mut self, spec: &str, mapping: &Value) -> Result<String> {
        let sbytes = spec.as_bytes();
        let mut spec_out = String::new();
        let mut si = 0;
        while si < sbytes.len() {
            match sbytes[si] {
                b'{' if si + 1 < sbytes.len() && sbytes[si + 1] == b'{' => {
                    spec_out.push('{');
                    si += 2;
                }
                b'}' if si + 1 < sbytes.len() && sbytes[si + 1] == b'}' => {
                    spec_out.push('}');
                    si += 2;
                }
                b'{' => {
                    let ss = si + 1;
                    let se = sbytes[ss..]
                        .iter()
                        .position(|&b| b == b'}')
                        .ok_or_else(|| {
                            pyrust_core::value_err!(
                                "Single '{' encountered in format string".to_string()
                            )
                        })?
                        + ss;
                    let inner_raw = &spec[ss..se];
                    si = se + 1;
                    let inner = inner_raw
                        .split_once(':')
                        .map(|(name, _)| name)
                        .unwrap_or(inner_raw);
                    if inner.is_empty() || inner.parse::<usize>().is_ok() {
                        return Err(pyrust_core::value_err!(
                            "Format string contains positional fields"
                        ));
                    }
                    let sv = self.eval_index(mapping, Value::string(inner))?;
                    spec_out.push_str(&sv.to_py_str());
                }
                b'}' => {
                    return Err(pyrust_core::value_err!(
                        "Single '}' encountered in format string".to_string()
                    ));
                }
                _ => {
                    let ch_s = si;
                    si += 1;
                    while si < sbytes.len() && (sbytes[si] & 0xC0) == 0x80 {
                        si += 1;
                    }
                    spec_out.push_str(&spec[ch_s..si]);
                }
            }
        }
        Ok(spec_out)
    }
}

/// A format-spec fill character.
///
/// Held as a raw codepoint rather than a `char`: a lone surrogate is a legal
/// fill (`format("a", "\udce9^3")`) but not a Rust `char`, and decoding one
/// into a `char` is undefined behaviour (#3315).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FillChar(u32);

impl FillChar {
    pub(crate) const SPACE: Self = Self(' ' as u32);
    pub(crate) const ZERO: Self = Self('0' as u32);

    /// Append `n` copies of the fill to `out`.
    pub(crate) fn push_n(self, out: &mut String, n: usize) {
        match char::from_u32(self.0) {
            Some(c) => out.extend(std::iter::repeat_n(c, n)),
            None => {
                for _ in 0..n {
                    pyrust_core::push_codepoint(out, self.0);
                }
            }
        }
    }

    /// `n` copies of the fill as a new string.
    pub(crate) fn repeat(self, n: usize) -> String {
        let mut out = String::new();
        self.push_n(&mut out, n);
        out
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FormatSpec {
    fill: FillChar,
    align: Option<char>,
    /// True when the user explicitly supplied a fill character (the
    /// two-character `[fill]align` form).  When false, a subsequent `0`
    /// flag promotes the fill to `'0'`.
    fill_explicit: bool,
    sign: Option<char>,
    alt: bool,
    zero_pad: bool,
    width: usize,
    grouping: Option<char>,
    precision: Option<usize>,
    type_char: Option<char>,
}

/// Cursor over the `(byte offset, codepoint)` pairs of a format spec.
///
/// Decodes raw codepoints instead of `char`s so a lone surrogate in the spec
/// (legal as the fill, an error anywhere else) is never turned into an
/// invalid `char` (#3315).  Every structural spec character is ASCII.
struct SpecCursor<'a> {
    spec: &'a str,
    pos: usize,
}

impl SpecCursor<'_> {
    #[inline]
    fn peek(&self) -> Option<(usize, u32)> {
        let bytes = self.spec.as_bytes();
        match bytes.get(self.pos) {
            Some(&b) if b < 0x80 => Some((self.pos, u32::from(b))),
            Some(_) => {
                pyrust_core::cesu8_next_codepoint(self.spec, self.pos).map(|(cp, _)| (self.pos, cp))
            }
            None => None,
        }
    }

    /// The codepoint after the next one, without consuming anything.
    #[inline]
    fn peek_second(&self) -> Option<u32> {
        let (_, next) = pyrust_core::cesu8_next_codepoint(self.spec, self.pos)?;
        pyrust_core::cesu8_next_codepoint(self.spec, next).map(|(cp, _)| cp)
    }

    #[inline]
    fn next(&mut self) -> Option<(usize, u32)> {
        let (at, cp) = self.peek()?;
        self.pos =
            pyrust_core::cesu8_next_codepoint(self.spec, at).map_or(self.spec.len(), |(_, n)| n);
        Some((at, cp))
    }

    /// Consume the next codepoint when it is one of the ASCII `options`.
    #[inline]
    fn eat_ascii(&mut self, options: &[u8]) -> Option<char> {
        let (_, cp) = self.peek()?;
        let b = u8::try_from(cp).ok().filter(|b| options.contains(b))?;
        self.pos += 1;
        Some(char::from(b))
    }

    /// Consume an ASCII digit run, returning its byte span.
    #[inline]
    fn digit_run(&mut self) -> (usize, usize) {
        let start = self.pos;
        while self
            .spec
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_digit)
        {
            self.pos += 1;
        }
        (start, self.pos)
    }
}

fn parse_format_spec(spec: &str, type_name: &str) -> Result<FormatSpec> {
    // Walk the borrowed `&str` in place with a codepoint cursor rather than
    // collecting into a throwaway `Vec<char>`.  An f-string interpolation
    // re-parses its (usually constant) spec on every iteration of a hot loop,
    // and that per-call heap allocation+free dominated the spec path (#2357).
    // Width/precision are ASCII digit runs, so their byte spans slice the
    // original `&str` directly.
    let mut chars = SpecCursor { spec, pos: 0 };

    // fill + align: the align character (one of <>=^) must be the *second*
    // char when a fill is present.  A bare align char first means fill
    // defaults to space.  '{' and '}' are not legal fill characters (they
    // would terminate the replacement field) — guard explicitly.  Peek the
    // first two chars (without consuming the cursor) to decide.
    const ALIGNS: [u32; 4] = ['<' as u32, '>' as u32, '=' as u32, '^' as u32];
    let c0 = chars.peek().map(|(_, cp)| cp);
    let c1 = chars.peek_second();
    let (fill, align, fill_explicit) = match (c0, c1) {
        (Some(f), Some(a)) if ALIGNS.contains(&a) && f != '{' as u32 && f != '}' as u32 => {
            chars.next();
            chars.next();
            (FillChar(f), char::from_u32(a), true)
        }
        (Some(a), _) if ALIGNS.contains(&a) => {
            chars.next();
            (FillChar::SPACE, char::from_u32(a), false)
        }
        _ => (FillChar::SPACE, None, false),
    };

    // sign
    let sign = chars.eat_ascii(b"+- ");

    // alternate form '#'
    let alt = chars.eat_ascii(b"#").is_some();

    // zero-padding '0' — always consumed when present at this position.
    // Semantics depend on whether align/fill were explicit (see render).
    let zero_pad = chars.eat_ascii(b"0").is_some();

    // width — an ASCII digit run; parse it from its byte span.
    let (width_start, width_end) = chars.digit_run();
    let width: usize = if width_end > width_start {
        spec[width_start..width_end]
            .parse::<usize>()
            .map_err(|_| pyrust_core::value_err!("Too many decimal digits in format string"))?
    } else {
        0
    };

    // grouping option (',' or '_') — sits between width and precision.
    let grouping = chars.eat_ascii(b",_");

    // precision
    let precision =
        if chars.eat_ascii(b".").is_some() {
            let (prec_start, prec_end) = chars.digit_run();
            if prec_end > prec_start {
                Some(spec[prec_start..prec_end].parse::<usize>().map_err(|_| {
                    pyrust_core::value_err!("Too many decimal digits in format string")
                })?)
            } else {
                // '.' with no digits is a syntax error in CPython.
                return Err(pyrust_core::value_err!(
                    "Format specifier missing precision"
                ));
            }
        } else {
            None
        };

    // type char (must be the last character if present)
    let type_cp = chars.next().map(|(_, cp)| cp);

    if chars.next().is_some() {
        return Err(pyrust_core::value_err!(
            "Invalid format specifier '{spec}' for object of type '{type_name}'"
        ));
    }

    // A lone surrogate is never a presentation type.  It cannot be held in
    // `type_char`, so report it here exactly as the renderer reports any other
    // unknown code (`\x` + hex, CPython's `unknown_presentation_type`), after
    // the grouping/type compatibility check that CPython runs first.
    let type_char = match type_cp.map(|cp| (cp, char::from_u32(cp))) {
        None => None,
        Some((_, Some(c))) => Some(c),
        Some((cp, None)) => {
            if let Some(g) = grouping {
                return Err(pyrust_core::value_err!(
                    "Cannot specify '{g}' with '\\x{cp:x}'."
                ));
            }
            return Err(pyrust_core::value_err!(
                "Unknown format code '\\x{cp:x}' for object of type '{type_name}'"
            ));
        }
    };

    // CPython validates grouping/type compatibility at parse time, BEFORE any
    // per-value "Unknown format code" check (issue #2373): ',' allows only
    // d/e/E/f/F/g/G/% as the type code; '_' additionally allows b/o/x/X.  A
    // second separator in type position reports the doubled separator or the
    // pair ("both").  Pinned against python3.12 (",d" on a str value
    // correctly falls through to the str path's unknown-code error; the
    // incompatible type char is hex-escaped like unknown format codes).
    if let (Some(g), Some(t)) = (grouping, type_char) {
        if t == ',' || t == '_' {
            if t == g {
                return Err(pyrust_core::value_err!("Cannot specify '{g}' with '{g}'."));
            }
            return Err(pyrust_core::value_err!("Cannot specify both ',' and '_'."));
        }
        let grouping_ok = matches!(t, 'd' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%')
            || (g == '_' && matches!(t, 'b' | 'o' | 'x' | 'X'));
        if !grouping_ok {
            let t_repr = format_code_repr(t);
            return Err(pyrust_core::value_err!(
                "Cannot specify '{g}' with '{t_repr}'."
            ));
        }
    }

    Ok(FormatSpec {
        fill,
        align,
        fill_explicit,
        sign,
        alt,
        zero_pad,
        width,
        grouping,
        precision,
        type_char,
    })
}

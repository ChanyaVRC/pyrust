#[derive(Debug, Clone)]
pub(crate) struct FormatSpec {
    fill: char,
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

fn parse_format_spec(spec: &str, type_name: &str) -> Result<FormatSpec> {
    // Walk the borrowed `&str` in place with a `Peekable<CharIndices>` rather
    // than collecting into a throwaway `Vec<char>`.  An f-string interpolation
    // re-parses its (usually constant) spec on every iteration of a hot loop,
    // and that per-call heap allocation+free dominated the spec path (#2357).
    // Width/precision are ASCII digit runs, so their byte spans from
    // `char_indices` slice the original `&str` directly (byte == char index).
    let spec_len = spec.len();
    let mut chars = spec.char_indices().peekable();

    // fill + align: the align character (one of <>=^) must be the *second*
    // char when a fill is present.  A bare align char first means fill
    // defaults to space.  '{' and '}' are not legal fill characters (they
    // would terminate the replacement field) — guard explicitly.  Peek the
    // first two chars (without consuming the cursor) to decide.
    let mut head = spec.chars();
    let c0 = head.next();
    let c1 = head.next();
    let (fill, align, fill_explicit) =
        if matches!(c1, Some('<' | '>' | '=' | '^')) && !matches!(c0, Some('{' | '}')) {
            let f = c0.unwrap();
            let a = c1.unwrap();
            chars.next();
            chars.next();
            (f, Some(a), true)
        } else if matches!(c0, Some('<' | '>' | '=' | '^')) {
            let a = c0.unwrap();
            chars.next();
            (' ', Some(a), false)
        } else {
            (' ', None, false)
        };

    // sign
    let sign = match chars.peek() {
        Some(&(_, c @ ('+' | '-' | ' '))) => {
            chars.next();
            Some(c)
        }
        _ => None,
    };

    // alternate form '#'
    let alt = if matches!(chars.peek(), Some(&(_, '#'))) {
        chars.next();
        true
    } else {
        false
    };

    // zero-padding '0' — always consumed when present at this position.
    // Semantics depend on whether align/fill were explicit (see render).
    let zero_pad = if matches!(chars.peek(), Some(&(_, '0'))) {
        chars.next();
        true
    } else {
        false
    };

    // width — an ASCII digit run; capture its byte span to parse from the slice.
    let width_start = chars.peek().map(|&(i, _)| i).unwrap_or(spec_len);
    let mut width_end = width_start;
    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_digit() {
            chars.next();
            width_end = i + 1;
        } else {
            break;
        }
    }
    let width: usize = if width_end > width_start {
        spec[width_start..width_end]
            .parse::<usize>()
            .map_err(|_| pyrust_core::value_err!("Too many decimal digits in format string"))?
    } else {
        0
    };

    // grouping option (',' or '_') — sits between width and precision.
    let grouping = match chars.peek() {
        Some(&(_, c @ (',' | '_'))) => {
            chars.next();
            Some(c)
        }
        _ => None,
    };

    // precision
    let precision =
        if matches!(chars.peek(), Some(&(_, '.'))) {
            chars.next();
            let prec_start = chars.peek().map(|&(i, _)| i).unwrap_or(spec_len);
            let mut prec_end = prec_start;
            while let Some(&(i, c)) = chars.peek() {
                if c.is_ascii_digit() {
                    chars.next();
                    prec_end = i + 1;
                } else {
                    break;
                }
            }
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
    let type_char = chars.next().map(|(_, c)| c);

    if chars.next().is_some() {
        return Err(pyrust_core::value_err!(
            "Invalid format specifier '{spec}' for object of type '{type_name}'"
        ));
    }

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

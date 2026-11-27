//! A scanner equivalent to CPython 3.12's `fractions._RATIONAL_FORMAT`:
//!
//! ```text
//! \A\s*                                  # optional whitespace at the start,
//! (?P<sign>[-+]?)                        # an optional sign, then
//! (?=\d|\.\d)                            # lookahead for digit or .digit
//! (?P<num>\d*|\d+(_\d+)*)                # numerator (possibly empty)
//! (?:                                    # followed by
//!    (?:\s*/\s*(?P<denom>\d+(_\d+)*))?   # an optional denominator
//! |                                      # or
//!    (?:\.(?P<decimal>\d*|\d+(_\d+)*))?  # an optional fractional part
//!    (?:E(?P<exp>[-+]?\d+(_\d+)*))?      # and optional exponent
//! )
//! \s*\Z                                  # and optional whitespace to finish
//! ```
//!
//! compiled with `re.VERBOSE | re.IGNORECASE`.  `\d` is `str.isdecimal()`
//! (every Unicode `Nd` digit) and `\s` is `str.isspace()`, exactly as for a
//! Python `str` pattern.
//!
//! Each digit group is followed in the pattern only by whitespace, `/`, `.`,
//! `E`/`e` or the end, none of which can continue a `[\d_]` run.  So whenever
//! the whole pattern matches, every group captured the *maximal* run of
//! digits and underscores at its position, and that run must have the
//! `\d+(_\d+)*` shape.  Scanning maximal runs therefore yields the same
//! match / no-match decision and the same captures as the backtracking
//! engine, without pulling the pure-Python `re` module into `import
//! fractions`.

/// The captures of a successful `_RATIONAL_FORMAT` match.  Digit groups are
/// returned with every Unicode digit mapped to its ASCII value (what `int()`
/// does) and with their underscores kept.
pub(super) struct RationalLiteral {
    pub(super) negative: bool,
    /// The `num` group; possibly empty (`".5"`).
    pub(super) num: String,
    pub(super) denom: Option<String>,
    /// The `decimal` group: `Some("")` for `"1."`.
    pub(super) decimal: Option<String>,
    /// The `exp` group, including its optional sign.
    pub(super) exp: Option<String>,
}

fn is_space(cp: u32) -> bool {
    pyrust_builtins::string::is_python_space_cp(cp)
}

/// The ASCII digit for a Unicode decimal digit (`\d`), if `cp` is one.
fn digit(cp: u32) -> Option<char> {
    if (u32::from(b'0')..=u32::from(b'9')).contains(&cp) {
        return char::from_u32(cp);
    }
    crate::builtin_modules::unicodedata::decimal_value(cp).and_then(|d| char::from_digit(d, 10))
}

struct Scanner {
    cps: Vec<u32>,
    pos: usize,
}

impl Scanner {
    fn peek(&self) -> Option<u32> {
        self.cps.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u32> {
        self.cps.get(self.pos + offset).copied()
    }

    fn eat(&mut self, wanted: u8) -> bool {
        if self.peek() == Some(u32::from(wanted)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(is_space) {
            self.pos += 1;
        }
    }

    fn at_end_after_space(&mut self) -> bool {
        self.skip_space();
        self.pos == self.cps.len()
    }

    /// The maximal run of digits and underscores at the cursor, if it is
    /// empty or has the `\d+(_\d+)*` shape; `None` for a malformed run.
    fn digit_run(&mut self) -> Option<String> {
        let mut out = String::new();
        let mut last_was_digit = false;
        while let Some(cp) = self.peek() {
            if let Some(d) = digit(cp) {
                out.push(d);
                last_was_digit = true;
            } else if cp == u32::from(b'_') {
                if !last_was_digit {
                    return None;
                }
                out.push('_');
                last_was_digit = false;
            } else {
                break;
            }
            self.pos += 1;
        }
        if !out.is_empty() && !last_was_digit {
            return None;
        }
        Some(out)
    }

    /// A non-empty `\d+(_\d+)*` run.
    fn required_digit_run(&mut self) -> Option<String> {
        self.digit_run().filter(|run| !run.is_empty())
    }
}

/// Match `text` against `_RATIONAL_FORMAT`.
pub(super) fn match_rational(text: &str) -> Option<RationalLiteral> {
    let mut sc = Scanner {
        cps: pyrust_core::cesu8_codepoints(text).collect(),
        pos: 0,
    };
    sc.skip_space();
    let negative = if sc.eat(b'-') {
        true
    } else {
        sc.eat(b'+');
        false
    };
    // (?=\d|\.\d)
    let starts_number = match sc.peek() {
        Some(cp) if digit(cp).is_some() => true,
        Some(cp) if cp == u32::from(b'.') => sc.peek_at(1).and_then(digit).is_some(),
        _ => false,
    };
    if !starts_number {
        return None;
    }
    let num = sc.digit_run()?;
    let after_num = sc.pos;

    // First alternative: an optional `\s*/\s*denom`.
    if !num.is_empty() {
        sc.skip_space();
        if sc.eat(b'/') {
            sc.skip_space();
            if let Some(denom) = sc.required_digit_run()
                && sc.at_end_after_space()
            {
                return Some(RationalLiteral {
                    negative,
                    num,
                    denom: Some(denom),
                    decimal: None,
                    exp: None,
                });
            }
            // A malformed denominator also fails the second alternative:
            // after `num` it would need `.`, `E` or the end, not `/`.
            return None;
        }
        sc.pos = after_num;
    }

    // Second alternative: an optional `.decimal`, an optional `E exp`.
    let decimal = if sc.eat(b'.') {
        Some(sc.digit_run()?)
    } else {
        None
    };
    let exp = if sc
        .peek()
        .is_some_and(|cp| cp == u32::from(b'e') || cp == u32::from(b'E'))
    {
        sc.pos += 1;
        let mut exp = String::new();
        if sc.eat(b'-') {
            exp.push('-');
        } else if sc.eat(b'+') {
            exp.push('+');
        }
        exp.push_str(&sc.required_digit_run()?);
        Some(exp)
    } else {
        None
    };
    if !sc.at_end_after_space() {
        return None;
    }
    Some(RationalLiteral {
        negative,
        num,
        denom: None,
        decimal,
        exp,
    })
}

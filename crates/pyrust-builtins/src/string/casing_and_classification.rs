/// Unicode full case-folding (CaseFolding.txt status F and S).
/// Handles multi-char expansions (ß→ss, ligatures) that Rust's `to_lowercase` misses.
fn unicode_casefold(s: &str, is_ascii: bool) -> String {
    if is_ascii {
        return s.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        // CaseFolding.txt full folding: for most characters the fold equals the
        // lowercase mapping, so use to_lowercase and override only the documented
        // exceptions (µ→μ, ς→σ, ß→ss, ﬆ→st, Cherokee, …) where fold ≠ lowercase.
        match unicode_data::casefold_exception(c) {
            Some(folded) => out.push_str(folded),
            None => out.extend(c.to_lowercase()),
        }
    }
    out
}

fn swapcase(s: &str, is_ascii: bool) -> String {
    if is_ascii {
        return s
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_uppercase() {
            out.extend(c.to_lowercase());
        } else if c.is_lowercase() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn titlecase(s: &str, is_ascii: bool) -> String {
    if is_ascii {
        let mut out = String::with_capacity(s.len());
        let mut prev_cased = false;
        for c in s.chars() {
            if c.is_ascii_alphabetic() {
                if prev_cased {
                    out.push(c.to_ascii_lowercase());
                } else {
                    out.push(c.to_ascii_uppercase());
                }
                prev_cased = true;
            } else {
                out.push(c);
                prev_cased = false;
            }
        }
        return out;
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                push_titlecase(&mut out, c);
            }
            prev_cased = true;
        } else {
            out.push(c);
            prev_cased = false;
        }
    }
    out
}

/// Push the Unicode titlecase form of `c` onto `out`. Unlike `char::to_uppercase`,
/// this maps Lt digraphs to their titlecase form (ǆ→ǅ, ǳ→ǲ, …) and applies the
/// SpecialCasing titlecase entries (ß→Ss, ﬀ→Ff, …); for all other characters the
/// titlecase mapping equals the uppercase mapping.
fn push_titlecase(out: &mut String, c: char) {
    match unicode_data::to_titlecase(c) {
        Some(t) => out.push_str(t),
        None => out.extend(c.to_uppercase()),
    }
}

fn str_islower(s: &str, is_ascii: bool) -> bool {
    if is_ascii {
        let mut has_cased = false;
        for b in s.bytes() {
            if b.is_ascii_uppercase() {
                return false;
            }
            if b.is_ascii_lowercase() {
                has_cased = true;
            }
        }
        return has_cased;
    }
    // cesu8_codepoints: surrogate codepoints have no case; treat as uncased separator.
    let mut has_cased = false;
    for n in cesu8_codepoints(s) {
        let Some(c) = char::from_u32(n) else { continue };
        // char::is_*case tracks a newer Unicode than CPython 3.12 (Unicode 15.0);
        // codepoints assigned in 16.0+ were Cn in 15.0 and have no case.
        if unicode_data::is_assigned_after_15_0(c) {
            continue;
        }
        if c.is_uppercase() {
            return false;
        }
        if c.is_lowercase() {
            has_cased = true;
        }
    }
    has_cased
}

fn str_isupper(s: &str, is_ascii: bool) -> bool {
    if is_ascii {
        let mut has_cased = false;
        for b in s.bytes() {
            if b.is_ascii_lowercase() {
                return false;
            }
            if b.is_ascii_uppercase() {
                has_cased = true;
            }
        }
        return has_cased;
    }
    // cesu8_codepoints: surrogate codepoints have no case; treat as uncased separator.
    let mut has_cased = false;
    for n in cesu8_codepoints(s) {
        let Some(c) = char::from_u32(n) else { continue };
        // char::is_*case tracks a newer Unicode than CPython 3.12 (Unicode 15.0);
        // codepoints assigned in 16.0+ were Cn in 15.0 and have no case.
        if unicode_data::is_assigned_after_15_0(c) {
            continue;
        }
        if c.is_lowercase() {
            return false;
        }
        if c.is_uppercase() {
            has_cased = true;
        }
    }
    has_cased
}

fn str_istitle(s: &str, is_ascii: bool) -> bool {
    if is_ascii {
        let mut prev_cased = false;
        let mut has_cased = false;
        for b in s.bytes() {
            if b.is_ascii_uppercase() {
                if prev_cased {
                    return false;
                }
                prev_cased = true;
                has_cased = true;
            } else if b.is_ascii_lowercase() {
                if !prev_cased {
                    return false;
                }
                prev_cased = true;
                has_cased = true;
            } else {
                prev_cased = false;
            }
        }
        return has_cased;
    }
    // cesu8_codepoints: surrogate codepoints have no case; treat as uncased separator.
    let mut prev_cased = false;
    let mut has_cased = false;
    for n in cesu8_codepoints(s) {
        let c = match char::from_u32(n) {
            Some(c) => c,
            None => {
                prev_cased = false;
                continue;
            }
        };
        // char::is_*case / general_category track a newer Unicode than CPython
        // 3.12 (Unicode 15.0); codepoints assigned in 16.0+ were Cn in 15.0, so
        // treat them as uncased separators.
        if unicode_data::is_assigned_after_15_0(c) {
            prev_cased = false;
            continue;
        }
        // CPython's unicode_istitle treats titlecase (Lt) characters like
        // uppercase: they must start a word (follow a non-cased character).
        // Rust's char::is_uppercase covers only Lu, so test Lt explicitly.
        if c.is_uppercase() || c.general_category() == GeneralCategory::TitlecaseLetter {
            if prev_cased {
                return false; // uppercase/titlecase after cased (must follow non-cased)
            }
            prev_cased = true;
            has_cased = true;
        } else if c.is_lowercase() {
            if !prev_cased {
                return false; // lowercase after non-cased
            }
            prev_cased = true;
            has_cased = true;
        } else {
            prev_cased = false;
        }
    }
    has_cased
}

fn str_isidentifier(s: &str, is_ascii: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    if is_ascii {
        let mut bytes = s.bytes();
        let first = bytes.next().unwrap();
        if !first.is_ascii_alphabetic() && first != b'_' {
            return false;
        }
        return bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
    }
    // Use cesu8_codepoints to avoid chars() panicking on surrogate bytes.
    // A surrogate codepoint is not a valid identifier character → return false.
    //
    // Python identifiers use the Unicode XID_Start / XID_Continue properties
    // (plus `_`), not is_alphabetic / is_alphanumeric. Combining marks (Mn/Mc)
    // are XID_Continue but not alphanumeric; superscripts (²) are alphanumeric
    // but not XID_Continue.
    let mut codepoints = cesu8_codepoints(s);
    let first = match codepoints.next().and_then(char::from_u32) {
        Some(c) => c,
        None => return false, // empty or surrogate first codepoint
    };
    if !unicode_data::is_xid_start(first) {
        return false;
    }
    codepoints.all(|n| char::from_u32(n).is_some_and(unicode_data::is_xid_continue))
}

/// ASCII whitespace per Python's `str.isspace()` / `Py_UNICODE_ISSPACE`. In
/// addition to the usual ` \t\n\r\x0b\x0c`, CPython treats the C0 information
/// separators `\x1c`–`\x1f` (bidirectional class B/S) as whitespace.
#[inline]
fn is_python_space_ascii(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1f)
}

/// Python's `str.isspace()`: the fixed whitespace set used by CPython's
/// `Py_UNICODE_ISSPACE` (Unicode 15.0). This differs from Rust's
/// `char::is_whitespace`, which omits `\x1c`–`\x1f` and `\x85`.
fn is_python_space(c: char) -> bool {
    matches!(
        c as u32,
        0x09..=0x0D
            | 0x1C..=0x1F
            | 0x20
            | 0x85
            | 0xA0
            | 0x1680
            | 0x2000..=0x200A
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x205F
            | 0x3000
    )
}

/// Python's `str.isalnum()`: a character is alphanumeric when it is alphabetic
/// (`isalpha`), or has Numeric_Type Decimal (`isdecimal`), Digit (`isdigit`), or
/// Numeric (`isnumeric`). Symbol categories such as circled letters (So) are
/// none of these and are correctly excluded.
fn is_python_alnum(c: char) -> bool {
    is_python_alpha(c) || is_python_digit(c) || unicode_data::is_numeric(c)
}

// ─────────────────────────────────────────────────────────────────────────────

/// Python's str.isdigit(): Unicode Nd (DecimalNumber) category plus all codepoints with
/// Numeric_Type=Digit (category No). Authoritative list from CPython 3.12 / Unicode 15.
fn is_python_digit(c: char) -> bool {
    // Nd covers all decimal digit scripts (Arabic-Indic, Devanagari, etc.).
    // `general_category` tracks a newer Unicode than CPython 3.12 (Unicode 15.0),
    // so skip codepoints assigned in 16.0+ (Cn in 15.0) to keep parity.
    if !unicode_data::is_assigned_after_15_0(c)
        && c.general_category() == GeneralCategory::DecimalNumber
    {
        return true;
    }
    // Remaining codepoints with Numeric_Type=Digit (category No) per Unicode 15 / CPython 3.12.
    matches!(
        c as u32,
        0x00B2 | 0x00B3 | 0x00B9           // superscript 2, 3, 1
        | 0x1369..=0x1371                   // Ethiopic digits 1–9
        | 0x19DA                            // New Tai Lue Tham Digit One
        | 0x2070 | 0x2074..=0x2079         // superscript 0, 4–9
        | 0x2080..=0x2089                   // subscript 0–9
        | 0x2460..=0x2468                   // circled digits 1–9
        | 0x2474..=0x247C                   // parenthesized digits 1–9
        | 0x2488..=0x2490                   // digit full-stop 1–9
        | 0x24EA                            // circled digit 0
        | 0x24F5..=0x24FD                   // double circled digits 1–9
        | 0x24FF                            // negative circled digit 0
        | 0x2776..=0x277E                   // dingbat negative circled digits 1–9
        | 0x2780..=0x2788                   // dingbat circled sans-serif digits 1–9
        | 0x278A..=0x2792                   // dingbat negative circled sans-serif digits 1–9
        | 0x10A40..=0x10A43                 // Kharoshthi digits 1–4
        | 0x10E60..=0x10E68                 // Rumi digits 1–9
        | 0x11052..=0x1105A                 // Brahmi numbers 1–9
        | 0x1F100..=0x1F10A                 // digit full-stop/comma 0–9
    )
}

/// Python's str.isalpha(): Unicode general category L* (Letter).
///
/// `general_category` tracks a newer Unicode database than CPython 3.12
/// (Unicode 15.0); codepoints assigned in Unicode 16.0+ were `Cn` in 15.0, so
/// they must classify as non-alphabetic to stay byte-identical to python3.12.
fn is_python_alpha(c: char) -> bool {
    !unicode_data::is_assigned_after_15_0(c)
        && matches!(
            c.general_category(),
            GeneralCategory::UppercaseLetter
                | GeneralCategory::LowercaseLetter
                | GeneralCategory::TitlecaseLetter
                | GeneralCategory::ModifierLetter
                | GeneralCategory::OtherLetter
        )
}

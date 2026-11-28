//! The pure text algorithms of `textwrap`: the hand-written scanners that
//! replace CPython's `wordsep_re`, `wordsep_simple_re` and `sentence_end_re`
//! (used for the default patterns), and `dedent`'s two multiline regexes.
//!
//! Everything works on code points (`Vec<char>`), the way CPython's `str`
//! regex engine does, and is unit-tested without an interpreter.

use pyrust_builtins::string::{is_python_alnum, is_python_decimal};

/// The recognised whitespace: `_whitespace = '\t\n\x0b\x0c\r '`.
pub(super) fn is_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0b' | '\x0c' | '\r' | ' ')
}

/// `\w` of a `str` regex.
fn is_word(c: char) -> bool {
    c == '_' || is_python_alnum(c)
}

/// `letter = r'[^\d\W]'`: a word character that is not a decimal digit.
fn is_letter(c: char) -> bool {
    is_word(c) && !is_python_decimal(c)
}

/// `word_punct = r'[\w!"\'&.,?]'`.
fn is_word_punct(c: char) -> bool {
    is_word(c) || matches!(c, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

/// The chunk boundaries (`start..end` in code points) `wordsep_re.split(text)`
/// captures, whitespace runs, em-dash runs and (possibly hyphenated) words.
///
/// Every position of the text starts one of the three alternatives, so the
/// chunks tile the text exactly and `re.split`'s in-between pieces are always
/// empty (the caller filters empty strings anyway).
pub(super) fn wordsep_spans(t: &[char]) -> Vec<(usize, usize)> {
    let n = t.len();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < n {
        // `[ws]+`
        if is_ws(t[i]) {
            let mut j = i + 1;
            while j < n && is_ws(t[j]) {
                j += 1;
            }
            spans.push((i, j));
            i = j;
            continue;
        }
        // `(?<=word_punct) -{2,} (?=\w)`: an em-dash between words.
        if t[i] == '-' && i > 0 && is_word_punct(t[i - 1]) {
            let j = dash_run_end(t, i);
            if j - i >= 2 && j < n && is_word(t[j]) {
                spans.push((i, j));
                i = j;
                continue;
            }
        }
        // `[^ws]+? (?: hyphen | (?=ws|\Z) | em-dash )`: a word.
        let mut j = i + 1;
        loop {
            if j < n && t[j] == '-' {
                // `-(?: (?<=L{2}-) | (?<=L-L-)) (?= L -? L)`
                let behind_two = j >= 2 && is_letter(t[j - 1]) && is_letter(t[j - 2]);
                let behind_four =
                    j >= 3 && is_letter(t[j - 1]) && t[j - 2] == '-' && is_letter(t[j - 3]);
                if (behind_two || behind_four) && letters_follow(t, j + 1) {
                    spans.push((i, j + 1));
                    i = j + 1;
                    break;
                }
            }
            // `(?=ws|\Z)`: end of word.
            if j >= n || is_ws(t[j]) {
                spans.push((i, j));
                i = j;
                break;
            }
            // `(?<=word_punct) (?=-{2,}\w)`: just before an em-dash.
            if t[j] == '-' && is_word_punct(t[j - 1]) {
                let k = dash_run_end(t, j);
                if k - j >= 2 && k < n && is_word(t[k]) {
                    spans.push((i, j));
                    i = j;
                    break;
                }
            }
            j += 1;
        }
    }
    spans
}

/// The end of the run of `-` starting at `from`.
fn dash_run_end(t: &[char], from: usize) -> usize {
    let mut j = from;
    while j < t.len() && t[j] == '-' {
        j += 1;
    }
    j
}

/// `(?= L -? L)` at `p`.
fn letters_follow(t: &[char], p: usize) -> bool {
    let n = t.len();
    if p >= n || !is_letter(t[p]) {
        return false;
    }
    if p + 1 < n && is_letter(t[p + 1]) {
        return true;
    }
    p + 2 < n && t[p + 1] == '-' && is_letter(t[p + 2])
}

/// The chunk boundaries of `wordsep_simple_re.split(text)`: whitespace runs
/// and the non-whitespace runs between them.
pub(super) fn simple_spans(t: &[char]) -> Vec<(usize, usize)> {
    let n = t.len();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < n {
        let ws = is_ws(t[i]);
        let mut j = i + 1;
        while j < n && is_ws(t[j]) == ws {
            j += 1;
        }
        spans.push((i, j));
        i = j;
    }
    spans
}

/// `sentence_end_re.search(chunk)`: `[a-z][\.\!\?][\"\']?\Z`.
pub(super) fn sentence_end(chunk: &str) -> bool {
    let mut tail = chunk.chars().rev();
    let (Some(c1), c2, c3) = (tail.next(), tail.next(), tail.next()) else {
        return false;
    };
    let terminal = |c: char| matches!(c, '.' | '!' | '?');
    let lower = |c: char| c.is_ascii_lowercase();
    // `[a-z][.!?]` at the end.
    if terminal(c1) && c2.is_some_and(lower) {
        return true;
    }
    // `[a-z][.!?]["']` at the end.
    matches!(c1, '"' | '\'') && c2.is_some_and(terminal) && c3.is_some_and(lower)
}

/// `textwrap.dedent(text)`.
pub(super) fn dedent(text: &str) -> String {
    // `_whitespace_only_re.sub('', text)`: lines of only spaces / tabs become
    // empty.
    let mut cleaned = String::with_capacity(text.len());
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            cleaned.push('\n');
        }
        if !line.chars().all(|c| c == ' ' || c == '\t') {
            cleaned.push_str(line);
        }
    }
    // `_leading_whitespace_re.findall(text)`: the indentation of every line
    // that has a character other than space, tab or newline; the margin is
    // their longest common prefix.
    let mut margin: Option<&str> = None;
    for line in cleaned.split('\n') {
        let indent_len = line
            .char_indices()
            .find(|&(_, c)| c != ' ' && c != '\t')
            .map(|(pos, _)| pos);
        let Some(indent_len) = indent_len else {
            // No content character: not a match (empty, or all indentation).
            continue;
        };
        let indent = &line[..indent_len];
        margin = Some(match margin {
            None => indent,
            Some(m) => {
                let common = m
                    .bytes()
                    .zip(indent.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                &m[..common]
            }
        });
    }
    let Some(margin) = margin.filter(|m| !m.is_empty()) else {
        return cleaned;
    };
    // `re.sub(r'(?m)^' + margin, '', text)`.
    let mut out = String::with_capacity(cleaned.len());
    for (idx, line) in cleaned.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(line.strip_prefix(margin).unwrap_or(line));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        wordsep_spans(&chars)
            .into_iter()
            .map(|(a, b)| chars[a..b].iter().collect())
            .collect()
    }

    #[test]
    fn hyphen_and_emdash_chunks_match_the_docstring() {
        assert_eq!(
            words("Look, goof-ball -- use the -b option!"),
            [
                "Look,", " ", "goof-", "ball", " ", "--", " ", "use", " ", "the", " ", "-b", " ",
                "option!"
            ]
        );
    }

    #[test]
    fn em_dash_inside_a_word_is_split_off() {
        assert_eq!(words("foo--bar"), ["foo", "--", "bar"]);
        assert_eq!(words("foo-bar-baz"), ["foo-", "bar-", "baz"]);
        assert_eq!(words("a-b"), ["a-b"]);
        assert_eq!(words("-leading trailing-"), ["-leading", " ", "trailing-"]);
    }

    #[test]
    fn sentence_endings() {
        assert!(sentence_end("foo."));
        assert!(sentence_end("foo?\""));
        assert!(sentence_end("a!'"));
        assert!(sentence_end("Foo."));
        assert!(!sentence_end("FOO."));
        assert!(!sentence_end("1."));
        assert!(!sentence_end("."));
        assert!(!sentence_end("foo.x"));
    }

    #[test]
    fn dedent_cases() {
        assert_eq!(dedent("  a\n   b\n"), "a\n b\n");
        assert_eq!(dedent("  a\n\tb\n"), "  a\n\tb\n");
        assert_eq!(dedent("  a\n   \n  b"), "a\n\nb");
        assert_eq!(dedent("   \n"), "\n");
        assert_eq!(dedent(""), "");
        assert_eq!(dedent(" \ta\n \t b"), "a\n b");
    }
}

/// Measure the leading indentation of a logical line in `chars` starting at
/// `start`, implementing CPython 3.12's 8-column tab expansion alongside the
/// alternate 1-column tab counter (`altcol`) used to detect inconsistent
/// tab/space mixing (`TabError`).
///
/// Returns `(char_count, col, altcol)` where `char_count` is the number of
/// leading whitespace characters consumed, `col` is the 8-column visual indent,
/// and `altcol` is the 1-column tab indent.
fn count_indent_chars(chars: &[char], start: usize) -> (usize, usize, usize) {
    let mut pos = start;
    let mut col = 0usize;
    let mut altcol = 0usize;
    loop {
        match chars.get(pos) {
            Some(&' ') => {
                col += 1;
                altcol += 1;
                pos += 1;
            }
            Some(&'\t') => {
                col = (col / 8 + 1) * 8;
                altcol += 1;
                pos += 1;
            }
            Some(&'\x0c') => {
                // Formfeed resets column counters in CPython's tokenizer.
                col = 0;
                altcol = 0;
                pos += 1;
            }
            _ => break,
        }
    }
    (pos - start, col, altcol)
}

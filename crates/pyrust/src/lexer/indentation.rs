/// Count the leading spaces in `chars` starting at `start`.
/// Tabs are rejected (same rule as before).  Stops at any non-space, non-tab
/// character (including '\n') so it is safe to call on the full source array.
fn count_indent_chars(chars: &[char], start: usize) -> Result<usize> {
    let mut count = 0;
    let mut pos = start;
    loop {
        match chars.get(pos) {
            Some(&' ') => {
                count += 1;
                pos += 1;
            }
            Some(&'\t') => {
                return Err(PyError::Lex(
                    "tabs are not supported; use spaces".to_string(),
                ));
            }
            _ => break,
        }
    }
    Ok(count)
}

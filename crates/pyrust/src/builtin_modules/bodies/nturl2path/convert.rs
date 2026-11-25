//! Native Rust implementation of CPython 3.12's `nturl2path` module.
//!
//! Reference: CPython 3.12 `Lib/nturl2path.py`.

use crate::error::{PyError, Result};

/// Percent-decode a URL string using UTF-8 (matching `urllib.parse.unquote`).
fn unquote(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out_bytes: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out_bytes.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out_bytes.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out_bytes).into_owned()
}

/// Percent-encode a string using `urllib.parse.quote(s, safe='/')`.
fn quote(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        let is_safe = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/');
        if is_safe {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// CPython 3.12 `nturl2path.url2pathname(url)`.
pub(super) fn url2pathname(url: &str) -> Result<String> {
    // Windows itself uses ":" even in URLs.
    let url_replaced = url.replace(':', "|");
    if !url_replaced.contains('|') {
        // No drive specifier, just convert slashes.
        let slice = if let Some(rest) = url_replaced.strip_prefix("////") {
            // path is something like ////host/path/on/remote/host
            // convert this to \\host\path\on\remote\host
            format!("//{rest}")
        } else {
            url_replaced
        };
        let converted = slice.replace('/', "\\");
        return Ok(unquote(&converted));
    }

    let comp: Vec<&str> = url_replaced.split('|').collect();
    let last_char = comp[0].chars().last();
    if comp.len() != 2 || !last_char.is_some_and(|c| c.is_ascii_alphabetic()) {
        return Err(PyError::named(
            "OSError",
            format!("Bad URL: {url_replaced}"),
        ));
    }
    let drive = last_char.unwrap().to_ascii_uppercase();
    let mut path = format!("{drive}:");
    for part in comp[1].split('/') {
        if !part.is_empty() {
            path.push('\\');
            path.push_str(&unquote(part));
        }
    }
    // Issue #11474 - handling url such as |c/|
    if path.ends_with(':') && url_replaced.ends_with('/') {
        path.push('\\');
    }
    Ok(path)
}

/// CPython 3.12 `nturl2path.pathname2url(p)`.
pub(super) fn pathname2url(p: &str) -> Result<String> {
    // Clean up extended-length path prefixes first.
    let cleaned = if let Some(after_prefix) = p.strip_prefix(r"\\?\") {
        if after_prefix.len() >= 4 && after_prefix[..4].eq_ignore_ascii_case(r"UNC\") {
            format!(r"\{}", &after_prefix[3..])
        } else if after_prefix.as_bytes().get(1) != Some(&b':') {
            return Err(PyError::named(
                "OSError",
                format!("Bad path: {after_prefix}"),
            ));
        } else {
            after_prefix.to_string()
        }
    } else {
        p.to_string()
    };

    if !cleaned.contains(':') {
        // No drive specifier, just convert slashes and quote.
        let prefixed = if cleaned.starts_with(r"\\") {
            format!(r"\\{cleaned}")
        } else {
            cleaned
        };
        let joined = prefixed.replace('\\', "/");
        return Ok(quote(&joined));
    }

    let comp: Vec<&str> = cleaned.splitn(3, ':').collect();
    if comp.len() != 2 || comp[0].chars().count() > 1 {
        return Err(PyError::named("OSError", format!("Bad path: {cleaned}")));
    }
    let drive = quote(&comp[0].to_uppercase());
    let mut path = format!("///{drive}:");
    for part in comp[1].split('\\') {
        if !part.is_empty() {
            path.push('/');
            path.push_str(&quote(part));
        }
    }
    Ok(path)
}

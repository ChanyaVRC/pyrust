// XML plist (`FMT_XML`) parser and serializer for `plistlib` (issue #3259).
// Included directly into `bodies/plistlib.rs`.

const PLIST_XML_HEADER: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";

fn is_fmt_xml(header: &[u8]) -> bool {
    const PREFIXES: [&[u8]; 2] = [b"<?xml", b"<plist"];
    for pfx in PREFIXES {
        if header.starts_with(pfx) {
            return true;
        }
    }
    // UTF-8 BOM
    if let Some(rest) = header.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        for pfx in PREFIXES {
            if rest.starts_with(pfx) {
                return true;
            }
        }
    }
    // UTF-16-BE BOM
    if let Some(rest) = header.strip_prefix(&[0xFE, 0xFF]) {
        for pfx in PREFIXES {
            let encoded: Vec<u8> = pfx.iter().flat_map(|&b| [0u8, b]).collect();
            if rest.starts_with(&encoded) {
                return true;
            }
        }
    }
    // UTF-16-LE BOM
    if let Some(rest) = header.strip_prefix(&[0xFF, 0xFE]) {
        for pfx in PREFIXES {
            let encoded: Vec<u8> = pfx.iter().flat_map(|&b| [b, 0u8]).collect();
            if rest.starts_with(&encoded) {
                return true;
            }
        }
    }
    false
}

fn decode_xml_bytes(raw: &[u8]) -> Result<String> {
    let text = if let Some(rest) = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        std::str::from_utf8(rest)
            .map_err(|_| PyError::named("UnicodeDecodeError", "invalid UTF-8 in XML plist"))?
            .to_string()
    } else if let Some(rest) = raw.strip_prefix(&[0xFE, 0xFF]) {
        if !rest.len().is_multiple_of(2) {
            return Err(PyError::named(
                "UnicodeDecodeError",
                "truncated UTF-16-BE XML plist",
            ));
        }
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_be_bytes(c))
            .collect();
        String::from_utf16(&units)
            .map_err(|_| PyError::named("UnicodeDecodeError", "invalid UTF-16-BE in XML plist"))?
    } else if let Some(rest) = raw.strip_prefix(&[0xFF, 0xFE]) {
        if !rest.len().is_multiple_of(2) {
            return Err(PyError::named(
                "UnicodeDecodeError",
                "truncated UTF-16-LE XML plist",
            ));
        }
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        String::from_utf16(&units)
            .map_err(|_| PyError::named("UnicodeDecodeError", "invalid UTF-16-LE in XML plist"))?
    } else {
        std::str::from_utf8(raw)
            .map_err(|_| PyError::named("UnicodeDecodeError", "invalid UTF-8 in XML plist"))?
            .to_string()
    };
    // Normalize XML 1.0 line endings (\r\n and \r -> \n).
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

// ── Base64 codec (RFC 4648, matching CPython binascii.b2a_base64 / a2b_base64) ─

const B64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode_base64_lines(data: &[u8], max_line_length: usize) -> Vec<Vec<u8>> {
    let max_bin_size = (max_line_length / 4) * 3;
    let chunk_size = max_bin_size.max(3);
    let mut lines = Vec::new();
    for chunk in data.chunks(chunk_size) {
        let mut line = Vec::with_capacity(chunk.len().div_ceil(3) * 4);
        let mut i = 0;
        while i < chunk.len() {
            let b0 = chunk[i] as u32;
            let b1 = if i + 1 < chunk.len() {
                chunk[i + 1] as u32
            } else {
                0
            };
            let b2 = if i + 2 < chunk.len() {
                chunk[i + 2] as u32
            } else {
                0
            };
            let triple = (b0 << 16) | (b1 << 8) | b2;
            line.push(B64_CHARS[((triple >> 18) & 0x3F) as usize]);
            line.push(B64_CHARS[((triple >> 12) & 0x3F) as usize]);
            if i + 1 < chunk.len() {
                line.push(B64_CHARS[((triple >> 6) & 0x3F) as usize]);
            } else {
                line.push(b'=');
            }
            if i + 2 < chunk.len() {
                line.push(B64_CHARS[(triple & 0x3F) as usize]);
            } else {
                line.push(b'=');
            }
            i += 3;
        }
        lines.push(line);
    }
    lines
}

fn decode_base64_str(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity((s.len() * 3) / 4);
    let mut buf = [0u8; 4];
    let mut count = 0usize;
    let mut pad = 0usize;

    for b in s.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad += 1;
                buf[count] = 0;
                count += 1;
                if count == 4 {
                    let v0 = buf[0] as u32;
                    let v1 = buf[1] as u32;
                    let v2 = buf[2] as u32;
                    let triple = (v0 << 18) | (v1 << 12) | (v2 << 6);
                    out.push(((triple >> 16) & 0xFF) as u8);
                    if pad == 1 {
                        out.push(((triple >> 8) & 0xFF) as u8);
                    }
                    count = 0;
                    pad = 0;
                }
                continue;
            }
            _ => continue, // Matches CPython binascii.a2b_base64 skipping non-alphabet bytes
        };
        if pad > 0 {
            return Err(PyError::named("ValueError", "invalid base64 padding"));
        }
        buf[count] = val;
        count += 1;
        if count == 4 {
            let triple = ((buf[0] as u32) << 18)
                | ((buf[1] as u32) << 12)
                | ((buf[2] as u32) << 6)
                | (buf[3] as u32);
            out.push(((triple >> 16) & 0xFF) as u8);
            out.push(((triple >> 8) & 0xFF) as u8);
            out.push((triple & 0xFF) as u8);
            count = 0;
        }
    }
    if count != 0 {
        return Err(PyError::named("ValueError", "Incorrect padding"));
    }
    Ok(out)
}

// ── ISO 8601 date parsing for XML plist ──────────────────────────────────────

fn parse_xml_date(interp: &mut Interpreter, s: &str) -> Result<Value> {
    let bytes = s.as_bytes();
    if !bytes.ends_with(b"Z") {
        return Err(PyError::named(
            "ValueError",
            format!("invalid plist date: {s:?}"),
        ));
    }
    let body = &s[..s.len() - 1];
    let parse_digits = |part: &str, expected: usize| -> Option<i64> {
        if part.len() != expected || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse::<i64>().ok()
    };

    let (date_part, time_part) = match body.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (body, None),
    };
    let date_fields: Vec<&str> = date_part.split('-').collect();
    if date_fields.is_empty() || date_fields.len() > 3 {
        return Err(PyError::named(
            "ValueError",
            format!("invalid plist date: {s:?}"),
        ));
    }
    let mut args: Vec<Value> = Vec::with_capacity(6);
    let y = parse_digits(date_fields[0], 4)
        .ok_or_else(|| PyError::named("ValueError", format!("invalid plist date: {s:?}")))?;
    args.push(Value::int(y));
    if date_fields.len() >= 2 {
        let m = parse_digits(date_fields[1], 2)
            .ok_or_else(|| PyError::named("ValueError", format!("invalid plist date: {s:?}")))?;
        args.push(Value::int(m));
    }
    if date_fields.len() >= 3 {
        let d = parse_digits(date_fields[2], 2)
            .ok_or_else(|| PyError::named("ValueError", format!("invalid plist date: {s:?}")))?;
        args.push(Value::int(d));
    }
    if let Some(t) = time_part {
        if date_fields.len() != 3 {
            return Err(PyError::named(
                "ValueError",
                format!("invalid plist date: {s:?}"),
            ));
        }
        let time_fields: Vec<&str> = t.split(':').collect();
        if time_fields.is_empty() || time_fields.len() > 3 {
            return Err(PyError::named(
                "ValueError",
                format!("invalid plist date: {s:?}"),
            ));
        }
        for tf in time_fields {
            let v = parse_digits(tf, 2).ok_or_else(|| {
                PyError::named("ValueError", format!("invalid plist date: {s:?}"))
            })?;
            args.push(Value::int(v));
        }
    }
    let dt_cls = get_datetime_class(interp)?;
    call_pos(interp, dt_cls, &args)
}

// ── XML Plist Parser ─────────────────────────────────────────────────────────

fn unescape_xml_text(raw: &str) -> Result<String> {
    if !raw.contains('&') {
        return Ok(raw.to_string());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch != '&' {
            out.push(ch);
            continue;
        }
        let rest = &raw[idx + 1..];
        let Some(semi) = rest.find(';') else {
            return Err(PyError::named(
                "ValueError",
                "unterminated XML entity reference",
            ));
        };
        let entity = &rest[..semi];
        match entity {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            s if s.starts_with("#x") || s.starts_with("#X") => {
                let cp = u32::from_str_radix(&s[2..], 16)
                    .map_err(|_| PyError::named("ValueError", "invalid hex char reference"))?;
                let c = char::from_u32(cp)
                    .ok_or_else(|| PyError::named("ValueError", "invalid unicode code point"))?;
                out.push(c);
            }
            s if s.starts_with('#') => {
                let cp = s[1..]
                    .parse::<u32>()
                    .map_err(|_| PyError::named("ValueError", "invalid decimal char reference"))?;
                let c = char::from_u32(cp)
                    .ok_or_else(|| PyError::named("ValueError", "invalid unicode code point"))?;
                out.push(c);
            }
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("undefined XML entity: &{entity};"),
                ));
            }
        }
        let skip_until = idx + 1 + semi;
        while let Some(&(next_i, _)) = chars.peek() {
            chars.next();
            if next_i >= skip_until {
                break;
            }
        }
    }
    Ok(out)
}

enum XmlContainer {
    Dict(Value),
    Array(Value),
}

struct XmlPlistParser<'a> {
    src: &'a str,
    pos: usize,
    line: usize,
    dict_type: Value,
    stack: Vec<XmlContainer>,
    tag_stack: Vec<String>,
    current_key: Option<String>,
    root: Option<Value>,
    data_buf: String,
}

impl<'a> XmlPlistParser<'a> {
    fn new(src: &'a str, dict_type: Value) -> Self {
        Self {
            src,
            pos: 0,
            line: 1,
            dict_type,
            stack: Vec::new(),
            tag_stack: Vec::new(),
            current_key: None,
            root: None,
            data_buf: String::new(),
        }
    }

    fn remaining(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn advance(&mut self, n: usize) {
        let slice = &self.src[self.pos..self.pos + n];
        self.line += slice.bytes().filter(|&b| b == b'\n').count();
        self.pos += n;
    }

    fn add_object(&mut self, interp: &mut Interpreter, value: Value) -> Result<()> {
        if let Some(key) = self.current_key.take() {
            match self.stack.last() {
                Some(XmlContainer::Dict(d)) => {
                    call_method(interp, d, "__setitem__", &[Value::string(key), value])?;
                }
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("unexpected element at line {}", self.line),
                    ));
                }
            }
        } else if self.stack.is_empty() {
            self.root = Some(value);
        } else {
            match self.stack.last() {
                Some(XmlContainer::Array(arr)) => {
                    call_method(interp, arr, "append", &[value])?;
                }
                _ => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("unexpected element at line {}", self.line),
                    ));
                }
            }
        }
        Ok(())
    }

    fn handle_begin(&mut self, interp: &mut Interpreter, tag: &str) -> Result<()> {
        self.data_buf.clear();
        match tag {
            "dict" => {
                let d = call_pos(interp, self.dict_type.clone(), &[])?;
                self.add_object(interp, d.clone())?;
                self.stack.push(XmlContainer::Dict(d));
            }
            "array" => {
                let a = Value::list(Vec::new());
                self.add_object(interp, a.clone())?;
                self.stack.push(XmlContainer::Array(a));
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_end(&mut self, interp: &mut Interpreter, tag: &str) -> Result<()> {
        match tag {
            "dict" => {
                if let Some(k) = &self.current_key {
                    return Err(PyError::named(
                        "ValueError",
                        format!("missing value for key '{}' at line {}", k, self.line),
                    ));
                }
                self.stack.pop();
            }
            "key" => {
                if self.current_key.is_some()
                    || !matches!(self.stack.last(), Some(XmlContainer::Dict(_)))
                {
                    return Err(PyError::named(
                        "ValueError",
                        format!("unexpected key at line {}", self.line),
                    ));
                }
                self.current_key = Some(std::mem::take(&mut self.data_buf));
            }
            "array" => {
                self.stack.pop();
            }
            "true" => {
                self.add_object(interp, Value::bool_(true))?;
            }
            "false" => {
                self.add_object(interp, Value::bool_(false))?;
            }
            "integer" => {
                let raw = std::mem::take(&mut self.data_buf);
                let trimmed = raw.trim();
                let val = if let Some(hex) = trimmed
                    .strip_prefix("0x")
                    .or_else(|| trimmed.strip_prefix("0X"))
                {
                    parse_int_radix(hex, 16, false)?
                } else if let Some(hex) = trimmed
                    .strip_prefix("-0x")
                    .or_else(|| trimmed.strip_prefix("-0X"))
                {
                    parse_int_radix(hex, 16, true)?
                } else if let Some(hex) = trimmed
                    .strip_prefix("+0x")
                    .or_else(|| trimmed.strip_prefix("+0X"))
                {
                    parse_int_radix(hex, 16, false)?
                } else {
                    parse_int_decimal(trimmed)?
                };
                self.add_object(interp, val)?;
            }
            "real" => {
                let raw = std::mem::take(&mut self.data_buf);
                let float_cls = crate::interpreter::primitive_class_by_name("float")
                    .map(Value::py_class)
                    .unwrap_or_else(|| Value::builtin_function("float"));
                let f = call_pos(interp, float_cls, &[Value::string(raw)])?;
                self.add_object(interp, f)?;
            }
            "string" => {
                let s = std::mem::take(&mut self.data_buf);
                self.add_object(interp, Value::string(s))?;
            }
            "data" => {
                let raw = std::mem::take(&mut self.data_buf);
                let bytes = decode_base64_str(&raw)?;
                self.add_object(interp, Value::bytes(bytes))?;
            }
            "date" => {
                let raw = std::mem::take(&mut self.data_buf);
                let dt = parse_xml_date(interp, &raw)?;
                self.add_object(interp, dt)?;
            }
            _ => {}
        }
        self.data_buf.clear();
        Ok(())
    }

    fn parse(&mut self, interp: &mut Interpreter) -> Result<Value> {
        let mut saw_root_element = false;
        while self.pos < self.src.len() {
            let rem = self.remaining();
            if let Some(lt) = rem.find('<') {
                if lt > 0 {
                    let text = &rem[..lt];
                    if !self.tag_stack.is_empty() {
                        self.data_buf.push_str(&unescape_xml_text(text)?);
                    } else if !text.trim().is_empty() {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    }
                    self.advance(lt);
                }
                let rem = self.remaining();
                if rem.starts_with("<?") {
                    // Processing instruction (e.g. <?xml ...?>)
                    let Some(end) = rem.find("?>") else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    self.advance(end + 2);
                } else if rem.starts_with("<!--") {
                    let Some(end) = rem.find("-->") else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    self.advance(end + 3);
                } else if rem.starts_with("<![CDATA[") {
                    let Some(end) = rem.find("]]>") else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    self.data_buf.push_str(&rem[9..end]);
                    self.advance(end + 3);
                } else if rem.starts_with("<!DOCTYPE") || rem.starts_with("<!doctype") {
                    // Skip DOCTYPE, handling optional internal subset `[...]` and
                    // rejecting any `<!ENTITY` declaration.
                    let mut i = 9usize;
                    let bytes = rem.as_bytes();
                    let mut in_bracket = false;
                    let mut in_quote: Option<u8> = None;
                    let mut end_pos = None;
                    while i < bytes.len() {
                        let b = bytes[i];
                        if let Some(q) = in_quote {
                            if b == q {
                                in_quote = None;
                            }
                            i += 1;
                            continue;
                        }
                        if b == b'"' || b == b'\'' {
                            in_quote = Some(b);
                            i += 1;
                            continue;
                        }
                        if b == b'[' {
                            in_bracket = true;
                            i += 1;
                            continue;
                        }
                        if b == b']' && in_bracket {
                            in_bracket = false;
                            i += 1;
                            continue;
                        }
                        if in_bracket && rem[i..].starts_with("<!ENTITY") {
                            return Err(raise_invalid_file(
                                interp,
                                "XML entity declarations are not supported in plist files",
                            ));
                        }
                        if b == b'>' && !in_bracket {
                            end_pos = Some(i + 1);
                            break;
                        }
                        i += 1;
                    }
                    let Some(ep) = end_pos else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    self.advance(ep);
                } else if rem.starts_with("<!ENTITY") {
                    return Err(raise_invalid_file(
                        interp,
                        "XML entity declarations are not supported in plist files",
                    ));
                } else if rem.starts_with("</") {
                    let Some(gt) = rem.find('>') else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    let tag = rem[2..gt].trim().to_string();
                    let Some(open_tag) = self.tag_stack.pop() else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    if open_tag != tag {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    }
                    self.advance(gt + 1);
                    self.handle_end(interp, &tag)?;
                } else {
                    // Opening or self-closing tag
                    let bytes = rem.as_bytes();
                    let mut i = 1usize;
                    let mut in_quote: Option<u8> = None;
                    let mut gt = None;
                    while i < bytes.len() {
                        let b = bytes[i];
                        if let Some(q) = in_quote {
                            if b == q {
                                in_quote = None;
                            }
                        } else if b == b'"' || b == b'\'' {
                            in_quote = Some(b);
                        } else if b == b'>' {
                            gt = Some(i);
                            break;
                        }
                        i += 1;
                    }
                    let Some(gt_pos) = gt else {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    };
                    let inside = rem[1..gt_pos].trim();
                    let (inside, self_closing) = if let Some(stripped) = inside.strip_suffix('/') {
                        (stripped.trim(), true)
                    } else {
                        (inside, false)
                    };
                    let tag = inside
                        .split(|c: char| c.is_ascii_whitespace())
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if tag.is_empty() {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    }
                    saw_root_element = true;
                    self.advance(gt_pos + 1);
                    self.handle_begin(interp, &tag)?;
                    if self_closing {
                        self.handle_end(interp, &tag)?;
                    } else {
                        self.tag_stack.push(tag);
                    }
                }
            } else {
                if !rem.trim().is_empty() {
                    return Err(raise_invalid_file(interp, "Invalid file"));
                }
                break;
            }
        }
        if !saw_root_element || !self.tag_stack.is_empty() {
            return Err(raise_invalid_file(interp, "Invalid file"));
        }
        Ok(self.root.take().unwrap_or_else(Value::none))
    }
}

fn parse_int_radix(digits: &str, radix: u32, negative: bool) -> Result<Value> {
    if digits.is_empty() {
        return Err(PyError::named("ValueError", "invalid literal for int()"));
    }
    let bi = num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)
        .ok_or_else(|| PyError::named("ValueError", "invalid literal for int()"))?;
    let bi = if negative { -bi } else { bi };
    if let Ok(i) = i64::try_from(&bi) {
        Ok(Value::int(i))
    } else {
        Ok(Value::bigint(bi))
    }
}

fn parse_int_decimal(s: &str) -> Result<Value> {
    if let Ok(i) = s.parse::<i64>() {
        return Ok(Value::int(i));
    }
    let bi = num_bigint::BigInt::parse_bytes(s.as_bytes(), 10)
        .ok_or_else(|| PyError::named("ValueError", format!("invalid literal for int(): {s:?}")))?;
    Ok(Value::bigint(bi))
}

fn parse_xml_plist(interp: &mut Interpreter, raw: &[u8], dict_type: Value) -> Result<Value> {
    let text = decode_xml_bytes(raw)?;
    let mut parser = XmlPlistParser::new(&text, dict_type);
    parser.parse(interp)
}

// ── XML Plist Writer ─────────────────────────────────────────────────────────

fn xml_escape(text: &str) -> Result<String> {
    for b in text.bytes() {
        if b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r' {
            return Err(PyError::named(
                "ValueError",
                "strings can't contain control characters; use bytes instead",
            ));
        }
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    Ok(out)
}

struct XmlPlistWriter {
    out: Vec<u8>,
    indent_level: usize,
    sort_keys: bool,
    skipkeys: bool,
}

impl XmlPlistWriter {
    fn new(sort_keys: bool, skipkeys: bool) -> Self {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(PLIST_XML_HEADER);
        Self {
            out,
            indent_level: 0,
            sort_keys,
            skipkeys,
        }
    }

    fn writeln_bytes(&mut self, line: &[u8]) {
        if !line.is_empty() {
            for _ in 0..self.indent_level {
                self.out.push(b'\t');
            }
            self.out.extend_from_slice(line);
        }
        self.out.push(b'\n');
    }

    fn writeln(&mut self, line: &str) {
        self.writeln_bytes(line.as_bytes());
    }

    fn begin_element(&mut self, tag: &str) {
        self.writeln(&format!("<{tag}>"));
        self.indent_level += 1;
    }

    fn end_element(&mut self, tag: &str) {
        self.indent_level -= 1;
        self.writeln(&format!("</{tag}>"));
    }

    fn simple_element(&mut self, tag: &str, value: Option<&str>) -> Result<()> {
        if let Some(val) = value {
            let esc = xml_escape(val)?;
            self.writeln(&format!("<{tag}>{esc}</{tag}>"));
        } else {
            self.writeln(&format!("<{tag}/>"));
        }
        Ok(())
    }

    fn write_root(mut self, interp: &mut Interpreter, value: &Value) -> Result<Vec<u8>> {
        self.writeln("<plist version=\"1.0\">");
        self.write_value(interp, value)?;
        self.writeln("</plist>");
        Ok(self.out)
    }

    fn write_value(&mut self, interp: &mut Interpreter, value: &Value) -> Result<()> {
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(value) {
            self.write_bytes(&snap);
            return Ok(());
        }
        match value.kind() {
            ValueKind::Str(s) => self.simple_element("string", Some(s)),
            ValueKind::Bool(true) => self.simple_element("true", None),
            ValueKind::Bool(false) => self.simple_element("false", None),
            ValueKind::Int(n) => self.simple_element("integer", Some(&n.to_string())),
            ValueKind::BigInt(bi) => {
                let min_i64 = num_bigint::BigInt::from(i64::MIN);
                let max_u64_excl = num_bigint::BigInt::from(1u128 << 64);
                if bi >= &min_i64 && bi < &max_u64_excl {
                    self.simple_element("integer", Some(&bi.to_string()))
                } else {
                    Err(PyError::named("OverflowError", bi.to_string()))
                }
            }
            ValueKind::Float(_) => {
                let repr = render_value_repr(interp, value)?;
                self.simple_element("real", Some(&repr))
            }
            ValueKind::Bytes(b) => {
                self.write_bytes(b.as_slice());
                Ok(())
            }
            ValueKind::List(items) => {
                let cloned = items.to_vec();
                self.write_array(interp, &cloned)
            }
            ValueKind::Tuple(items) => self.write_array(interp, items),
            ValueKind::Dict(_) => self.write_dict(interp, value),
            ValueKind::PyInstance(_) => {
                if is_datetime_instance(interp, value)? {
                    let s = format_datetime_iso(interp, value)?;
                    self.simple_element("date", Some(&s))
                } else if is_dict_subclass(interp, value)? {
                    self.write_dict(interp, value)
                } else if is_str_subclass(interp, value)? {
                    let s = extract_str_value(interp, value)?;
                    self.simple_element("string", Some(&s))
                } else if is_int_subclass(interp, value)? {
                    let n = extract_int_i128(interp, value)?;
                    self.simple_element("integer", Some(&n.to_string()))
                } else if is_list_or_tuple_subclass(interp, value)? {
                    let items = interp.collect_iterable(value)?;
                    self.write_array(interp, &items)
                } else {
                    let ty = value_type_repr(interp, value);
                    Err(PyError::named(
                        "TypeError",
                        format!("unsupported type: {ty}"),
                    ))
                }
            }
            _ => {
                let ty = value_type_repr(interp, value);
                Err(PyError::named(
                    "TypeError",
                    format!("unsupported type: {ty}"),
                ))
            }
        }
    }

    fn write_bytes(&mut self, data: &[u8]) {
        self.begin_element("data");
        self.indent_level -= 1;
        // CPython: maxlinelength = max(16, 76 - len((b"\t" * self._indent_level).expandtabs()))
        let expanded_indent = self.indent_level * 8;
        let max_line_length = 76usize.saturating_sub(expanded_indent).max(16);
        for line in encode_base64_lines(data, max_line_length) {
            if !line.is_empty() {
                self.writeln_bytes(&line);
            }
        }
        self.indent_level += 1;
        self.end_element("data");
    }

    fn write_dict(&mut self, interp: &mut Interpreter, dict_val: &Value) -> Result<()> {
        let items = collect_dict_items(interp, dict_val, self.sort_keys, self.skipkeys)?;
        if items.is_empty() {
            // Check whether the original dict was empty (if non-empty but all skipped, CPython still writes <dict>\n</dict>)
            let orig_len = call_pos(
                interp,
                Value::builtin_function("len"),
                std::slice::from_ref(dict_val),
            )?
            .as_int()
            .unwrap_or(0);
            if orig_len == 0 {
                return self.simple_element("dict", None);
            }
            self.begin_element("dict");
            self.end_element("dict");
            return Ok(());
        }
        self.begin_element("dict");
        for (key, val) in items {
            self.simple_element("key", Some(&key))?;
            self.write_value(interp, &val)?;
        }
        self.end_element("dict");
        Ok(())
    }

    fn write_array(&mut self, interp: &mut Interpreter, items: &[Value]) -> Result<()> {
        if items.is_empty() {
            return self.simple_element("array", None);
        }
        self.begin_element("array");
        for item in items {
            self.write_value(interp, item)?;
        }
        self.end_element("array");
        Ok(())
    }
}

fn dump_xml_plist(
    interp: &mut Interpreter,
    value: &Value,
    sort_keys: bool,
    skipkeys: bool,
) -> Result<Vec<u8>> {
    XmlPlistWriter::new(sort_keys, skipkeys).write_root(interp, value)
}

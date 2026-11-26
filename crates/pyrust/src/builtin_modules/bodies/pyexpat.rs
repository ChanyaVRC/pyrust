// `pyexpat` module — 100% pure-Rust Python 3.12 streaming XML/Expat parser
// (issue #3260). Zero C `libexpat` dependency.
//
// Implements `ExpatError` (`error`), `ParserCreate`, `ErrorString`,
// `XMLParserType` (`xmlparser`), `EXPAT_VERSION`, `version_info`,
// `native_encoding`, `features`, `XML_PARAM_ENTITY_PARSING_*`, and the
// `errors` and `model` submodules.
//
// Reference: <https://docs.python.org/3/library/pyexpat.html>

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

// Expat error codes (matching `pyexpat_errors::ERROR_TABLE`)
const ERR_SYNTAX: i64 = 2;
const ERR_NO_ELEMENTS: i64 = 3;
const ERR_INVALID_TOKEN: i64 = 4;
const ERR_UNCLOSED_TOKEN: i64 = 5;
const ERR_PARTIAL_CHAR: i64 = 6;
const ERR_TAG_MISMATCH: i64 = 7;
const ERR_DUPLICATE_ATTRIBUTE: i64 = 8;
const ERR_JUNK_AFTER_DOC_ELEMENT: i64 = 9;
const ERR_UNDEFINED_ENTITY: i64 = 11;
const ERR_RECURSIVE_ENTITY_REF: i64 = 12;
const ERR_BAD_CHAR_REF: i64 = 14;
const ERR_MISPLACED_XML_PI: i64 = 17;
const ERR_UNKNOWN_ENCODING: i64 = 18;
const ERR_UNCLOSED_CDATA_SECTION: i64 = 20;
const ERR_UNBOUND_PREFIX: i64 = 27;
const ERR_UNDECLARING_PREFIX: i64 = 28;
const ERR_XML_DECL: i64 = 30;
const ERR_FINISHED: i64 = 36;
const ERR_RESERVED_PREFIX_XML: i64 = 38;
const ERR_RESERVED_PREFIX_XMLNS: i64 = 39;
const ERR_RESERVED_NAMESPACE_URI: i64 = 40;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let f = interp.get_attr(obj, name)?;
    call_pos(interp, f, args)
}

fn make_features_list() -> Value {
    let pairs: &[(&str, i64)] = &[
        ("sizeof(XML_Char)", 1),
        ("sizeof(XML_LChar)", 1),
        ("XML_DTD", 0),
        ("XML_CONTEXT_BYTES", 1024),
        ("XML_NS", 0),
        ("XML_BLAP_MAX_AMP", 100),
        ("XML_BLAP_ACT_THRES", 8_388_608),
        ("XML_GE", 0),
    ];
    Value::list(
        pairs
            .iter()
            .map(|&(k, v)| Value::tuple(vec![Value::string(k), Value::int(v)]))
            .collect(),
    )
}

fn make_expat_error(interp: &mut Interpreter, code: i64, lineno: usize, offset: usize) -> PyError {
    let base_msg = super::pyexpat_errors::error_string(code).unwrap_or("unknown error");
    let full_msg = format!("{base_msg}: line {lineno}, column {offset}");
    if let Ok(m) = interp.load_module("pyexpat")
        && let Ok(cls) = interp.get_attr(&m, "ExpatError")
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(&full_msg)])
    {
        let _ = interp.assign_attr(inst.clone(), "code", Value::int(code));
        let _ = interp.assign_attr(inst.clone(), "lineno", Value::int(lineno as i64));
        let _ = interp.assign_attr(inst.clone(), "offset", Value::int(offset as i64));
        return PyError::raised(inst);
    }
    PyError::named("ExpatError", full_msg)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let (exc_cls, parser_cls) = {
        let m = module.borrow();
        (
            m.get_attr_value("ExpatError"),
            m.get_attr_value("xmlparser"),
        )
    };
    let errors_mod = interp.load_module("pyexpat.errors")?;
    let model_mod = interp.load_module("pyexpat.model")?;
    {
        let mut m = module.borrow_mut();
        if let Some(exc) = exc_cls {
            m.insert_attr("error".to_string(), exc);
        }
        if let Some(pcls) = parser_cls {
            m.insert_attr("XMLParserType".to_string(), pcls);
        }
        m.insert_attr("errors".to_string(), errors_mod);
        m.insert_attr("model".to_string(), model_mod);
    }
    Ok(())
}

// ── Pure-Rust Streaming XML/Expat Engine ─────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseStage {
    PrologStart,
    Prolog,
    InRoot,
    Epilog,
    Finished,
}

#[derive(Clone)]
struct OpenElement {
    raw_name: String,
    expanded_name: String,
    declared_prefixes: Vec<Option<String>>,
    saved_bindings: Vec<(Option<String>, Option<String>)>,
}

#[derive(Clone)]
struct EntityDeclInfo {
    value: Option<String>,
}

#[derive(Clone, Default)]
struct HandlerTable {
    start_element: Option<Value>,
    end_element: Option<Value>,
    character_data: Option<Value>,
    processing_instruction: Option<Value>,
    comment: Option<Value>,
    start_cdata_section: Option<Value>,
    end_cdata_section: Option<Value>,
    xml_decl: Option<Value>,
    start_doctype_decl: Option<Value>,
    end_doctype_decl: Option<Value>,
    start_namespace_decl: Option<Value>,
    end_namespace_decl: Option<Value>,
    default: Option<Value>,
    default_expand: Option<Value>,
    entity_decl: Option<Value>,
    element_decl: Option<Value>,
    attlist_decl: Option<Value>,
    notation_decl: Option<Value>,
    unparsed_entity_decl: Option<Value>,
    skipped_entity: Option<Value>,
    external_entity_ref: Option<Value>,
    not_standalone: Option<Value>,
}

#[derive(Clone)]
struct ExpatParserState {
    encoding: Option<String>,
    detected_encoding: String,
    bom_checked: bool,
    namespace_separator: Option<String>,
    intern: Option<Value>,
    base: Option<String>,
    param_entity_parsing: i64,
    use_foreign_dtd: bool,
    has_external_dtd: bool,
    reparse_deferral_enabled: bool,

    // User-configurable flags
    buffer_text: bool,
    buffer_size: usize,
    namespace_prefixes: bool,
    ordered_attributes: bool,
    specified_attributes: bool,

    // Buffers & streaming state
    raw_pending: Vec<u8>,
    text: String,
    pos: usize,
    chardata_buf: String,
    in_callback: bool,
    last_context: Vec<u8>,
    stage: ParseStage,
    started: bool,

    // Position tracking (current cursor & active event / error)
    cursor_line: usize,
    cursor_col: usize,
    cursor_byte: usize,
    current_line: usize,
    current_col: usize,
    current_byte: i64,
    error_code: i64,
    error_line: usize,
    error_col: usize,
    error_byte: i64,

    // XML tree / namespaces / DTD state
    stack: Vec<OpenElement>,
    ns_bindings: HashMap<Option<String>, String>,
    entities: HashMap<String, EntityDeclInfo>,
    default_attrs: HashMap<String, Vec<(String, String)>>,
    pending_events: Vec<ParserEvent>,

    // Callbacks
    handlers: HandlerTable,
}

impl ExpatParserState {
    fn new(
        encoding: Option<String>,
        namespace_separator: Option<String>,
        intern: Option<Value>,
    ) -> Self {
        let detected = encoding
            .clone()
            .unwrap_or_else(|| "utf-8".to_string())
            .to_ascii_lowercase();
        let mut ns_bindings = HashMap::new();
        ns_bindings.insert(Some("xml".to_string()), XML_NS.to_string());
        Self {
            encoding,
            detected_encoding: detected,
            bom_checked: false,
            namespace_separator,
            intern,
            base: None,
            param_entity_parsing: 0,
            use_foreign_dtd: false,
            has_external_dtd: false,
            reparse_deferral_enabled: true,
            buffer_text: false,
            buffer_size: 8192,
            namespace_prefixes: false,
            ordered_attributes: false,
            specified_attributes: false,
            raw_pending: Vec::new(),
            text: String::new(),
            pos: 0,
            chardata_buf: String::new(),
            in_callback: false,
            last_context: Vec::new(),
            stage: ParseStage::PrologStart,
            started: false,
            cursor_line: 1,
            cursor_col: 0,
            cursor_byte: 0,
            current_line: 1,
            current_col: 0,
            current_byte: -1,
            error_code: 0,
            error_line: 1,
            error_col: 0,
            error_byte: -1,
            stack: Vec::new(),
            ns_bindings,
            entities: HashMap::new(),
            default_attrs: HashMap::new(),
            pending_events: Vec::new(),
            handlers: HandlerTable::default(),
        }
    }

    fn set_error(&mut self, code: i64, line: usize, col: usize, byte: usize) -> ParseStep {
        self.error_code = code;
        self.error_line = line;
        self.error_col = col;
        self.error_byte = byte as i64;
        self.current_line = line;
        self.current_col = col;
        self.current_byte = byte as i64;
        ParseStep::Error { code, line, col }
    }

    fn advance_pos(&mut self, s: &str) {
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                self.cursor_line += 1;
                self.cursor_col = 0;
            } else if c == '\n' {
                self.cursor_line += 1;
                self.cursor_col = 0;
            } else {
                self.cursor_col += 1;
            }
        }
        self.cursor_byte += s.len();
    }

    fn record_event_pos(&mut self, line: usize, col: usize, byte: usize) {
        self.current_line = line;
        self.current_col = col;
        self.current_byte = byte as i64;
    }

    fn compact_if_needed(&mut self) {
        if self.pos > 0 && (self.pos >= self.text.len() || self.pos > 4096) {
            self.text.drain(..self.pos);
            self.pos = 0;
        }
    }
}

#[derive(Clone)]
enum ParserEvent {
    XmlDecl {
        version: String,
        encoding: Option<String>,
        standalone: i64,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    StartDoctype {
        name: String,
        sysid: Option<String>,
        pubid: Option<String>,
        has_internal_subset: i64,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    EndDoctype {
        line: usize,
        col: usize,
        byte: usize,
    },
    EntityDecl {
        name: String,
        is_param: i64,
        value: Option<String>,
        base: Option<String>,
        sysid: Option<String>,
        pubid: Option<String>,
        notation: Option<String>,
        line: usize,
        col: usize,
        byte: usize,
    },
    UnparsedEntityDecl {
        name: String,
        base: Option<String>,
        sysid: Option<String>,
        pubid: Option<String>,
        notation: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    ElementDecl {
        name: String,
        model: Value,
        line: usize,
        col: usize,
        byte: usize,
    },
    AttlistDecl {
        elname: String,
        attname: String,
        att_type: String,
        default: Option<String>,
        required: i64,
        line: usize,
        col: usize,
        byte: usize,
    },
    NotationDecl {
        name: String,
        base: Option<String>,
        sysid: Option<String>,
        pubid: Option<String>,
        line: usize,
        col: usize,
        byte: usize,
    },
    StartNamespace {
        prefix: Option<String>,
        uri: Option<String>,
        line: usize,
        col: usize,
        byte: usize,
    },
    EndNamespace {
        prefix: Option<String>,
        line: usize,
        col: usize,
        byte: usize,
    },
    StartElement {
        name: String,
        attrs: Vec<(String, String)>,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    EndElement {
        name: String,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    CharData {
        data: String,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    Comment {
        data: String,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    ProcessingInstruction {
        target: String,
        data: String,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    StartCdata {
        line: usize,
        col: usize,
        byte: usize,
    },
    EndCdata {
        line: usize,
        col: usize,
        byte: usize,
    },
    SkippedEntity {
        name: String,
        is_param: i64,
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
    DefaultOnly {
        raw: String,
        line: usize,
        col: usize,
        byte: usize,
    },
}

enum ParseStep {
    Emit(ParserEvent),
    NeedMore,
    Done,
    Error { code: i64, line: usize, col: usize },
}

fn is_xml_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_xml_char(cp: u32) -> bool {
    matches!(
        cp,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
}

fn is_name_start_char(c: char) -> bool {
    c == ':'
        || c == '_'
        || c.is_ascii_alphabetic()
        || matches!(
            c as u32,
            0xC0..=0xD6
                | 0xD8..=0xF6
                | 0xF8..=0x2FF
                | 0x370..=0x37D
                | 0x37F..=0x1FFF
                | 0x200C..=0x200D
                | 0x2070..=0x218F
                | 0x2C00..=0x2FEF
                | 0x3001..=0xD7FF
                | 0xF900..=0xFDCF
                | 0xFDF0..=0xFFFD
                | 0x10000..=0xEFFFF
        )
}

fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || c == '-'
        || c == '.'
        || c.is_ascii_digit()
        || matches!(c as u32, 0xB7 | 0x0300..=0x036F | 0x203F..=0x2040)
}

fn normalize_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Decode raw bytes into UTF-8 according to the parser's active encoding.
fn decode_pending_bytes(
    interp: &mut Interpreter,
    st: &mut ExpatParserState,
    isfinal: bool,
) -> std::result::Result<(), (i64, usize, usize)> {
    if st.raw_pending.is_empty() {
        return Ok(());
    }

    if !st.bom_checked {
        if st.raw_pending.len() < 4 && !isfinal {
            // Wait if it could be a BOM prefix
            let b = &st.raw_pending;
            if (b.starts_with(&[0xEF]) && b.len() < 3)
                || (b.starts_with(&[0xFF, 0xFE]) && b.len() < 4)
                || (b.starts_with(&[0x00, 0x00]) && b.len() < 4)
                || (b.len() == 1 && matches!(b[0], 0xFE | 0xFF | 0x00))
            {
                return Ok(());
            }
        }
        st.bom_checked = true;
        if st.raw_pending.starts_with(&[0xEF, 0xBB, 0xBF]) {
            st.raw_pending.drain(..3);
            if st.encoding.is_none() {
                st.detected_encoding = "utf-8".to_string();
            }
        } else if st.raw_pending.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
            st.raw_pending.drain(..4);
            if st.encoding.is_none() {
                st.detected_encoding = "utf-32-be".to_string();
            }
        } else if st.raw_pending.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
            st.raw_pending.drain(..4);
            if st.encoding.is_none() {
                st.detected_encoding = "utf-32-le".to_string();
            }
        } else if st.raw_pending.starts_with(&[0xFE, 0xFF]) {
            st.raw_pending.drain(..2);
            if st.encoding.is_none() {
                st.detected_encoding = "utf-16-be".to_string();
            }
        } else if st.raw_pending.starts_with(&[0xFF, 0xFE]) {
            st.raw_pending.drain(..2);
            if st.encoding.is_none() {
                st.detected_encoding = "utf-16-le".to_string();
            }
        }
    }

    // Before decoding past the XML declaration, peek ASCII `<?xml ... encoding="..." ?>`
    if st.stage == ParseStage::PrologStart
        && st.encoding.is_none()
        && st.text.is_empty()
        && st.detected_encoding == "utf-8"
        && st.raw_pending.starts_with(b"<?xml")
    {
        if let Some(end_decl) = st.raw_pending.windows(2).position(|w| w == b"?>") {
            let decl_ascii = String::from_utf8_lossy(&st.raw_pending[..end_decl + 2]);
            if let Some(enc) = extract_xml_decl_encoding(&decl_ascii) {
                st.detected_encoding = enc.to_ascii_lowercase();
            }
        } else if !isfinal && st.raw_pending.len() < 256 {
            return Ok(());
        }
    }

    let enc_norm = st.detected_encoding.to_ascii_lowercase().replace('_', "-");
    match enc_norm.as_str() {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => {
            let valid_len = match std::str::from_utf8(&st.raw_pending) {
                Ok(s) => {
                    if (enc_norm == "us-ascii" || enc_norm == "ascii") && !s.is_ascii() {
                        return Err((ERR_INVALID_TOKEN, st.cursor_line, st.cursor_col));
                    }
                    st.text.push_str(s);
                    st.raw_pending.len()
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    if valid > 0 {
                        let s = std::str::from_utf8(&st.raw_pending[..valid]).unwrap_or("");
                        st.text.push_str(s);
                    }
                    if e.error_len().is_none() && !isfinal {
                        valid
                    } else {
                        let code = if e.error_len().is_none() {
                            ERR_PARTIAL_CHAR
                        } else {
                            ERR_INVALID_TOKEN
                        };
                        return Err((code, st.cursor_line, st.cursor_col));
                    }
                }
            };
            st.raw_pending.drain(..valid_len);
        }
        "iso-8859-1" | "latin-1" | "latin1" => {
            let decoded: String = st.raw_pending.drain(..).map(|b| b as char).collect();
            st.text.push_str(&decoded);
        }
        "utf-16" | "utf-16-le" | "utf16le" | "utf-16-be" | "utf16be" => {
            let is_be = enc_norm.ends_with("be");
            let usable = (st.raw_pending.len() / 2) * 2;
            if usable == 0 {
                if isfinal && !st.raw_pending.is_empty() {
                    return Err((ERR_PARTIAL_CHAR, st.cursor_line, st.cursor_col));
                }
                return Ok(());
            }
            let units: Vec<u16> = st.raw_pending[..usable]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| {
                    if is_be {
                        u16::from_be_bytes([c[0], c[1]])
                    } else {
                        u16::from_le_bytes([c[0], c[1]])
                    }
                })
                .collect();
            // Avoid splitting a surrogate pair across chunks
            let take_units = if !isfinal
                && units
                    .last()
                    .is_some_and(|&u| (0xD800..=0xDBFF).contains(&u))
            {
                units.len() - 1
            } else {
                units.len()
            };
            let decoded = String::from_utf16(&units[..take_units])
                .map_err(|_| (ERR_INVALID_TOKEN, st.cursor_line, st.cursor_col))?;
            st.text.push_str(&decoded);
            st.raw_pending.drain(..take_units * 2);
            if isfinal && !st.raw_pending.is_empty() {
                return Err((ERR_PARTIAL_CHAR, st.cursor_line, st.cursor_col));
            }
        }
        "utf-32" | "utf-32-le" | "utf32le" | "utf-32-be" | "utf32be" => {
            let is_be = enc_norm.ends_with("be");
            let usable = (st.raw_pending.len() / 4) * 4;
            for chunk in st.raw_pending[..usable].as_chunks::<4>().0 {
                let code = if is_be {
                    u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                } else {
                    u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
                };
                let ch = char::from_u32(code).ok_or((
                    ERR_INVALID_TOKEN,
                    st.cursor_line,
                    st.cursor_col,
                ))?;
                st.text.push(ch);
            }
            st.raw_pending.drain(..usable);
            if isfinal && !st.raw_pending.is_empty() {
                return Err((ERR_PARTIAL_CHAR, st.cursor_line, st.cursor_col));
            }
        }
        _ => {
            // Fall back to Python's codecs module for arbitrary encodings (cp1252, shift_jis, etc.)
            let raw = std::mem::take(&mut st.raw_pending);
            let codecs_mod = interp
                .load_module("codecs")
                .map_err(|_| (ERR_UNKNOWN_ENCODING, st.cursor_line, st.cursor_col))?;
            let decode_fn = interp
                .get_attr(&codecs_mod, "decode")
                .map_err(|_| (ERR_UNKNOWN_ENCODING, st.cursor_line, st.cursor_col))?;
            let decoded_val = call_pos(
                interp,
                decode_fn,
                &[Value::bytes(raw), Value::string(&st.detected_encoding)],
            )
            .map_err(|_| (ERR_UNKNOWN_ENCODING, st.cursor_line, st.cursor_col))?;
            let Some(s) = decoded_val.as_str() else {
                return Err((ERR_UNKNOWN_ENCODING, st.cursor_line, st.cursor_col));
            };
            st.text.push_str(s);
        }
    }
    Ok(())
}

fn extract_xml_decl_encoding(decl: &str) -> Option<String> {
    let idx = decl.find("encoding")?;
    let rest = decl[idx + 8..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let after = &rest[1..];
    let end = after.find(quote)?;
    Some(after[..end].to_string())
}

fn expand_char_ref(ref_body: &str) -> std::result::Result<char, i64> {
    let cp = if let Some(hex) = ref_body.strip_prefix("#x") {
        if hex.is_empty() {
            return Err(ERR_SYNTAX);
        }
        u32::from_str_radix(hex, 16).map_err(|_| ERR_BAD_CHAR_REF)?
    } else if let Some(dec) = ref_body.strip_prefix('#') {
        if dec.is_empty() {
            return Err(ERR_SYNTAX);
        }
        dec.parse::<u32>().map_err(|_| ERR_BAD_CHAR_REF)?
    } else {
        return Err(ERR_SYNTAX);
    };
    if !is_xml_char(cp) {
        return Err(ERR_BAD_CHAR_REF);
    }
    char::from_u32(cp).ok_or(ERR_BAD_CHAR_REF)
}

fn expand_entity_value(
    entities: &HashMap<String, EntityDeclInfo>,
    raw: &str,
    in_attr: bool,
    seen: &mut HashSet<String>,
) -> std::result::Result<(String, Vec<String>), i64> {
    let mut out = String::with_capacity(raw.len());
    let mut skipped = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = raw[i..].chars().next().unwrap();
        if in_attr && ch == '<' {
            return Err(ERR_INVALID_TOKEN);
        }
        if ch == '&' {
            let rest = &raw[i + 1..];
            let Some(semi) = rest.find(';') else {
                return Err(ERR_INVALID_TOKEN);
            };
            let name = &rest[..semi];
            if name.is_empty() {
                return Err(ERR_INVALID_TOKEN);
            }
            if name.starts_with('#') {
                let c = expand_char_ref(name)?;
                out.push(c);
            } else {
                match name {
                    "lt" => out.push('<'),
                    "gt" => out.push('>'),
                    "amp" => out.push('&'),
                    "apos" => out.push('\''),
                    "quot" => out.push('"'),
                    _ => {
                        if !is_valid_xml_name(name) {
                            return Err(ERR_INVALID_TOKEN);
                        }
                        if seen.contains(name) {
                            return Err(ERR_RECURSIVE_ENTITY_REF);
                        }
                        if let Some(info) = entities.get(name) {
                            if let Some(val) = &info.value {
                                seen.insert(name.to_string());
                                let (expanded, sub_skip) =
                                    expand_entity_value(entities, val, in_attr, seen)?;
                                seen.remove(name);
                                out.push_str(&expanded);
                                skipped.extend(sub_skip);
                            } else {
                                skipped.push(name.to_string());
                            }
                        } else {
                            skipped.push(name.to_string());
                        }
                    }
                }
            }
            i += 1 + semi + 1;
        } else {
            if in_attr && matches!(ch, '\r' | '\n' | '\t') {
                if ch == '\r' && raw[i + 1..].starts_with('\n') {
                    i += 1;
                }
                out.push(' ');
            } else {
                out.push(ch);
            }
            i += ch.len_utf8();
        }
    }
    Ok((out, skipped))
}

fn is_valid_xml_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_name_start_char(first) && chars.all(is_name_char)
}

fn parse_xml_decl(body: &str) -> std::result::Result<(String, Option<String>, i64), i64> {
    // body is the interior between "<?xml" and "?>"
    let mut rest = body;
    if !rest.starts_with(is_xml_ws) {
        return Err(ERR_XML_DECL);
    }
    rest = rest.trim_start_matches(is_xml_ws);
    let mut version: Option<String> = None;
    let mut encoding: Option<String> = None;
    let mut standalone: i64 = -1;
    let mut stage = 0u8; // 0=version, 1=encoding, 2=standalone, 3=done

    while !rest.is_empty() {
        let eq = rest.find('=').ok_or(ERR_XML_DECL)?;
        let key = rest[..eq].trim();
        let after_eq = rest[eq + 1..].trim_start_matches(is_xml_ws);
        let quote = after_eq.chars().next().ok_or(ERR_XML_DECL)?;
        if quote != '"' && quote != '\'' {
            return Err(ERR_XML_DECL);
        }
        let after_q = &after_eq[1..];
        let end_q = after_q.find(quote).ok_or(ERR_XML_DECL)?;
        let val = &after_q[..end_q];
        rest = &after_q[end_q + 1..];
        if !rest.is_empty() && !rest.starts_with(is_xml_ws) {
            return Err(ERR_XML_DECL);
        }
        rest = rest.trim_start_matches(is_xml_ws);

        match key {
            "version" if stage == 0 => {
                if !val.starts_with("1.") || val.len() < 3 {
                    return Err(ERR_XML_DECL);
                }
                version = Some(val.to_string());
                stage = 1;
            }
            "encoding" if stage == 1 => {
                if val.is_empty() {
                    return Err(ERR_XML_DECL);
                }
                encoding = Some(val.to_string());
                stage = 2;
            }
            "standalone" if stage == 1 || stage == 2 => {
                standalone = match val {
                    "yes" => 1,
                    "no" => 0,
                    _ => return Err(ERR_XML_DECL),
                };
                stage = 3;
            }
            _ => return Err(ERR_XML_DECL),
        }
    }
    let ver = version.ok_or(ERR_XML_DECL)?;
    Ok((ver, encoding, standalone))
}

fn find_tag_end(s: &str) -> Option<usize> {
    let mut in_quote: Option<char> = None;
    for (idx, ch) in s.char_indices() {
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch == '>' {
            return Some(idx);
        }
    }
    None
}

fn find_doctype_end(s: &str) -> Option<usize> {
    let mut in_quote: Option<char> = None;
    let mut bracket_depth: usize = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if in_quote.is_none() && bracket_depth > 0 && s[i..].starts_with("<!--") {
            let end_c = s[i + 4..].find("-->")?;
            i += 4 + end_c + 3;
            continue;
        }
        if in_quote.is_none() && bracket_depth > 0 && s[i..].starts_with("<?") {
            let end_p = s[i + 2..].find("?>")?;
            i += 2 + end_p + 2;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        if let Some(q) = in_quote {
            if ch == q {
                in_quote = None;
            }
        } else {
            match ch {
                '"' | '\'' => in_quote = Some(ch),
                '[' => bracket_depth += 1,
                ']' => bracket_depth = bracket_depth.saturating_sub(1),
                '>' if bracket_depth == 0 => return Some(i),
                _ => {}
            }
        }
        i += ch.len_utf8();
    }
    None
}

fn parse_quoted_str(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start_matches(is_xml_ws);
    let q = s.chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let after = &s[1..];
    let end = after.find(q)?;
    Some((after[..end].to_string(), &after[end + 1..]))
}

fn parse_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start_matches(is_xml_ws);
    if s.is_empty() {
        return None;
    }
    let end = s
        .find(|c: char| is_xml_ws(c) || matches!(c, '>' | '<' | '[' | ']' | '=' | '/'))
        .unwrap_or(s.len());
    if end == 0 {
        None
    } else {
        Some((&s[..end], &s[end..]))
    }
}

fn parse_doctype_subset(
    st: &mut ExpatParserState,
    subset: &str,
    line: usize,
    col: usize,
    byte: usize,
) -> std::result::Result<Vec<ParserEvent>, i64> {
    let mut events = Vec::new();
    let mut rest = subset;
    while !rest.is_empty() {
        rest = rest.trim_start_matches(is_xml_ws);
        if rest.is_empty() {
            break;
        }
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after.find("-->").ok_or(ERR_UNCLOSED_TOKEN)?;
            let data = normalize_newlines(&after[..end]);
            events.push(ParserEvent::Comment {
                data,
                raw: rest[..4 + end + 3].to_string(),
                line,
                col,
                byte,
            });
            rest = &after[end + 3..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("<?") {
            let end = after.find("?>").ok_or(ERR_UNCLOSED_TOKEN)?;
            let inner = &after[..end];
            let (target, data) = split_pi(inner)?;
            events.push(ParserEvent::ProcessingInstruction {
                target,
                data,
                raw: rest[..2 + end + 2].to_string(),
                line,
                col,
                byte,
            });
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!ENTITY") {
            let end = find_tag_end(after).ok_or(ERR_UNCLOSED_TOKEN)?;
            let body = after[..end].trim();
            rest = &after[end + 1..];

            let (is_param, body) = if let Some(b) = body.strip_prefix('%') {
                (1i64, b.trim_start_matches(is_xml_ws))
            } else {
                (0i64, body)
            };
            let Some((name, after_name)) = parse_token(body) else {
                return Err(ERR_SYNTAX);
            };
            let after_name = after_name.trim_start_matches(is_xml_ws);
            if after_name.starts_with('"') || after_name.starts_with('\'') {
                let (val, _) = parse_quoted_str(after_name).ok_or(ERR_SYNTAX)?;
                if is_param == 0 {
                    st.entities
                        .entry(name.to_string())
                        .or_insert(EntityDeclInfo {
                            value: Some(val.clone()),
                        });
                }
                events.push(ParserEvent::EntityDecl {
                    name: name.to_string(),
                    is_param,
                    value: Some(val),
                    base: st.base.clone(),
                    sysid: None,
                    pubid: None,
                    notation: None,
                    line,
                    col,
                    byte,
                });
            } else if let Some(kw_rest) = after_name.strip_prefix("SYSTEM") {
                let (sysid, rem) = parse_quoted_str(kw_rest).ok_or(ERR_SYNTAX)?;
                let rem = rem.trim_start_matches(is_xml_ws);
                let notation = if let Some(nd) = rem.strip_prefix("NDATA") {
                    parse_token(nd).map(|(t, _)| t.to_string())
                } else {
                    None
                };
                if is_param == 0 {
                    st.entities
                        .entry(name.to_string())
                        .or_insert(EntityDeclInfo { value: None });
                }
                if let Some(ref not_name) = notation {
                    events.push(ParserEvent::UnparsedEntityDecl {
                        name: name.to_string(),
                        base: st.base.clone(),
                        sysid: Some(sysid.clone()),
                        pubid: None,
                        notation: not_name.clone(),
                        line,
                        col,
                        byte,
                    });
                }
                events.push(ParserEvent::EntityDecl {
                    name: name.to_string(),
                    is_param,
                    value: None,
                    base: st.base.clone(),
                    sysid: Some(sysid),
                    pubid: None,
                    notation,
                    line,
                    col,
                    byte,
                });
            } else if let Some(kw_rest) = after_name.strip_prefix("PUBLIC") {
                let (pubid, rem) = parse_quoted_str(kw_rest).ok_or(ERR_SYNTAX)?;
                let (sysid, rem2) = parse_quoted_str(rem).ok_or(ERR_SYNTAX)?;
                let rem2 = rem2.trim_start_matches(is_xml_ws);
                let notation = if let Some(nd) = rem2.strip_prefix("NDATA") {
                    parse_token(nd).map(|(t, _)| t.to_string())
                } else {
                    None
                };
                if is_param == 0 {
                    st.entities
                        .entry(name.to_string())
                        .or_insert(EntityDeclInfo { value: None });
                }
                if let Some(ref not_name) = notation {
                    events.push(ParserEvent::UnparsedEntityDecl {
                        name: name.to_string(),
                        base: st.base.clone(),
                        sysid: Some(sysid.clone()),
                        pubid: Some(pubid.clone()),
                        notation: not_name.clone(),
                        line,
                        col,
                        byte,
                    });
                }
                events.push(ParserEvent::EntityDecl {
                    name: name.to_string(),
                    is_param,
                    value: None,
                    base: st.base.clone(),
                    sysid: Some(sysid),
                    pubid: Some(pubid),
                    notation,
                    line,
                    col,
                    byte,
                });
            }
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!ELEMENT") {
            let end = find_tag_end(after).ok_or(ERR_UNCLOSED_TOKEN)?;
            let body = after[..end].trim();
            rest = &after[end + 1..];
            if let Some((name, spec)) = parse_token(body) {
                let spec_s = spec.trim();
                let ctype = if spec_s == "EMPTY" {
                    1
                } else if spec_s == "ANY" {
                    2
                } else if spec_s.contains("#PCDATA") {
                    3
                } else {
                    6
                };
                let model = Value::tuple(vec![
                    Value::int(ctype),
                    Value::int(0),
                    Value::none(),
                    Value::tuple(Vec::new()),
                ]);
                events.push(ParserEvent::ElementDecl {
                    name: name.to_string(),
                    model,
                    line,
                    col,
                    byte,
                });
            }
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!ATTLIST") {
            let end = find_tag_end(after).ok_or(ERR_UNCLOSED_TOKEN)?;
            let body = after[..end].trim();
            rest = &after[end + 1..];
            if let Some((elname, mut rem)) = parse_token(body) {
                loop {
                    rem = rem.trim_start_matches(is_xml_ws);
                    if rem.is_empty() {
                        break;
                    }
                    let Some((attname, r1)) = parse_token(rem) else {
                        break;
                    };
                    let r1 = r1.trim_start_matches(is_xml_ws);
                    let (att_type, r2) = if r1.starts_with('(') {
                        let Some(close_p) = r1.find(')') else {
                            break;
                        };
                        (r1[..=close_p].to_string(), &r1[close_p + 1..])
                    } else if let Some((t, r)) = parse_token(r1) {
                        (t.to_string(), r)
                    } else {
                        break;
                    };
                    let r2 = r2.trim_start_matches(is_xml_ws);
                    let (default_val, required, r3) = if let Some(r) = r2.strip_prefix("#REQUIRED")
                    {
                        (None, 1i64, r)
                    } else if let Some(r) = r2.strip_prefix("#IMPLIED") {
                        (None, 0i64, r)
                    } else if let Some(r) = r2.strip_prefix("#FIXED") {
                        let (v, next_r) = parse_quoted_str(r).ok_or(ERR_SYNTAX)?;
                        (Some(v), 1i64, next_r)
                    } else if r2.starts_with('"') || r2.starts_with('\'') {
                        let (v, next_r) = parse_quoted_str(r2).ok_or(ERR_SYNTAX)?;
                        (Some(v), 0i64, next_r)
                    } else {
                        break;
                    };
                    if let Some(ref dv) = default_val {
                        st.default_attrs
                            .entry(elname.to_string())
                            .or_default()
                            .push((attname.to_string(), dv.clone()));
                    }
                    events.push(ParserEvent::AttlistDecl {
                        elname: elname.to_string(),
                        attname: attname.to_string(),
                        att_type,
                        default: default_val,
                        required,
                        line,
                        col,
                        byte,
                    });
                    rem = r3;
                }
            }
            continue;
        }
        if let Some(after) = rest.strip_prefix("<!NOTATION") {
            let end = find_tag_end(after).ok_or(ERR_UNCLOSED_TOKEN)?;
            let body = after[..end].trim();
            rest = &after[end + 1..];
            if let Some((name, after_name)) = parse_token(body) {
                let after_name = after_name.trim_start_matches(is_xml_ws);
                let (pubid, sysid) = if let Some(r) = after_name.strip_prefix("SYSTEM") {
                    let (s, _) = parse_quoted_str(r).ok_or(ERR_SYNTAX)?;
                    (None, Some(s))
                } else if let Some(r) = after_name.strip_prefix("PUBLIC") {
                    let (p, rem) = parse_quoted_str(r).ok_or(ERR_SYNTAX)?;
                    let s = parse_quoted_str(rem).map(|(sv, _)| sv);
                    (Some(p), s)
                } else {
                    (None, None)
                };
                events.push(ParserEvent::NotationDecl {
                    name: name.to_string(),
                    base: st.base.clone(),
                    sysid,
                    pubid,
                    line,
                    col,
                    byte,
                });
            }
            continue;
        }
        if let Some(after) = rest.strip_prefix('%') {
            // Parameter entity reference in internal subset
            let semi = after.find(';').ok_or(ERR_SYNTAX)?;
            rest = &after[semi + 1..];
            continue;
        }
        return Err(ERR_SYNTAX);
    }
    Ok(events)
}

fn split_pi(inner: &str) -> std::result::Result<(String, String), i64> {
    let target_end = inner.find(is_xml_ws).unwrap_or(inner.len());
    let target = &inner[..target_end];
    if !is_valid_xml_name(target) {
        return Err(ERR_INVALID_TOKEN);
    }
    let data = if target_end < inner.len() {
        normalize_newlines(inner[target_end..].trim_start_matches(is_xml_ws))
    } else {
        String::new()
    };
    Ok((target.to_string(), data))
}

fn split_qname(qname: &str) -> std::result::Result<(Option<&str>, &str), i64> {
    match qname.find(':') {
        None => Ok((None, qname)),
        Some(idx) => {
            let pfx = &qname[..idx];
            let local = &qname[idx + 1..];
            if pfx.is_empty() || local.is_empty() || local.contains(':') {
                return Err(ERR_INVALID_TOKEN);
            }
            Ok((Some(pfx), local))
        }
    }
}

fn format_ns_name(
    uri: &str,
    local: &str,
    prefix: Option<&str>,
    sep: &str,
    include_prefix: bool,
) -> String {
    if sep.is_empty() {
        return format!("{uri}{local}");
    }
    if include_prefix && let Some(pfx) = prefix {
        format!("{uri}{sep}{local}{sep}{pfx}")
    } else {
        format!("{uri}{sep}{local}")
    }
}

/// Advance the XML parser by one step. Never invokes Python callbacks directly.
fn next_step(st: &mut ExpatParserState, isfinal: bool) -> ParseStep {
    if !st.pending_events.is_empty() {
        let ev = st.pending_events.remove(0);
        return ParseStep::Emit(ev);
    }

    if st.stage == ParseStage::Finished {
        return ParseStep::Done;
    }

    loop {
        if !st.pending_events.is_empty() {
            let ev = st.pending_events.remove(0);
            return ParseStep::Emit(ev);
        }

        let rem = &st.text[st.pos..];
        if rem.is_empty() {
            if !isfinal {
                return ParseStep::NeedMore;
            }
            // Finalize check
            if st.stage == ParseStage::PrologStart || st.stage == ParseStage::Prolog {
                let (l, c, b) = (st.cursor_line, st.cursor_col, st.cursor_byte);
                return st.set_error(ERR_NO_ELEMENTS, l, c, b);
            }
            if !st.stack.is_empty() {
                let (l, c, b) = (st.cursor_line, st.cursor_col, st.cursor_byte);
                return st.set_error(ERR_NO_ELEMENTS, l, c, b);
            }
            st.stage = ParseStage::Finished;
            return ParseStep::Done;
        }

        let start_line = st.cursor_line;
        let start_col = st.cursor_col;
        let start_byte = st.cursor_byte;

        if rem.starts_with('<') {
            // Need at least 2 bytes to classify `<...`
            if rem.len() < 2 {
                if !isfinal {
                    return ParseStep::NeedMore;
                }
                return st.set_error(ERR_UNCLOSED_TOKEN, start_line, start_col, start_byte);
            }

            // 1. Processing Instruction or XML declaration: `<?...?>`
            if let Some(after_pi) = rem.strip_prefix("<?") {
                let Some(end_rel) = after_pi.find("?>") else {
                    if !isfinal {
                        return ParseStep::NeedMore;
                    }
                    return st.set_error(ERR_UNCLOSED_TOKEN, start_line, start_col, start_byte);
                };
                let raw = rem[..2 + end_rel + 2].to_string();
                let inner = after_pi[..end_rel].to_string();
                st.pos += raw.len();
                st.advance_pos(&raw);

                let target_end = inner.find(is_xml_ws).unwrap_or(inner.len());
                let target = &inner[..target_end];
                if target.eq_ignore_ascii_case("xml") {
                    if st.stage != ParseStage::PrologStart || target != "xml" {
                        return st.set_error(
                            ERR_MISPLACED_XML_PI,
                            start_line,
                            start_col,
                            start_byte,
                        );
                    }
                    let body = &inner[3..];
                    match parse_xml_decl(body) {
                        Ok((version, encoding, standalone)) => {
                            st.stage = ParseStage::Prolog;
                            if st.encoding.is_none()
                                && let Some(ref enc) = encoding
                            {
                                st.detected_encoding = enc.to_ascii_lowercase();
                            }
                            return ParseStep::Emit(ParserEvent::XmlDecl {
                                version,
                                encoding,
                                standalone,
                                raw,
                                line: start_line,
                                col: start_col,
                                byte: start_byte,
                            });
                        }
                        Err(code) => return st.set_error(code, start_line, start_col, start_byte),
                    }
                } else {
                    if st.stage == ParseStage::PrologStart {
                        st.stage = ParseStage::Prolog;
                    }
                    match split_pi(&inner) {
                        Ok((t, d)) => {
                            return ParseStep::Emit(ParserEvent::ProcessingInstruction {
                                target: t,
                                data: d,
                                raw,
                                line: start_line,
                                col: start_col,
                                byte: start_byte,
                            });
                        }
                        Err(code) => return st.set_error(code, start_line, start_col, start_byte),
                    }
                }
            }

            // 2. Comment `<!--...-->`
            if let Some(after_comment) = rem.strip_prefix("<!--") {
                let Some(end_rel) = after_comment.find("-->") else {
                    if !isfinal {
                        return ParseStep::NeedMore;
                    }
                    return st.set_error(ERR_UNCLOSED_TOKEN, start_line, start_col, start_byte);
                };
                let comment_inner = &after_comment[..end_rel];
                if comment_inner.contains("--") || comment_inner.ends_with('-') {
                    return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
                }
                let data = normalize_newlines(comment_inner);
                let raw = rem[..4 + end_rel + 3].to_string();
                st.pos += raw.len();
                st.advance_pos(&raw);
                if st.stage == ParseStage::PrologStart {
                    st.stage = ParseStage::Prolog;
                }
                return ParseStep::Emit(ParserEvent::Comment {
                    data,
                    raw,
                    line: start_line,
                    col: start_col,
                    byte: start_byte,
                });
            }

            // 3. CDATA section `<![CDATA[...]]>`
            if let Some(after_cdata) = rem.strip_prefix("<![CDATA[") {
                if st.stage != ParseStage::InRoot {
                    return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                }
                let Some(end_rel) = after_cdata.find("]]>") else {
                    if !isfinal {
                        return ParseStep::NeedMore;
                    }
                    let end_l = st.cursor_line;
                    let end_c = st.cursor_col + rem.chars().count();
                    let end_b = st.cursor_byte + rem.len();
                    return st.set_error(ERR_UNCLOSED_CDATA_SECTION, end_l, end_c, end_b);
                };
                let cdata_body = normalize_newlines(&after_cdata[..end_rel]);
                let raw = rem[..9 + end_rel + 3].to_string();
                st.pos += raw.len();
                st.advance_pos(&raw);
                st.pending_events.push(ParserEvent::StartCdata {
                    line: start_line,
                    col: start_col,
                    byte: start_byte,
                });
                if !cdata_body.is_empty() {
                    st.pending_events.push(ParserEvent::CharData {
                        data: cdata_body.clone(),
                        raw: cdata_body,
                        line: start_line,
                        col: start_col,
                        byte: start_byte,
                    });
                }
                st.pending_events.push(ParserEvent::EndCdata {
                    line: st.cursor_line,
                    col: st.cursor_col,
                    byte: st.cursor_byte,
                });
                continue;
            }

            // 4. DOCTYPE `<!DOCTYPE ...>`
            if rem.starts_with("<!DOCTYPE") {
                if st.stage != ParseStage::PrologStart && st.stage != ParseStage::Prolog {
                    return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                }
                let Some(end_rel) = find_doctype_end(rem) else {
                    if !isfinal {
                        return ParseStep::NeedMore;
                    }
                    return st.set_error(ERR_UNCLOSED_TOKEN, start_line, start_col, start_byte);
                };
                let raw = rem[..=end_rel].to_string();
                let inner = rem[9..end_rel].to_string();
                st.pos += raw.len();
                st.advance_pos(&raw);
                st.stage = ParseStage::Prolog;

                let Some((doc_name, mut after_name)) = parse_token(&inner) else {
                    return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                };
                after_name = after_name.trim_start_matches(is_xml_ws);
                let mut pubid: Option<String> = None;
                let mut sysid: Option<String> = None;
                if let Some(r) = after_name.strip_prefix("SYSTEM") {
                    let Some((s, rem2)) = parse_quoted_str(r) else {
                        return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                    };
                    sysid = Some(s);
                    st.has_external_dtd = true;
                    after_name = rem2.trim_start_matches(is_xml_ws);
                } else if let Some(r) = after_name.strip_prefix("PUBLIC") {
                    let Some((p, rem2)) = parse_quoted_str(r) else {
                        return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                    };
                    let Some((s, rem3)) = parse_quoted_str(rem2) else {
                        return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                    };
                    pubid = Some(p);
                    sysid = Some(s);
                    st.has_external_dtd = true;
                    after_name = rem3.trim_start_matches(is_xml_ws);
                }

                let (has_internal, subset_str) = if let Some(bracket_start) = after_name.find('[') {
                    let Some(bracket_end) = after_name.rfind(']') else {
                        return st.set_error(ERR_SYNTAX, start_line, start_col, start_byte);
                    };
                    (
                        1i64,
                        Some(after_name[bracket_start + 1..bracket_end].to_string()),
                    )
                } else {
                    (0i64, None)
                };

                st.pending_events.push(ParserEvent::StartDoctype {
                    name: doc_name.to_string(),
                    sysid,
                    pubid,
                    has_internal_subset: has_internal,
                    raw,
                    line: start_line,
                    col: start_col,
                    byte: start_byte,
                });
                if let Some(sub) = subset_str {
                    match parse_doctype_subset(st, &sub, start_line, start_col, start_byte) {
                        Ok(sub_events) => st.pending_events.extend(sub_events),
                        Err(code) => return st.set_error(code, start_line, start_col, start_byte),
                    }
                }
                st.pending_events.push(ParserEvent::EndDoctype {
                    line: st.cursor_line,
                    col: st.cursor_col,
                    byte: st.cursor_byte,
                });
                continue;
            }

            // Could be partial `<!--`, `<![CDATA[`, `<!DOCTYPE`
            if rem.starts_with("<!")
                && !isfinal
                && ("<!--".starts_with(rem)
                    || "<![CDATA[".starts_with(rem)
                    || "<!DOCTYPE".starts_with(rem))
            {
                return ParseStep::NeedMore;
            }

            // 5. End tag `</name>`
            if rem.starts_with("</") {
                if st.stage != ParseStage::InRoot {
                    return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
                }
                let Some(end_rel) = rem.find('>') else {
                    if !isfinal {
                        return ParseStep::NeedMore;
                    }
                    return st.set_error(ERR_UNCLOSED_TOKEN, start_line, start_col, start_byte);
                };
                let raw = rem[..=end_rel].to_string();
                let name = rem[2..end_rel].trim_end_matches(is_xml_ws).to_string();
                if !is_valid_xml_name(&name) {
                    return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
                }
                let Some(top) = st.stack.pop() else {
                    return st.set_error(ERR_TAG_MISMATCH, start_line, start_col, start_byte);
                };
                if top.raw_name != name {
                    // Report column at tag name start (col + 2), matching Expat
                    return st.set_error(
                        ERR_TAG_MISMATCH,
                        start_line,
                        start_col + 2,
                        start_byte + 2,
                    );
                }
                st.pos += raw.len();
                st.advance_pos(&raw);

                // Restore namespace bindings
                for (pfx, prev) in top.saved_bindings.into_iter().rev() {
                    match prev {
                        Some(uri) => {
                            st.ns_bindings.insert(pfx, uri);
                        }
                        None => {
                            st.ns_bindings.remove(&pfx);
                        }
                    }
                }

                if st.stack.is_empty() {
                    st.stage = ParseStage::Epilog;
                }

                st.pending_events.push(ParserEvent::EndElement {
                    name: top.expanded_name,
                    raw,
                    line: start_line,
                    col: start_col,
                    byte: start_byte,
                });
                for pfx in top.declared_prefixes.into_iter().rev() {
                    st.pending_events.push(ParserEvent::EndNamespace {
                        prefix: pfx,
                        line: start_line,
                        col: start_col,
                        byte: start_byte,
                    });
                }
                continue;
            }

            // 6. Start tag `<name ...>` or empty element `<name .../>`
            if st.stage == ParseStage::Epilog {
                return st.set_error(
                    ERR_JUNK_AFTER_DOC_ELEMENT,
                    start_line,
                    start_col,
                    start_byte,
                );
            }
            let Some(end_rel) = find_tag_end(rem) else {
                if !isfinal {
                    return ParseStep::NeedMore;
                }
                return st.set_error(ERR_UNCLOSED_TOKEN, start_line, start_col, start_byte);
            };
            let raw = rem[..=end_rel].to_string();
            let mut inner = rem[1..end_rel].to_string();
            let self_closing = inner.ends_with('/');
            if self_closing {
                inner.pop();
            }

            // Parse element tag name
            let name_end = inner.find(is_xml_ws).unwrap_or(inner.len());
            let raw_tag = inner[..name_end].to_string();
            if !is_valid_xml_name(&raw_tag) {
                return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
            }

            // Parse attributes
            let mut raw_attrs: Vec<(String, String, usize)> = Vec::new();
            let mut seen_raw_names: HashSet<String> = HashSet::new();
            let mut a_rest = &inner[name_end..];
            let mut attr_err: Option<(i64, usize)> = None;
            while !a_rest.is_empty() {
                let ws_len = a_rest.find(|c: char| !is_xml_ws(c)).unwrap_or(a_rest.len());
                if ws_len == a_rest.len() {
                    break;
                }
                if ws_len == 0 && !raw_attrs.is_empty() {
                    attr_err = Some((ERR_INVALID_TOKEN, inner.len() - a_rest.len() + 1));
                    break;
                }
                a_rest = &a_rest[ws_len..];
                let attr_offset = inner.len() - a_rest.len() + 1;
                let a_name_end = a_rest
                    .find(|c: char| is_xml_ws(c) || c == '=')
                    .unwrap_or(a_rest.len());
                let a_name = &a_rest[..a_name_end];
                if !is_valid_xml_name(a_name) {
                    attr_err = Some((ERR_INVALID_TOKEN, attr_offset));
                    break;
                }
                if !seen_raw_names.insert(a_name.to_string()) {
                    attr_err = Some((ERR_DUPLICATE_ATTRIBUTE, attr_offset));
                    break;
                }
                a_rest = a_rest[a_name_end..].trim_start_matches(is_xml_ws);
                let Some(after_eq) = a_rest.strip_prefix('=') else {
                    attr_err = Some((ERR_INVALID_TOKEN, attr_offset));
                    break;
                };
                let after_eq = after_eq.trim_start_matches(is_xml_ws);
                let Some(quote) = after_eq.chars().next() else {
                    attr_err = Some((ERR_INVALID_TOKEN, attr_offset));
                    break;
                };
                if quote != '"' && quote != '\'' {
                    attr_err = Some((ERR_INVALID_TOKEN, attr_offset));
                    break;
                }
                let after_q = &after_eq[1..];
                let Some(end_q) = after_q.find(quote) else {
                    attr_err = Some((ERR_UNCLOSED_TOKEN, attr_offset));
                    break;
                };
                let raw_val = &after_q[..end_q];
                let mut ent_seen = HashSet::new();
                let (val, skipped) =
                    match expand_entity_value(&st.entities, raw_val, true, &mut ent_seen) {
                        Ok(r) => r,
                        Err(code) => {
                            attr_err = Some((code, attr_offset));
                            break;
                        }
                    };
                if !skipped.is_empty() && !st.has_external_dtd && !st.use_foreign_dtd {
                    attr_err = Some((ERR_UNDEFINED_ENTITY, attr_offset));
                    break;
                }
                raw_attrs.push((a_name.to_string(), val, attr_offset));
                a_rest = &after_q[end_q + 1..];
            }
            if let Some((code, rel_off)) = attr_err {
                return st.set_error(code, start_line, start_col + rel_off, start_byte + rel_off);
            }

            // Apply DTD default attributes if `!specified_attributes`
            if !st.specified_attributes
                && let Some(defaults) = st.default_attrs.get(&raw_tag).cloned()
            {
                for (dn, dv) in defaults {
                    if !seen_raw_names.contains(&dn) {
                        seen_raw_names.insert(dn.clone());
                        raw_attrs.push((dn, dv, 0));
                    }
                }
            }

            // Namespace handling
            let mut declared_prefixes: Vec<Option<String>> = Vec::new();
            let mut saved_bindings: Vec<(Option<String>, Option<String>)> = Vec::new();
            let mut final_attrs: Vec<(String, String)> = Vec::new();
            let expanded_tag: String;

            if let Some(sep) = st.namespace_separator.clone() {
                // First pass: process xmlns / xmlns:* declarations
                let mut non_ns_attrs: Vec<(String, String, usize)> = Vec::new();
                for (aname, aval, aoff) in raw_attrs {
                    if aname == "xmlns" {
                        if aval == XML_NS || aval == XMLNS_NS {
                            return st.set_error(
                                ERR_RESERVED_NAMESPACE_URI,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                        let prev = st.ns_bindings.get(&None).cloned();
                        saved_bindings.push((None, prev));
                        declared_prefixes.push(None);
                        if aval.is_empty() {
                            st.ns_bindings.remove(&None);
                            st.pending_events.push(ParserEvent::StartNamespace {
                                prefix: None,
                                uri: None,
                                line: start_line,
                                col: start_col,
                                byte: start_byte,
                            });
                        } else {
                            st.ns_bindings.insert(None, aval.clone());
                            st.pending_events.push(ParserEvent::StartNamespace {
                                prefix: None,
                                uri: Some(aval),
                                line: start_line,
                                col: start_col,
                                byte: start_byte,
                            });
                        }
                    } else if let Some(pfx) = aname.strip_prefix("xmlns:") {
                        if pfx.is_empty() || pfx.contains(':') {
                            return st.set_error(
                                ERR_INVALID_TOKEN,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                        if pfx == "xmlns" {
                            return st.set_error(
                                ERR_RESERVED_PREFIX_XMLNS,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                        if pfx == "xml" && aval != XML_NS {
                            return st.set_error(
                                ERR_RESERVED_PREFIX_XML,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                        if (pfx != "xml" && aval == XML_NS) || aval == XMLNS_NS {
                            return st.set_error(
                                ERR_RESERVED_NAMESPACE_URI,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                        if aval.is_empty() {
                            return st.set_error(
                                ERR_UNDECLARING_PREFIX,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                        let key = Some(pfx.to_string());
                        let prev = st.ns_bindings.get(&key).cloned();
                        saved_bindings.push((key.clone(), prev));
                        declared_prefixes.push(key.clone());
                        st.ns_bindings.insert(key, aval.clone());
                        st.pending_events.push(ParserEvent::StartNamespace {
                            prefix: Some(pfx.to_string()),
                            uri: Some(aval),
                            line: start_line,
                            col: start_col,
                            byte: start_byte,
                        });
                    } else {
                        non_ns_attrs.push((aname, aval, aoff));
                    }
                }

                // Resolve element tag QName
                let (tag_pfx, tag_local) = match split_qname(&raw_tag) {
                    Ok(v) => v,
                    Err(code) => return st.set_error(code, start_line, start_col, start_byte),
                };
                expanded_tag = match tag_pfx {
                    Some(pfx) => {
                        let Some(uri) = st.ns_bindings.get(&Some(pfx.to_string())) else {
                            return st.set_error(
                                ERR_UNBOUND_PREFIX,
                                start_line,
                                start_col,
                                start_byte,
                            );
                        };
                        format_ns_name(uri, tag_local, Some(pfx), &sep, st.namespace_prefixes)
                    }
                    None => {
                        if let Some(uri) = st.ns_bindings.get(&None) {
                            format_ns_name(uri, tag_local, None, &sep, st.namespace_prefixes)
                        } else {
                            raw_tag.clone()
                        }
                    }
                };

                // Resolve attribute QNames (unprefixed attributes do NOT get default ns)
                let mut seen_expanded_attrs: HashSet<String> = HashSet::new();
                for (aname, aval, aoff) in non_ns_attrs {
                    let (apfx, alocal) = match split_qname(&aname) {
                        Ok(v) => v,
                        Err(code) => {
                            return st.set_error(
                                code,
                                start_line,
                                start_col + aoff,
                                start_byte + aoff,
                            );
                        }
                    };
                    let (dedup_key, exp_attr) = match apfx {
                        Some(pfx) => {
                            let Some(uri) = st.ns_bindings.get(&Some(pfx.to_string())) else {
                                return st.set_error(
                                    ERR_UNBOUND_PREFIX,
                                    start_line,
                                    start_col + aoff,
                                    start_byte + aoff,
                                );
                            };
                            (
                                format!("{uri}\0{alocal}"),
                                format_ns_name(uri, alocal, Some(pfx), &sep, st.namespace_prefixes),
                            )
                        }
                        None => (aname.clone(), aname),
                    };
                    if !seen_expanded_attrs.insert(dedup_key) {
                        return st.set_error(
                            ERR_DUPLICATE_ATTRIBUTE,
                            start_line,
                            start_col + aoff,
                            start_byte + aoff,
                        );
                    }
                    final_attrs.push((exp_attr, aval));
                }
            } else {
                expanded_tag = raw_tag.clone();
                final_attrs = raw_attrs.into_iter().map(|(k, v, _)| (k, v)).collect();
            }

            st.pos += raw.len();
            st.advance_pos(&raw);

            st.pending_events.push(ParserEvent::StartElement {
                name: expanded_tag.clone(),
                attrs: final_attrs,
                raw: raw.clone(),
                line: start_line,
                col: start_col,
                byte: start_byte,
            });

            if self_closing {
                for (pfx, prev) in saved_bindings.into_iter().rev() {
                    match prev {
                        Some(uri) => {
                            st.ns_bindings.insert(pfx, uri);
                        }
                        None => {
                            st.ns_bindings.remove(&pfx);
                        }
                    }
                }
                st.pending_events.push(ParserEvent::EndElement {
                    name: expanded_tag,
                    raw,
                    line: st.cursor_line,
                    col: st.cursor_col,
                    byte: st.cursor_byte,
                });
                for pfx in declared_prefixes.into_iter().rev() {
                    st.pending_events.push(ParserEvent::EndNamespace {
                        prefix: pfx,
                        line: st.cursor_line,
                        col: st.cursor_col,
                        byte: st.cursor_byte,
                    });
                }
                if st.stack.is_empty() {
                    st.stage = ParseStage::Epilog;
                }
            } else {
                st.stage = ParseStage::InRoot;
                st.stack.push(OpenElement {
                    raw_name: raw_tag,
                    expanded_name: expanded_tag,
                    declared_prefixes,
                    saved_bindings,
                });
            }
            continue;
        }

        // Character data (outside `<`)
        let next_lt = rem.find('<').unwrap_or(rem.len());
        let chunk = &rem[..next_lt];

        // Outside root element, only XML whitespace is allowed
        if st.stage != ParseStage::InRoot {
            if let Some(non_ws_idx) = chunk.find(|c: char| !is_xml_ws(c)) {
                let prefix_ws = chunk[..non_ws_idx].to_string();
                st.advance_pos(&prefix_ws);
                st.pos += non_ws_idx;
                let code = if st.stage == ParseStage::Epilog {
                    ERR_JUNK_AFTER_DOC_ELEMENT
                } else {
                    ERR_SYNTAX
                };
                let (l, c, b) = (st.cursor_line, st.cursor_col, st.cursor_byte);
                return st.set_error(code, l, c, b);
            }
            let raw_ws = chunk.to_string();
            st.pos += raw_ws.len();
            st.advance_pos(&raw_ws);
            if st.stage == ParseStage::PrologStart && !raw_ws.is_empty() {
                st.stage = ParseStage::Prolog;
            }
            if !raw_ws.is_empty() {
                return ParseStep::Emit(ParserEvent::DefaultOnly {
                    raw: raw_ws,
                    line: start_line,
                    col: start_col,
                    byte: start_byte,
                });
            }
            continue;
        }

        // Inside root element: scan up to `<` or `&`
        if let Some(amp_idx) = chunk.find('&') {
            if amp_idx > 0 {
                let plain = &chunk[..amp_idx];
                if plain.contains("]]>") {
                    return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
                }
                let raw = plain.to_string();
                let data = normalize_newlines(&raw);
                st.pos += raw.len();
                st.advance_pos(&raw);
                return ParseStep::Emit(ParserEvent::CharData {
                    data,
                    raw,
                    line: start_line,
                    col: start_col,
                    byte: start_byte,
                });
            }
            // Starts with '&'
            let after_amp = &chunk[1..];
            let Some(semi) = after_amp.find(';') else {
                if !isfinal && next_lt == rem.len() && after_amp.len() < 128 {
                    return ParseStep::NeedMore;
                }
                return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
            };
            let ent_name = after_amp[..semi].to_string();
            let raw_ent = chunk[..1 + semi + 1].to_string();
            if ent_name.is_empty() {
                return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
            }
            if ent_name.starts_with('#') {
                match expand_char_ref(&ent_name) {
                    Ok(c) => {
                        st.pos += raw_ent.len();
                        st.advance_pos(&raw_ent);
                        return ParseStep::Emit(ParserEvent::CharData {
                            data: c.to_string(),
                            raw: raw_ent,
                            line: start_line,
                            col: start_col,
                            byte: start_byte,
                        });
                    }
                    Err(code) => return st.set_error(code, start_line, start_col, start_byte),
                }
            }
            match ent_name.as_str() {
                "lt" | "gt" | "amp" | "apos" | "quot" => {
                    let ch = match ent_name.as_str() {
                        "lt" => "<",
                        "gt" => ">",
                        "amp" => "&",
                        "apos" => "'",
                        _ => "\"",
                    };
                    st.pos += raw_ent.len();
                    st.advance_pos(&raw_ent);
                    return ParseStep::Emit(ParserEvent::CharData {
                        data: ch.to_string(),
                        raw: raw_ent,
                        line: start_line,
                        col: start_col,
                        byte: start_byte,
                    });
                }
                _ => {
                    if !is_valid_xml_name(&ent_name) {
                        return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
                    }
                    if let Some(info) = st.entities.get(&ent_name).cloned() {
                        if let Some(val) = info.value {
                            let mut seen = HashSet::new();
                            seen.insert(ent_name.clone());
                            match expand_entity_value(&st.entities, &val, false, &mut seen) {
                                Ok((expanded, skipped)) => {
                                    if !skipped.is_empty()
                                        && !st.has_external_dtd
                                        && !st.use_foreign_dtd
                                    {
                                        return st.set_error(
                                            ERR_UNDEFINED_ENTITY,
                                            start_line,
                                            start_col,
                                            start_byte,
                                        );
                                    }
                                    st.pos += raw_ent.len();
                                    st.advance_pos(&raw_ent);
                                    return ParseStep::Emit(ParserEvent::CharData {
                                        data: normalize_newlines(&expanded),
                                        raw: raw_ent,
                                        line: start_line,
                                        col: start_col,
                                        byte: start_byte,
                                    });
                                }
                                Err(code) => {
                                    return st.set_error(code, start_line, start_col, start_byte);
                                }
                            }
                        } else {
                            st.pos += raw_ent.len();
                            st.advance_pos(&raw_ent);
                            return ParseStep::Emit(ParserEvent::SkippedEntity {
                                name: ent_name,
                                is_param: 0,
                                raw: raw_ent,
                                line: start_line,
                                col: start_col,
                                byte: start_byte,
                            });
                        }
                    } else if st.has_external_dtd || st.use_foreign_dtd {
                        st.pos += raw_ent.len();
                        st.advance_pos(&raw_ent);
                        return ParseStep::Emit(ParserEvent::SkippedEntity {
                            name: ent_name,
                            is_param: 0,
                            raw: raw_ent,
                            line: start_line,
                            col: start_col,
                            byte: start_byte,
                        });
                    } else {
                        return st.set_error(
                            ERR_UNDEFINED_ENTITY,
                            start_line,
                            start_col,
                            start_byte,
                        );
                    }
                }
            }
        }

        // Plain character data without `<` or `&`
        // Guard against `]]>` split across chunk boundary
        let emit_len = if !isfinal && next_lt == rem.len() && chunk.ends_with(']') {
            let trimmed = chunk.trim_end_matches(']');
            if trimmed.is_empty() {
                return ParseStep::NeedMore;
            }
            trimmed.len()
        } else {
            chunk.len()
        };
        let slice = &chunk[..emit_len];
        if slice.contains("]]>") {
            return st.set_error(ERR_INVALID_TOKEN, start_line, start_col, start_byte);
        }
        // Avoid splitting `\r\n` across chunks
        let emit_len = if !isfinal && next_lt == rem.len() && slice.ends_with('\r') {
            if slice.len() == 1 {
                return ParseStep::NeedMore;
            }
            slice.len() - 1
        } else {
            emit_len
        };
        let raw = chunk[..emit_len].to_string();
        let data = normalize_newlines(&raw);
        st.pos += raw.len();
        st.advance_pos(&raw);
        return ParseStep::Emit(ParserEvent::CharData {
            data,
            raw,
            line: start_line,
            col: start_col,
            byte: start_byte,
        });
    }
}

fn intern_str(interp: &mut Interpreter, intern_opt: &Option<Value>, s: &str) -> Value {
    let v = Value::string(s);
    if let Some(dict_val) = intern_opt
        && let Ok(res) = call_method(interp, dict_val, "setdefault", &[v.clone(), v.clone()])
    {
        return res;
    }
    v
}

fn opt_str_val(s: Option<String>) -> Value {
    match s {
        Some(v) => Value::string(v),
        None => Value::none(),
    }
}

fn invoke_default_handler(
    interp: &mut Interpreter,
    default_h: Option<Value>,
    default_exp_h: Option<Value>,
    raw: &str,
) -> Result<()> {
    if let Some(h) = default_exp_h.or(default_h) {
        call_pos(interp, h, &[Value::string(raw)])?;
    }
    Ok(())
}

// ── Native module definition ─────────────────────────────────────────────────

pyrust_module! {
    constants {
        "EXPAT_VERSION" => Value::string("expat_2.6.0"),
        "version_info" => Value::tuple(vec![Value::int(2), Value::int(6), Value::int(0)]),
        "native_encoding" => Value::string("UTF-8"),
        "features" => make_features_list(),
        "XML_PARAM_ENTITY_PARSING_NEVER" => Value::int(0),
        "XML_PARAM_ENTITY_PARSING_UNLESS_STANDALONE" => Value::int(1),
        "XML_PARAM_ENTITY_PARSING_ALWAYS" => Value::int(2),
    }

    exception ExpatError;

    #[py_name = "ErrorString"]
    fn error_string_fn(code: PyIndex) -> Result<Value> {
        let c = code.expect_i64(FN_NAME, "code")?;
        Ok(match super::pyexpat_errors::error_string(c) {
            Some(msg) => Value::string(msg),
            None => Value::none(),
        })
    }

    #[py_name = "ParserCreate"]
    fn parser_create(
        #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] namespace_separator: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] intern: OptValueRef<'_>,
    ) -> Result<Value> {
        let enc_opt = match encoding.as_value() {
            None => None,
            Some(v) if v.is_none() => None,
            Some(v) => {
                let Some(s) = v.as_str() else {
                    return Err(PyError::named(
                        "TypeError",
                        "ParserCreate() argument 'encoding' must be str or None",
                    ));
                };
                Some(s.to_string())
            }
        };
        let sep_opt = match namespace_separator.as_value() {
            None => None,
            Some(v) if v.is_none() => None,
            Some(v) => {
                let Some(s) = v.as_str() else {
                    return Err(PyError::named(
                        "TypeError",
                        "ParserCreate() argument 'namespace_separator' must be str or None",
                    ));
                };
                if s.chars().count() > 1 {
                    return Err(PyError::named(
                        "ValueError",
                        "namespace_separator must be at most one character, omitted, or None",
                    ));
                }
                Some(s.to_string())
            }
        };
        let intern_opt = match intern.as_value() {
            None => None,
            Some(v) if v.is_none() => None,
            Some(v) => {
                if !matches!(v.kind(), ValueKind::Dict(_)) {
                    return Err(PyError::named("TypeError", "intern must be a dictionary"));
                }
                Some(v.clone())
            }
        };

        let m = _interp.load_module("pyexpat")?;
        let cls = _interp.get_attr(&m, "xmlparser")?;
        let inst = call_pos(_interp, cls, &[])?;
        let state = ExpatParserState::new(enc_opt, sep_opt, intern_opt);
        xmlparser_set_state(&inst, state)?;
        Ok(inst)
    }

    #[native_state(ExpatParserState)]
    class xmlparser {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            if !has_state(self_val.0) {
                set_state(self_val.0, ExpatParserState::new(None, None, None))?;
            }
            Ok(Value::none())
        }

        #[py_name = "Parse"]
        fn parse(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
            #[default(OptionalArg::Omitted)] isfinal: OptValueRef<'_>,
        ) -> Result<Value> {
            let is_final = match isfinal.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };
            let bytes: Vec<u8> = match data.0.kind() {
                ValueKind::Str(s) => s.as_bytes().to_vec(),
                ValueKind::Bytes(b) => b.as_ref().to_vec(),
                _ => {
                    let b = PyBytesLike::try_from_value_with_interp(
                        _interp,
                        &data.0,
                        FN_NAME,
                        "data",
                    )?;
                    b.into_vec()
                }
            };

            let already_finished = with_state_mut(self_val.0, |st| {
                if st.stage == ParseStage::Finished {
                    let (l, c, b) = (st.cursor_line, st.cursor_col, st.cursor_byte);
                    st.error_code = ERR_FINISHED;
                    st.error_line = l;
                    st.error_col = c;
                    st.error_byte = b as i64;
                    return Ok(Some((l, c)));
                }
                st.started = true;
                st.last_context = bytes.clone();
                st.raw_pending.extend_from_slice(&bytes);
                Ok(None)
            })?;
            if let Some((l, c)) = already_finished {
                return Err(make_expat_error(_interp, ERR_FINISHED, l, c));
            }

            // Decode bytes into UTF-8 text buffer
            let dec_res = with_state_mut(self_val.0, |st| {
                match decode_pending_bytes(_interp, st, is_final) {
                    Ok(()) => Ok(None),
                    Err((code, l, c)) => {
                        let b = st.cursor_byte;
                        st.error_code = code;
                        st.error_line = l;
                        st.error_col = c;
                        st.error_byte = b as i64;
                        Ok(Some((code, l, c)))
                    }
                }
            })?;
            if let Some((code, l, c)) = dec_res {
                return Err(make_expat_error(_interp, code, l, c));
            }

            // Helper closure to flush buffered character data outside any state borrow
            let flush_chardata = |interp: &mut Interpreter, self_v: &Value| -> Result<()> {
                let pending = with_state_mut(self_v, |st| {
                    if st.chardata_buf.is_empty() {
                        Ok(None)
                    } else {
                        let s = std::mem::take(&mut st.chardata_buf);
                        let h = st.handlers.character_data.clone();
                        let dh = st.handlers.default.clone();
                        let deh = st.handlers.default_expand.clone();
                        st.in_callback = true;
                        Ok(Some((s, h, dh, deh)))
                    }
                })?;
                if let Some((s, h, dh, deh)) = pending {
                    let res = if let Some(cb) = h {
                        call_pos(interp, cb, &[Value::string(&s)]).map(|_| ())
                    } else {
                        invoke_default_handler(interp, dh, deh, &s)
                    };
                    let _ = with_state_mut(self_v, |st| {
                        st.in_callback = false;
                        Ok(())
                    });
                    res?;
                }
                Ok(())
            };

            loop {
                let step = with_state_mut(self_val.0, |st| Ok(next_step(st, is_final)))?;
                match step {
                    ParseStep::NeedMore => {
                        if !with_state(self_val.0, |st| Ok(st.buffer_text))? {
                            flush_chardata(_interp, self_val.0)?;
                        }
                        with_state_mut(self_val.0, |st| {
                            st.compact_if_needed();
                            Ok(())
                        })?;
                        break;
                    }
                    ParseStep::Done => {
                        flush_chardata(_interp, self_val.0)?;
                        with_state_mut(self_val.0, |st| {
                            st.compact_if_needed();
                            Ok(())
                        })?;
                        break;
                    }
                    ParseStep::Error { code, line, col } => {
                        flush_chardata(_interp, self_val.0)?;
                        return Err(make_expat_error(_interp, code, line, col));
                    }
                    ParseStep::Emit(event) => {
                        // CharData can be buffered when `buffer_text` is True
                        if let ParserEvent::CharData {
                            data,
                            raw,
                            line,
                            col,
                            byte,
                        } = event
                        {
                            let should_emit_now = with_state_mut(self_val.0, |st| {
                                st.record_event_pos(line, col, byte);
                                if st.buffer_text && st.handlers.character_data.is_some() {
                                    st.chardata_buf.push_str(&data);
                                    if st.chardata_buf.len() >= st.buffer_size {
                                        Ok(None) // flush right below
                                    } else {
                                        Ok(Some(false))
                                    }
                                } else {
                                    Ok(Some(true))
                                }
                            })?;
                            match should_emit_now {
                                Some(false) => continue,
                                None => {
                                    flush_chardata(_interp, self_val.0)?;
                                    continue;
                                }
                                Some(true) => {
                                    let (h, dh, deh) = with_state_mut(self_val.0, |st| {
                                        st.in_callback = true;
                                        Ok((
                                            st.handlers.character_data.clone(),
                                            st.handlers.default.clone(),
                                            st.handlers.default_expand.clone(),
                                        ))
                                    })?;
                                    let res = if let Some(cb) = h {
                                        call_pos(_interp, cb, &[Value::string(&data)]).map(|_| ())
                                    } else {
                                        invoke_default_handler(_interp, dh, deh, &raw)
                                    };
                                    let _ = with_state_mut(self_val.0, |st| {
                                        st.in_callback = false;
                                        Ok(())
                                    });
                                    res?;
                                    continue;
                                }
                            }
                        }

                        // Flush any buffered character data before non-chardata events
                        flush_chardata(_interp, self_val.0)?;

                        // Dispatch non-chardata event outside state borrow
                        let cb_res = match event {
                            ParserEvent::XmlDecl {
                                version,
                                encoding,
                                standalone,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.xml_decl.clone(),
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                    ))
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[
                                            Value::string(version),
                                            opt_str_val(encoding),
                                            Value::int(standalone),
                                        ],
                                    )
                                    .map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::StartDoctype {
                                name,
                                sysid,
                                pubid,
                                has_internal_subset,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.start_doctype_decl.clone(),
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                    ))
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[
                                            Value::string(name),
                                            opt_str_val(sysid),
                                            opt_str_val(pubid),
                                            Value::int(has_internal_subset),
                                        ],
                                    )
                                    .map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::EndDoctype { line, col, byte } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.end_doctype_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(_interp, cb, &[]).map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::EntityDecl {
                                name,
                                is_param,
                                value,
                                base,
                                sysid,
                                pubid,
                                notation,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.entity_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[
                                            Value::string(name),
                                            Value::int(is_param),
                                            opt_str_val(value),
                                            opt_str_val(base),
                                            opt_str_val(sysid),
                                            opt_str_val(pubid),
                                            opt_str_val(notation),
                                        ],
                                    )
                                    .map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::UnparsedEntityDecl {
                                name,
                                base,
                                sysid,
                                pubid,
                                notation,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.unparsed_entity_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[
                                            Value::string(name),
                                            opt_str_val(base),
                                            opt_str_val(sysid),
                                            opt_str_val(pubid),
                                            Value::string(notation),
                                        ],
                                    )
                                    .map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::ElementDecl {
                                name,
                                model,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.element_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(_interp, cb, &[Value::string(name), model]).map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::AttlistDecl {
                                elname,
                                attname,
                                att_type,
                                default,
                                required,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.attlist_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[
                                            Value::string(elname),
                                            Value::string(attname),
                                            Value::string(att_type),
                                            opt_str_val(default),
                                            Value::int(required),
                                        ],
                                    )
                                    .map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::NotationDecl {
                                name,
                                base,
                                sysid,
                                pubid,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.notation_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[
                                            Value::string(name),
                                            opt_str_val(base),
                                            opt_str_val(sysid),
                                            opt_str_val(pubid),
                                        ],
                                    )
                                    .map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::StartNamespace {
                                prefix,
                                uri,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.start_namespace_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[opt_str_val(prefix), opt_str_val(uri)],
                                    )
                                    .map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::EndNamespace {
                                prefix,
                                line,
                                col,
                                byte,
                            } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.end_namespace_decl.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(_interp, cb, &[opt_str_val(prefix)]).map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::StartElement {
                                name,
                                attrs,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh, ordered, intern_opt) =
                                    with_state_mut(self_val.0, |st| {
                                        st.record_event_pos(line, col, byte);
                                        st.in_callback = true;
                                        Ok((
                                            st.handlers.start_element.clone(),
                                            st.handlers.default.clone(),
                                            st.handlers.default_expand.clone(),
                                            st.ordered_attributes,
                                            st.intern.clone(),
                                        ))
                                    })?;
                                if let Some(cb) = h {
                                    let name_val = intern_str(_interp, &intern_opt, &name);
                                    let attrs_val = if ordered {
                                        let mut list = Vec::with_capacity(attrs.len() * 2);
                                        for (k, v) in attrs {
                                            list.push(intern_str(_interp, &intern_opt, &k));
                                            list.push(Value::string(v));
                                        }
                                        Value::list(list)
                                    } else {
                                        let mut d = PyDict::default();
                                        for (k, v) in attrs {
                                            let kv = intern_str(_interp, &intern_opt, &k);
                                            let key_s = kv.as_str().unwrap_or(&k);
                                            d.insert(PyKey::str_from(key_s), Value::string(v));
                                        }
                                        Value::dict(d)
                                    };
                                    call_pos(_interp, cb, &[name_val, attrs_val]).map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::EndElement {
                                name,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh, intern_opt) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.end_element.clone(),
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                        st.intern.clone(),
                                    ))
                                })?;
                                if let Some(cb) = h {
                                    let name_val = intern_str(_interp, &intern_opt, &name);
                                    call_pos(_interp, cb, &[name_val]).map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::Comment {
                                data,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.comment.clone(),
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                    ))
                                })?;
                                if let Some(cb) = h {
                                    call_pos(_interp, cb, &[Value::string(data)]).map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::ProcessingInstruction {
                                target,
                                data,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.processing_instruction.clone(),
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                    ))
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[Value::string(target), Value::string(data)],
                                    )
                                    .map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::StartCdata { line, col, byte } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.start_cdata_section.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(_interp, cb, &[]).map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::EndCdata { line, col, byte } => {
                                let h = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok(st.handlers.end_cdata_section.clone())
                                })?;
                                if let Some(cb) = h {
                                    call_pos(_interp, cb, &[]).map(|_| ())
                                } else {
                                    Ok(())
                                }
                            }
                            ParserEvent::SkippedEntity {
                                name,
                                is_param,
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (h, dh, deh) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.skipped_entity.clone(),
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                    ))
                                })?;
                                if let Some(cb) = h {
                                    call_pos(
                                        _interp,
                                        cb,
                                        &[Value::string(name), Value::int(is_param)],
                                    )
                                    .map(|_| ())
                                } else {
                                    invoke_default_handler(_interp, dh, deh, &raw)
                                }
                            }
                            ParserEvent::DefaultOnly {
                                raw,
                                line,
                                col,
                                byte,
                            } => {
                                let (dh, deh) = with_state_mut(self_val.0, |st| {
                                    st.record_event_pos(line, col, byte);
                                    st.in_callback = true;
                                    Ok((
                                        st.handlers.default.clone(),
                                        st.handlers.default_expand.clone(),
                                    ))
                                })?;
                                invoke_default_handler(_interp, dh, deh, &raw)
                            }
                            ParserEvent::CharData { .. } => unreachable!(),
                        };
                        let _ = with_state_mut(self_val.0, |st| {
                            st.in_callback = false;
                            Ok(())
                        });
                        cb_res?;
                    }
                }
            }

            Ok(Value::int(1))
        }

        #[py_name = "ParseFile"]
        fn parse_file(
            #[positional_only] self_val: PyValueRef<'_>,
            file: PyValue,
        ) -> Result<Value> {
            let read_fn = _interp.get_attr(&file.0, "read")?;
            let parse_fn = _interp.get_attr(self_val.0, "Parse")?;
            loop {
                let chunk = call_pos(_interp, read_fn.clone(), &[Value::int(2048)])?;
                let empty = match chunk.kind() {
                    ValueKind::Bytes(b) => b.is_empty(),
                    ValueKind::Str(s) => s.is_empty(),
                    _ => {
                        let b = PyBytesLike::try_from_value_with_interp(
                            _interp,
                            &chunk,
                            FN_NAME,
                            "chunk",
                        )
                        .map_err(|_| {
                            PyError::named("TypeError", "read() did not return a bytes object")
                        })?;
                        b.as_bytes().is_empty()
                    }
                };
                if empty {
                    call_pos(_interp, parse_fn, &[Value::bytes(Vec::new()), Value::bool_(true)])?;
                    break;
                }
                call_pos(_interp, parse_fn.clone(), &[chunk, Value::bool_(false)])?;
            }
            Ok(Value::int(1))
        }

        #[py_name = "SetBase"]
        fn set_base(#[state_mut] st: &mut ExpatParserState, base: PyStr<'_>) -> Result<Value> {
            st.base = Some(base.0.to_string());
            Ok(Value::none())
        }

        #[py_name = "GetBase"]
        fn get_base(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(opt_str_val(st.base.clone()))
        }

        #[py_name = "GetInputContext"]
        fn get_input_context(#[state] st: &ExpatParserState) -> Result<Value> {
            if st.in_callback && !st.last_context.is_empty() {
                Ok(Value::bytes(st.last_context.clone()))
            } else {
                Ok(Value::none())
            }
        }

        #[py_name = "ExternalEntityParserCreate"]
        fn external_entity_parser_create(
            #[positional_only] self_val: PyValueRef<'_>,
            _context: PyValue,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
        ) -> Result<Value> {
            let enc_opt = match encoding.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => v.as_str().map(str::to_string),
            };
            let child_state = with_state(self_val.0, |st| {
                let mut child = ExpatParserState::new(
                    enc_opt.or_else(|| st.encoding.clone()),
                    st.namespace_separator.clone(),
                    st.intern.clone(),
                );
                child.base = st.base.clone();
                child.buffer_text = st.buffer_text;
                child.buffer_size = st.buffer_size;
                child.namespace_prefixes = st.namespace_prefixes;
                child.ordered_attributes = st.ordered_attributes;
                child.specified_attributes = st.specified_attributes;
                child.ns_bindings = st.ns_bindings.clone();
                child.entities = st.entities.clone();
                child.default_attrs = st.default_attrs.clone();
                child.handlers = st.handlers.clone();
                Ok(child)
            })?;
            let m = _interp.load_module("pyexpat")?;
            let cls = _interp.get_attr(&m, "xmlparser")?;
            let inst = call_pos(_interp, cls, &[])?;
            set_state(&inst, child_state)?;
            Ok(inst)
        }

        #[py_name = "SetParamEntityParsing"]
        fn set_param_entity_parsing(
            #[state_mut] st: &mut ExpatParserState,
            flag: PyIndex,
        ) -> Result<Value> {
            st.param_entity_parsing = flag.expect_i64(FN_NAME, "flag")?;
            Ok(Value::int(1))
        }

        #[py_name = "UseForeignDTD"]
        fn use_foreign_dtd(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] flag: OptValueRef<'_>,
        ) -> Result<Value> {
            let b = match flag.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            with_state_mut(self_val.0, |st| {
                st.use_foreign_dtd = b;
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[py_name = "SetReparseDeferralEnabled"]
        fn set_reparse_deferral_enabled(
            #[positional_only] self_val: PyValueRef<'_>,
            enabled: PyValue,
        ) -> Result<Value> {
            let b = _interp.truthy_value(&enabled.0)?;
            with_state_mut(self_val.0, |st| {
                st.reparse_deferral_enabled = b;
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[py_name = "GetReparseDeferralEnabled"]
        fn get_reparse_deferral_enabled(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::bool_(st.reparse_deferral_enabled))
        }

        // ── Configuration & Position Properties ──────────────────────────────

        #[getter]
        fn buffer_text(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::bool_(st.buffer_text))
        }

        #[setter("buffer_text")]
        fn set_buffer_text(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let b = _interp.truthy_value(&val.0)?;
            with_state_mut(self_val.0, |st| {
                st.buffer_text = b;
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn buffer_size(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.buffer_size as i64))
        }

        #[setter("buffer_size")]
        fn set_buffer_size(#[state_mut] st: &mut ExpatParserState, val: PyIndex) -> Result<Value> {
            let sz = val.expect_i64(FN_NAME, "buffer_size")?;
            if sz <= 0 {
                return Err(PyError::named("ValueError", "buffer_size must be greater than zero"));
            }
            st.buffer_size = sz as usize;
            Ok(Value::none())
        }

        #[getter]
        fn buffer_used(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.chardata_buf.len() as i64))
        }

        #[getter]
        fn namespace_prefixes(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::bool_(st.namespace_prefixes))
        }

        #[setter("namespace_prefixes")]
        fn set_namespace_prefixes(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let b = _interp.truthy_value(&val.0)?;
            with_state_mut(self_val.0, |st| {
                st.namespace_prefixes = b;
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn ordered_attributes(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::bool_(st.ordered_attributes))
        }

        #[setter("ordered_attributes")]
        fn set_ordered_attributes(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let b = _interp.truthy_value(&val.0)?;
            with_state_mut(self_val.0, |st| {
                st.ordered_attributes = b;
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn specified_attributes(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::bool_(st.specified_attributes))
        }

        #[setter("specified_attributes")]
        fn set_specified_attributes(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            let b = _interp.truthy_value(&val.0)?;
            with_state_mut(self_val.0, |st| {
                st.specified_attributes = b;
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn intern(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.intern.clone().unwrap_or_else(Value::none))
        }

        #[getter("CurrentLineNumber")]
        fn current_line_number(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.current_line as i64))
        }

        #[getter("CurrentColumnNumber")]
        fn current_column_number(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.current_col as i64))
        }

        #[getter("CurrentByteIndex")]
        fn current_byte_index(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.current_byte))
        }

        #[getter("ErrorCode")]
        fn error_code(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.error_code))
        }

        #[getter("ErrorLineNumber")]
        fn error_line_number(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.error_line as i64))
        }

        #[getter("ErrorColumnNumber")]
        fn error_column_number(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.error_col as i64))
        }

        #[getter("ErrorByteIndex")]
        fn error_byte_index(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(Value::int(st.error_byte))
        }

        // ── Handler Properties ───────────────────────────────────────────────

        #[getter("StartElementHandler")]
        fn get_start_element_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.start_element.clone().unwrap_or_else(Value::none))
        }
        #[setter("StartElementHandler")]
        fn set_start_element_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.start_element = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("EndElementHandler")]
        fn get_end_element_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.end_element.clone().unwrap_or_else(Value::none))
        }
        #[setter("EndElementHandler")]
        fn set_end_element_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.end_element = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("CharacterDataHandler")]
        fn get_character_data_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.character_data.clone().unwrap_or_else(Value::none))
        }
        #[setter("CharacterDataHandler")]
        fn set_character_data_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.character_data = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("ProcessingInstructionHandler")]
        fn get_pi_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.processing_instruction.clone().unwrap_or_else(Value::none))
        }
        #[setter("ProcessingInstructionHandler")]
        fn set_pi_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.processing_instruction = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("CommentHandler")]
        fn get_comment_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.comment.clone().unwrap_or_else(Value::none))
        }
        #[setter("CommentHandler")]
        fn set_comment_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.comment = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("StartCdataSectionHandler")]
        fn get_start_cdata_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.start_cdata_section.clone().unwrap_or_else(Value::none))
        }
        #[setter("StartCdataSectionHandler")]
        fn set_start_cdata_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.start_cdata_section = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("EndCdataSectionHandler")]
        fn get_end_cdata_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.end_cdata_section.clone().unwrap_or_else(Value::none))
        }
        #[setter("EndCdataSectionHandler")]
        fn set_end_cdata_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.end_cdata_section = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("XmlDeclHandler")]
        fn get_xml_decl_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.xml_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("XmlDeclHandler")]
        fn set_xml_decl_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.xml_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("StartDoctypeDeclHandler")]
        fn get_start_doctype_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.start_doctype_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("StartDoctypeDeclHandler")]
        fn set_start_doctype_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.start_doctype_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("EndDoctypeDeclHandler")]
        fn get_end_doctype_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.end_doctype_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("EndDoctypeDeclHandler")]
        fn set_end_doctype_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.end_doctype_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("StartNamespaceDeclHandler")]
        fn get_start_ns_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.start_namespace_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("StartNamespaceDeclHandler")]
        fn set_start_ns_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.start_namespace_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("EndNamespaceDeclHandler")]
        fn get_end_ns_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.end_namespace_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("EndNamespaceDeclHandler")]
        fn set_end_ns_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.end_namespace_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("DefaultHandler")]
        fn get_default_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.default.clone().unwrap_or_else(Value::none))
        }
        #[setter("DefaultHandler")]
        fn set_default_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.default = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("DefaultHandlerExpand")]
        fn get_default_expand_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.default_expand.clone().unwrap_or_else(Value::none))
        }
        #[setter("DefaultHandlerExpand")]
        fn set_default_expand_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.default_expand = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("EntityDeclHandler")]
        fn get_entity_decl_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.entity_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("EntityDeclHandler")]
        fn set_entity_decl_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.entity_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("ElementDeclHandler")]
        fn get_element_decl_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.element_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("ElementDeclHandler")]
        fn set_element_decl_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.element_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("AttlistDeclHandler")]
        fn get_attlist_decl_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.attlist_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("AttlistDeclHandler")]
        fn set_attlist_decl_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.attlist_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("NotationDeclHandler")]
        fn get_notation_decl_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.notation_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("NotationDeclHandler")]
        fn set_notation_decl_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.notation_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("UnparsedEntityDeclHandler")]
        fn get_unparsed_entity_decl_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.unparsed_entity_decl.clone().unwrap_or_else(Value::none))
        }
        #[setter("UnparsedEntityDeclHandler")]
        fn set_unparsed_entity_decl_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.unparsed_entity_decl = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("SkippedEntityHandler")]
        fn get_skipped_entity_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.skipped_entity.clone().unwrap_or_else(Value::none))
        }
        #[setter("SkippedEntityHandler")]
        fn set_skipped_entity_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.skipped_entity = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("ExternalEntityRefHandler")]
        fn get_external_entity_ref_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.external_entity_ref.clone().unwrap_or_else(Value::none))
        }
        #[setter("ExternalEntityRefHandler")]
        fn set_external_entity_ref_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.external_entity_ref = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }

        #[getter("NotStandaloneHandler")]
        fn get_not_standalone_handler(#[state] st: &ExpatParserState) -> Result<Value> {
            Ok(st.handlers.not_standalone.clone().unwrap_or_else(Value::none))
        }
        #[setter("NotStandaloneHandler")]
        fn set_not_standalone_handler(#[state_mut] st: &mut ExpatParserState, v: PyValue) -> Result<Value> {
            st.handlers.not_standalone = if v.0.is_none() { None } else { Some(v.0) };
            Ok(Value::none())
        }
    }
}

/// Helper so `ParserCreate` can attach state to a freshly constructed `xmlparser`.
fn xmlparser_set_state(inst: &Value, state: ExpatParserState) -> Result<()> {
    crate::builtin_modules::macros::native_state::set_native_state(inst, state)
}

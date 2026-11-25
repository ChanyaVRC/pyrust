// `lzma` module — body for the `lzma @inject` entry in
// `pyrust_builtin_modules!` (issue #3224).
//
// Pure-Rust XZ, LZMA (`.lzma` / FORMAT_ALONE), and raw LZMA1/LZMA2 + pre-filter
// compression/decompression backed by `lzma-rust2` (zero C dependencies).
// The public Python 3.12 surface (`LZMAError`, constants, `is_check_supported`,
// `LZMACompressor`, `LZMADecompressor`, `LZMAFile`, `open`, `compress`,
// `decompress`, plus `_encode_filter_properties` / `_decode_filter_properties`
// used by `zipfile`) is injected from `lzma_py.py`.
//
// Reference: <https://docs.python.org/3/library/lzma.html>

use std::any::Any;
use std::cell::RefCell;
use std::io::{self, Write as IoWrite};
use std::rc::Rc;

use lzma_rust2::filter::bcj::BcjWriter;
use lzma_rust2::filter::delta::DeltaWriter;
use lzma_rust2::{
    Action as LzAction, CheckType, DICT_SIZE_MAX, DICT_SIZE_MIN, EncodeMode, FilterConfig,
    FilterType, Lzma2Options, Lzma2Stream, Lzma2Writer, LzmaOptions, LzmaStream, LzmaWriter,
    MfType, Status as LzStatus, XzOptions, XzStream, XzWriter,
};
use pyrust_core::BuiltinTypeOps;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyKey, PyToPrimitive, Value, ValueKind};

/// Python-source definitions for the public `lzma` surface.
const LZMA_PY_SOURCE: &str = include_str!("lzma_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `lzma.__dict__` after injection so `dir(lzma)` matches CPython.
const LZMA_NATIVE_HELPERS: [&str; 6] = [
    "_is_check_supported",
    "_compressor_new",
    "_compressor_compress",
    "_compressor_flush",
    "_decompressor_new",
    "_decompressor_decompress",
];

/// Public names exported from `LZMA_PY_SOURCE` onto the `lzma` module.
const LZMA_PY_EXPORTS: [&str; 38] = [
    "CHECK_CRC32",
    "CHECK_CRC64",
    "CHECK_ID_MAX",
    "CHECK_NONE",
    "CHECK_SHA256",
    "CHECK_UNKNOWN",
    "FILTER_ARM",
    "FILTER_ARMTHUMB",
    "FILTER_DELTA",
    "FILTER_IA64",
    "FILTER_LZMA1",
    "FILTER_LZMA2",
    "FILTER_POWERPC",
    "FILTER_SPARC",
    "FILTER_X86",
    "FORMAT_ALONE",
    "FORMAT_AUTO",
    "FORMAT_RAW",
    "FORMAT_XZ",
    "LZMACompressor",
    "LZMADecompressor",
    "LZMAError",
    "LZMAFile",
    "MF_BT2",
    "MF_BT3",
    "MF_BT4",
    "MF_HC3",
    "MF_HC4",
    "MODE_FAST",
    "MODE_NORMAL",
    "PRESET_DEFAULT",
    "PRESET_EXTREME",
    "_decode_filter_properties",
    "_encode_filter_properties",
    "compress",
    "decompress",
    "is_check_supported",
    "open",
];

// Format constants
const FORMAT_AUTO: i32 = 0;
const FORMAT_XZ: i32 = 1;
const FORMAT_ALONE: i32 = 2;
const FORMAT_RAW: i32 = 3;

// Check constants
const CHECK_NONE: i32 = 0;
const CHECK_CRC32: i32 = 1;
const CHECK_CRC64: i32 = 4;
const CHECK_SHA256: i32 = 10;
const CHECK_UNKNOWN: i32 = 16;

// Filter IDs
const FILTER_LZMA1: u64 = 0x4000_0000_0000_0001;
const FILTER_LZMA2: u64 = 0x21;
const FILTER_DELTA: u64 = 0x03;
const FILTER_X86: u64 = 0x04;
const FILTER_POWERPC: u64 = 0x05;
const FILTER_IA64: u64 = 0x06;
const FILTER_ARM: u64 = 0x07;
const FILTER_ARMTHUMB: u64 = 0x08;
const FILTER_SPARC: u64 = 0x09;

// Match finders & modes & presets
const MF_HC3: i64 = 3;
const MF_HC4: i64 = 4;
const MF_BT2: i64 = 18;
const MF_BT3: i64 = 19;
const MF_BT4: i64 = 20;

const MODE_FAST: i64 = 1;
const MODE_NORMAL: i64 = 2;

const PRESET_DEFAULT: u32 = 6;
const PRESET_EXTREME: u32 = 0x8000_0000;

const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// Execute `LZMA_PY_SOURCE` once, hide internal helpers, and export the
/// public CPython 3.12 `lzma` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in LZMA_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(LZMA_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("lzma: exec namespace not a dict".into()))?;
    for name in LZMA_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("lzma"));
                if name == "LZMAError" {
                    class.error_name = Some("lzma.LZMAError");
                }
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Error raising & conversion helpers ───────────────────────────────────────

fn raise_lzma_error(error_cls: &Value, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let ValueKind::PyClass(cls_rc) = error_cls.kind() {
        return PyError::raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::runtime(msg)
}

fn check_type_to_id(ct: CheckType) -> i32 {
    match ct {
        CheckType::None => CHECK_NONE,
        CheckType::Crc32 => CHECK_CRC32,
        CheckType::Crc64 => CHECK_CRC64,
        CheckType::Sha256 => CHECK_SHA256,
    }
}

fn check_id_to_type(check_id: i32, error_cls: &Value) -> Result<CheckType> {
    match check_id {
        CHECK_NONE => Ok(CheckType::None),
        CHECK_CRC32 => Ok(CheckType::Crc32),
        CHECK_CRC64 => Ok(CheckType::Crc64),
        CHECK_SHA256 => Ok(CheckType::Sha256),
        _ => Err(raise_lzma_error(
            error_cls,
            format!("Unsupported integrity check: {check_id}"),
        )),
    }
}

fn preset_to_lzma_options(preset: u32, error_cls: &Value) -> Result<LzmaOptions> {
    let extreme = (preset & PRESET_EXTREME) != 0;
    let level = preset & !PRESET_EXTREME;
    if level > 9 {
        return Err(raise_lzma_error(
            error_cls,
            format!("Invalid compression preset: {preset}"),
        ));
    }
    let mut opts = LzmaOptions::with_preset(level);
    if extreme {
        opts.mode = EncodeMode::Normal;
        opts.mf = MfType::Bt4;
        opts.nice_len = LzmaOptions::NICE_LEN_MAX;
        opts.depth_limit = 0;
    }
    Ok(opts)
}

// ── Shared output buffer sink for incremental writers ────────────────────────

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(Vec::new())))
    }

    fn drain(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

impl IoWrite for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ── Parsed filter representation ─────────────────────────────────────────────

#[derive(Clone, Debug)]
enum ParsedFilter {
    Lzma1(LzmaOptions),
    Lzma2(LzmaOptions),
    Pre(FilterConfig),
}

fn parse_filter_dict(
    interp: &mut Interpreter,
    val: &Value,
    error_cls: &Value,
) -> Result<ParsedFilter> {
    let Some(dict) = val.as_dict() else {
        return Err(PyError::named(
            "TypeError",
            "Filter specifier must be a dict".to_string(),
        ));
    };
    let Some(id_val) = dict.get(&PyKey::str_from("id")) else {
        return Err(PyError::named(
            "ValueError",
            "Filter specifier must have an \"id\" entry".to_string(),
        ));
    };
    let filter_id = extract_u64(interp, id_val)?;
    match filter_id {
        FILTER_LZMA1 | FILTER_LZMA2 => {
            let preset = if let Some(pv) = dict.get(&PyKey::str_from("preset")) {
                if matches!(pv.kind(), ValueKind::None) {
                    PRESET_DEFAULT
                } else {
                    extract_u32(interp, pv)?
                }
            } else {
                PRESET_DEFAULT
            };
            let mut opts = preset_to_lzma_options(preset, error_cls)?;
            if let Some(v) = dict.get(&PyKey::str_from("dict_size")) {
                let ds = extract_u32(interp, v)?;
                if !(DICT_SIZE_MIN..=DICT_SIZE_MAX).contains(&ds) {
                    return Err(raise_lzma_error(
                        error_cls,
                        "Invalid dictionary size".to_string(),
                    ));
                }
                opts.dict_size = ds;
            }
            if let Some(v) = dict.get(&PyKey::str_from("lc")) {
                let lc = extract_u32(interp, v)?;
                if lc > 4 {
                    return Err(raise_lzma_error(error_cls, "Invalid lc value".to_string()));
                }
                opts.lc = lc;
            }
            if let Some(v) = dict.get(&PyKey::str_from("lp")) {
                let lp = extract_u32(interp, v)?;
                if lp > 4 {
                    return Err(raise_lzma_error(error_cls, "Invalid lp value".to_string()));
                }
                opts.lp = lp;
            }
            if opts.lc + opts.lp > 4 {
                return Err(raise_lzma_error(
                    error_cls,
                    "lc + lp must not exceed 4".to_string(),
                ));
            }
            if let Some(v) = dict.get(&PyKey::str_from("pb")) {
                let pb = extract_u32(interp, v)?;
                if pb > 4 {
                    return Err(raise_lzma_error(error_cls, "Invalid pb value".to_string()));
                }
                opts.pb = pb;
            }
            if let Some(v) = dict.get(&PyKey::str_from("mode")) {
                let mode = extract_i64(interp, v)?;
                opts.mode = match mode {
                    MODE_FAST => EncodeMode::Fast,
                    MODE_NORMAL => EncodeMode::Normal,
                    _ => {
                        return Err(raise_lzma_error(
                            error_cls,
                            "Invalid compression mode".to_string(),
                        ));
                    }
                };
            }
            if let Some(v) = dict.get(&PyKey::str_from("nice_len")) {
                let nl = extract_u32(interp, v)?;
                if !(2..=LzmaOptions::NICE_LEN_MAX).contains(&nl) {
                    return Err(raise_lzma_error(
                        error_cls,
                        "Invalid nice_len value".to_string(),
                    ));
                }
                opts.nice_len = nl.max(LzmaOptions::NICE_LEN_MIN);
            }
            if let Some(v) = dict.get(&PyKey::str_from("mf")) {
                let mf = extract_i64(interp, v)?;
                opts.mf = match mf {
                    MF_HC3 | MF_HC4 => MfType::Hc4,
                    MF_BT2 | MF_BT3 | MF_BT4 => MfType::Bt4,
                    _ => {
                        return Err(raise_lzma_error(
                            error_cls,
                            "Invalid match finder".to_string(),
                        ));
                    }
                };
            }
            if let Some(v) = dict.get(&PyKey::str_from("depth")) {
                let depth = extract_c_int(interp, v)?;
                if depth < 0 {
                    return Err(raise_lzma_error(
                        error_cls,
                        "Invalid depth value".to_string(),
                    ));
                }
                opts.depth_limit = depth;
            }
            if filter_id == FILTER_LZMA1 {
                Ok(ParsedFilter::Lzma1(opts))
            } else {
                Ok(ParsedFilter::Lzma2(opts))
            }
        }
        FILTER_DELTA => {
            let dist = if let Some(v) = dict.get(&PyKey::str_from("dist")) {
                extract_u32(interp, v)?
            } else {
                1
            };
            if !(1..=256).contains(&dist) {
                return Err(raise_lzma_error(
                    error_cls,
                    "Invalid delta distance".to_string(),
                ));
            }
            Ok(ParsedFilter::Pre(FilterConfig::new_delta(dist)))
        }
        FILTER_X86 | FILTER_POWERPC | FILTER_IA64 | FILTER_ARM | FILTER_ARMTHUMB | FILTER_SPARC => {
            let start_offset = if let Some(v) = dict.get(&PyKey::str_from("start_offset")) {
                extract_u32(interp, v)?
            } else {
                0
            };
            let cfg = match filter_id {
                FILTER_X86 => FilterConfig::new_bcj_x86(start_offset),
                FILTER_POWERPC => FilterConfig::new_bcj_ppc(start_offset),
                FILTER_IA64 => FilterConfig::new_bcj_ia64(start_offset),
                FILTER_ARM => FilterConfig::new_bcj_arm(start_offset),
                FILTER_ARMTHUMB => FilterConfig::new_bcj_arm_thumb(start_offset),
                FILTER_SPARC => FilterConfig::new_bcj_sparc(start_offset),
                _ => unreachable!(),
            };
            Ok(ParsedFilter::Pre(cfg))
        }
        _ => Err(PyError::named(
            "ValueError",
            format!("Invalid filter ID: {filter_id}"),
        )),
    }
}

fn parse_filter_chain(
    interp: &mut Interpreter,
    val: &Value,
    error_cls: &Value,
) -> Result<Vec<ParsedFilter>> {
    let items: Vec<Value> = match val.kind() {
        ValueKind::List(r) => r.clone(),
        ValueKind::Tuple(rc) => rc.as_ref().to_vec(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "filters must be a sequence of filter specifiers".to_string(),
            ));
        }
    };
    if items.is_empty() {
        return Err(PyError::named(
            "ValueError",
            "Filter chain cannot be empty".to_string(),
        ));
    }
    if items.len() > 4 {
        return Err(PyError::named(
            "ValueError",
            "liblzma supports at most 4 filters in a chain".to_string(),
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        out.push(parse_filter_dict(interp, item, error_cls)?);
    }
    Ok(out)
}

// ── Raw filter chain writer for FORMAT_RAW ───────────────────────────────────

enum RawFilterWriter {
    Lzma1(Box<LzmaWriter<SharedBuf>>),
    Lzma2(Box<Lzma2Writer<SharedBuf>>),
    Delta(Box<DeltaWriter<Box<RawFilterWriter>>>),
    Bcj(Box<BcjWriter<Box<RawFilterWriter>>>),
}

impl IoWrite for RawFilterWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Lzma1(w) => w.write(buf),
            Self::Lzma2(w) => w.write(buf),
            Self::Delta(w) => w.write(buf),
            Self::Bcj(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Lzma1(w) => w.flush(),
            Self::Lzma2(w) => w.flush(),
            Self::Delta(w) => w.flush(),
            Self::Bcj(w) => w.flush(),
        }
    }
}

impl RawFilterWriter {
    fn finish(self) -> io::Result<()> {
        match self {
            Self::Lzma1(w) => {
                w.finish()?;
                Ok(())
            }
            Self::Lzma2(w) => {
                w.finish()?;
                Ok(())
            }
            Self::Delta(w) => w.into_inner().finish(),
            Self::Bcj(w) => {
                let inner = w.finish()?;
                inner.finish()
            }
        }
    }

    fn build(sink: SharedBuf, filters: &[ParsedFilter], error_cls: &Value) -> Result<Self> {
        let Some((last, pre)) = filters.split_last() else {
            return Err(PyError::named(
                "ValueError",
                "Filter chain cannot be empty".to_string(),
            ));
        };
        let mut chain = match last {
            ParsedFilter::Lzma1(opts) => {
                let w = LzmaWriter::new_no_header(sink, opts, true)
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Self::Lzma1(Box::new(w))
            }
            ParsedFilter::Lzma2(opts) => {
                let lz2_opts = Lzma2Options {
                    lzma_options: opts.clone(),
                    chunk_size: None,
                };
                Self::Lzma2(Box::new(Lzma2Writer::new(sink, lz2_opts)))
            }
            ParsedFilter::Pre(_) => {
                return Err(raise_lzma_error(
                    error_cls,
                    "Last filter in raw chain must be LZMA1 or LZMA2",
                ));
            }
        };

        for f in pre.iter().rev() {
            let ParsedFilter::Pre(cfg) = f else {
                return Err(raise_lzma_error(
                    error_cls,
                    "LZMA1/LZMA2 must be the last filter in the chain",
                ));
            };
            let prop = cfg.property as usize;
            chain = match cfg.filter_type {
                FilterType::Delta => Self::Delta(Box::new(DeltaWriter::new(Box::new(chain), prop))),
                FilterType::BcjX86 => {
                    Self::Bcj(Box::new(BcjWriter::new_x86(Box::new(chain), prop)))
                }
                FilterType::BcjPpc => {
                    Self::Bcj(Box::new(BcjWriter::new_ppc(Box::new(chain), prop)))
                }
                FilterType::BcjIa64 => {
                    Self::Bcj(Box::new(BcjWriter::new_ia64(Box::new(chain), prop)))
                }
                FilterType::BcjArm => {
                    Self::Bcj(Box::new(BcjWriter::new_arm(Box::new(chain), prop)))
                }
                FilterType::BcjArmThumb => {
                    Self::Bcj(Box::new(BcjWriter::new_arm_thumb(Box::new(chain), prop)))
                }
                FilterType::BcjSparc => {
                    Self::Bcj(Box::new(BcjWriter::new_sparc(Box::new(chain), prop)))
                }
                _ => {
                    return Err(raise_lzma_error(
                        error_cls,
                        "Unsupported pre-filter in raw chain",
                    ));
                }
            };
        }
        Ok(chain)
    }
}

// ── Opaque native compressor state ───────────────────────────────────────────

enum CompressorBackend {
    Xz(Box<XzWriter<SharedBuf>>),
    Alone(Box<LzmaWriter<SharedBuf>>),
    Raw(Box<RawFilterWriter>),
    Finished,
}

struct LzmaCompressorState {
    backend: CompressorBackend,
    sink: SharedBuf,
}

impl LzmaCompressorState {
    fn new(
        format: i32,
        check: i32,
        preset: Option<u32>,
        filters: Option<Vec<ParsedFilter>>,
        error_cls: &Value,
    ) -> Result<Self> {
        let sink = SharedBuf::new();
        match format {
            FORMAT_XZ => {
                if preset.is_some() && filters.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify both preset and filter chain".to_string(),
                    ));
                }
                let check_type = if check == -1 {
                    CheckType::Crc64
                } else {
                    check_id_to_type(check, error_cls)?
                };
                let mut xz_opts = if let Some(chain) = filters {
                    let Some((last, pre)) = chain.split_last() else {
                        return Err(PyError::named(
                            "ValueError",
                            "Filter chain cannot be empty".to_string(),
                        ));
                    };
                    let ParsedFilter::Lzma2(lzma_opts) = last else {
                        return Err(raise_lzma_error(
                            error_cls,
                            "XZ format requires FILTER_LZMA2 as the last filter",
                        ));
                    };
                    let mut pre_cfgs = Vec::with_capacity(pre.len());
                    for f in pre {
                        match f {
                            ParsedFilter::Pre(cfg) => pre_cfgs.push(cfg.clone()),
                            _ => {
                                return Err(raise_lzma_error(
                                    error_cls,
                                    "Unsupported filter chain for XZ format",
                                ));
                            }
                        }
                    }
                    XzOptions {
                        lzma_options: lzma_opts.clone(),
                        check_type,
                        block_size: None,
                        filters: pre_cfgs,
                    }
                } else {
                    let p = preset.unwrap_or(PRESET_DEFAULT);
                    let lzma_opts = preset_to_lzma_options(p, error_cls)?;
                    XzOptions {
                        lzma_options: lzma_opts,
                        check_type,
                        block_size: None,
                        filters: Vec::new(),
                    }
                };
                xz_opts.check_type = check_type;
                let writer = XzWriter::new(sink.clone(), xz_opts)
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(Self {
                    backend: CompressorBackend::Xz(Box::new(writer)),
                    sink,
                })
            }
            FORMAT_ALONE => {
                if check != -1 && check != CHECK_NONE {
                    return Err(PyError::named(
                        "ValueError",
                        "Integrity checks are only supported by FORMAT_XZ".to_string(),
                    ));
                }
                if preset.is_some() && filters.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify both preset and filter chain".to_string(),
                    ));
                }
                let lzma_opts = if let Some(chain) = filters {
                    if chain.len() != 1 {
                        return Err(PyError::named(
                            "ValueError",
                            "FORMAT_ALONE supports only a single FILTER_LZMA1 filter".to_string(),
                        ));
                    }
                    match &chain[0] {
                        ParsedFilter::Lzma1(opts) => opts.clone(),
                        _ => {
                            return Err(PyError::named(
                                "ValueError",
                                "FORMAT_ALONE requires FILTER_LZMA1".to_string(),
                            ));
                        }
                    }
                } else {
                    preset_to_lzma_options(preset.unwrap_or(PRESET_DEFAULT), error_cls)?
                };
                let writer = LzmaWriter::new_use_header(sink.clone(), &lzma_opts, None)
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(Self {
                    backend: CompressorBackend::Alone(Box::new(writer)),
                    sink,
                })
            }
            FORMAT_RAW => {
                if check != -1 && check != CHECK_NONE {
                    return Err(PyError::named(
                        "ValueError",
                        "Integrity checks are only supported by FORMAT_XZ".to_string(),
                    ));
                }
                if preset.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify preset with FORMAT_RAW".to_string(),
                    ));
                }
                let Some(chain) = filters else {
                    return Err(PyError::named(
                        "ValueError",
                        "Must specify filters for FORMAT_RAW".to_string(),
                    ));
                };
                let writer = RawFilterWriter::build(sink.clone(), &chain, error_cls)?;
                Ok(Self {
                    backend: CompressorBackend::Raw(Box::new(writer)),
                    sink,
                })
            }
            _ => Err(PyError::named(
                "ValueError",
                format!("Invalid container format: {format}"),
            )),
        }
    }

    fn compress(&mut self, data: &[u8], error_cls: &Value) -> Result<Vec<u8>> {
        match &mut self.backend {
            CompressorBackend::Finished => Err(PyError::named(
                "ValueError",
                "Compressor has been flushed".to_string(),
            )),
            CompressorBackend::Xz(w) => {
                w.write_all(data)
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(self.sink.drain())
            }
            CompressorBackend::Alone(w) => {
                w.write_all(data)
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(self.sink.drain())
            }
            CompressorBackend::Raw(w) => {
                w.write_all(data)
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(self.sink.drain())
            }
        }
    }

    fn flush(&mut self, error_cls: &Value) -> Result<Vec<u8>> {
        let prev = std::mem::replace(&mut self.backend, CompressorBackend::Finished);
        match prev {
            CompressorBackend::Finished => Err(PyError::named(
                "ValueError",
                "Compressor has been flushed".to_string(),
            )),
            CompressorBackend::Xz(w) => {
                w.finish()
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(self.sink.drain())
            }
            CompressorBackend::Alone(w) => {
                w.finish()
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(self.sink.drain())
            }
            CompressorBackend::Raw(w) => {
                w.finish()
                    .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                Ok(self.sink.drain())
            }
        }
    }
}

// ── Opaque native decompressor state ─────────────────────────────────────────

enum DecompressorBackend {
    Auto { mem_limit_kb: u32 },
    Xz(Box<XzStream>),
    Alone(Box<LzmaStream>),
    RawLzma1(Box<LzmaStream>),
    RawLzma2(Box<Lzma2Stream>),
}

struct LzmaDecompressorState {
    backend: DecompressorBackend,
    pending_in: Vec<u8>,
    pending_pos: usize,
    alone_hdr: [u8; 13],
    alone_hdr_len: usize,
    check: i32,
    eof: bool,
    needs_input: bool,
    unused_data: Vec<u8>,
}

fn validate_alone_header_prefix(hdr: &[u8], error_cls: &Value) -> Result<()> {
    if hdr.is_empty() {
        return Ok(());
    }
    if hdr[0] >= 225 {
        return Err(raise_lzma_error(
            error_cls,
            "Input format not supported by decoder",
        ));
    }
    if hdr.len() >= 5 {
        let dict_size = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]);
        if dict_size != u32::MAX {
            let mut d = dict_size.wrapping_sub(1);
            d |= d >> 2;
            d |= d >> 3;
            d |= d >> 4;
            d |= d >> 8;
            d |= d >> 16;
            if d == u32::MAX || d.wrapping_add(1) != dict_size {
                return Err(raise_lzma_error(
                    error_cls,
                    "Input format not supported by decoder",
                ));
            }
        }
    }
    if hdr.len() >= 13 {
        let uncomp_size = u64::from_le_bytes([
            hdr[5], hdr[6], hdr[7], hdr[8], hdr[9], hdr[10], hdr[11], hdr[12],
        ]);
        if uncomp_size != u64::MAX && uncomp_size >= (1u64 << 38) {
            return Err(raise_lzma_error(
                error_cls,
                "Input format not supported by decoder",
            ));
        }
    }
    Ok(())
}

fn memlimit_bytes_to_kb(memlimit: Option<u64>) -> u32 {
    match memlimit {
        None => u32::MAX,
        Some(bytes) => {
            let kb = bytes / 1024;
            u32::try_from(kb).unwrap_or(u32::MAX)
        }
    }
}

impl LzmaDecompressorState {
    fn new(
        format: i32,
        memlimit: Option<u64>,
        filters: Option<Vec<ParsedFilter>>,
        error_cls: &Value,
    ) -> Result<Self> {
        let mem_limit_kb = memlimit_bytes_to_kb(memlimit);
        match format {
            FORMAT_AUTO => {
                if filters.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify filters except with FORMAT_RAW".to_string(),
                    ));
                }
                Ok(Self {
                    backend: DecompressorBackend::Auto { mem_limit_kb },
                    pending_in: Vec::new(),
                    pending_pos: 0,
                    alone_hdr: [0u8; 13],
                    alone_hdr_len: 0,
                    check: CHECK_UNKNOWN,
                    eof: false,
                    needs_input: true,
                    unused_data: Vec::new(),
                })
            }
            FORMAT_XZ => {
                if filters.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify filters except with FORMAT_RAW".to_string(),
                    ));
                }
                Ok(Self {
                    backend: DecompressorBackend::Xz(Box::new(XzStream::new_mem_limit(
                        false,
                        mem_limit_kb,
                    ))),
                    pending_in: Vec::new(),
                    pending_pos: 0,
                    alone_hdr: [0u8; 13],
                    alone_hdr_len: 0,
                    check: CHECK_UNKNOWN,
                    eof: false,
                    needs_input: true,
                    unused_data: Vec::new(),
                })
            }
            FORMAT_ALONE => {
                if filters.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify filters except with FORMAT_RAW".to_string(),
                    ));
                }
                Ok(Self {
                    backend: DecompressorBackend::Alone(Box::new(LzmaStream::new_mem_limit(
                        mem_limit_kb,
                        None,
                    ))),
                    pending_in: Vec::new(),
                    pending_pos: 0,
                    alone_hdr: [0u8; 13],
                    alone_hdr_len: 0,
                    check: CHECK_NONE,
                    eof: false,
                    needs_input: true,
                    unused_data: Vec::new(),
                })
            }
            FORMAT_RAW => {
                if memlimit.is_some() {
                    return Err(PyError::named(
                        "ValueError",
                        "Cannot specify memory limit with FORMAT_RAW".to_string(),
                    ));
                }
                let Some(chain) = filters else {
                    return Err(PyError::named(
                        "ValueError",
                        "Must specify filters for FORMAT_RAW".to_string(),
                    ));
                };
                let Some((last, pre)) = chain.split_last() else {
                    return Err(PyError::named(
                        "ValueError",
                        "Filter chain cannot be empty".to_string(),
                    ));
                };
                let mut pre_cfgs = Vec::with_capacity(pre.len());
                for f in pre {
                    match f {
                        ParsedFilter::Pre(cfg) => pre_cfgs.push(cfg.clone()),
                        _ => {
                            return Err(raise_lzma_error(
                                error_cls,
                                "LZMA1/LZMA2 must be the last filter in the chain",
                            ));
                        }
                    }
                }
                let backend = match last {
                    ParsedFilter::Lzma1(opts) => {
                        let mut stream = LzmaStream::new(
                            u64::MAX,
                            opts.lc,
                            opts.lp,
                            opts.pb,
                            opts.dict_size,
                            None,
                        )
                        .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                        if !pre_cfgs.is_empty() {
                            stream
                                .set_filters(&pre_cfgs)
                                .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                        }
                        DecompressorBackend::RawLzma1(Box::new(stream))
                    }
                    ParsedFilter::Lzma2(opts) => {
                        let mut stream = Lzma2Stream::new_mem_limit(opts.dict_size, u32::MAX);
                        if !pre_cfgs.is_empty() {
                            stream
                                .set_filters(&pre_cfgs)
                                .map_err(|e| raise_lzma_error(error_cls, e.to_string()))?;
                        }
                        DecompressorBackend::RawLzma2(Box::new(stream))
                    }
                    ParsedFilter::Pre(_) => {
                        return Err(raise_lzma_error(
                            error_cls,
                            "Last filter in raw chain must be LZMA1 or LZMA2",
                        ));
                    }
                };
                Ok(Self {
                    backend,
                    pending_in: Vec::new(),
                    pending_pos: 0,
                    alone_hdr: [0u8; 13],
                    alone_hdr_len: 0,
                    check: CHECK_NONE,
                    eof: false,
                    needs_input: true,
                    unused_data: Vec::new(),
                })
            }
            _ => Err(PyError::named(
                "ValueError",
                format!("Invalid container format: {format}"),
            )),
        }
    }

    fn compact_pending(&mut self) {
        if self.pending_pos == 0 {
            return;
        }
        if self.pending_pos >= self.pending_in.len() {
            self.pending_in.clear();
            self.pending_pos = 0;
        } else if self.pending_pos >= 4096 {
            self.pending_in.drain(..self.pending_pos);
            self.pending_pos = 0;
        }
    }

    fn resolve_auto_format(&mut self) -> bool {
        let DecompressorBackend::Auto { mem_limit_kb } = self.backend else {
            return true;
        };
        let avail = &self.pending_in[self.pending_pos..];
        if avail.is_empty() {
            return false;
        }
        let check_len = avail.len().min(XZ_MAGIC.len());
        if avail[..check_len] != XZ_MAGIC[..check_len] {
            // Cannot be XZ; fall back to FORMAT_ALONE (.lzma).
            self.check = CHECK_NONE;
            self.backend =
                DecompressorBackend::Alone(Box::new(LzmaStream::new_mem_limit(mem_limit_kb, None)));
            return true;
        }
        if avail.len() < XZ_MAGIC.len() {
            // Prefix matches XZ magic so far, need more bytes to confirm.
            return false;
        }
        self.backend =
            DecompressorBackend::Xz(Box::new(XzStream::new_mem_limit(false, mem_limit_kb)));
        true
    }

    fn decompress(&mut self, data: &[u8], max_length: i64, error_cls: &Value) -> Result<Vec<u8>> {
        if self.eof {
            return Err(PyError::named(
                "EOFError",
                "Already at end of stream".to_string(),
            ));
        }

        if !data.is_empty() {
            self.compact_pending();
            self.pending_in.extend_from_slice(data);
        }

        if max_length == 0 {
            if self.pending_pos < self.pending_in.len() {
                self.needs_input = false;
            }
            return Ok(Vec::new());
        }

        let was_auto = matches!(self.backend, DecompressorBackend::Auto { .. });
        if !self.resolve_auto_format() {
            self.needs_input = true;
            return Ok(Vec::new());
        }

        if matches!(self.backend, DecompressorBackend::Alone(_)) && self.alone_hdr_len < 13 {
            let new_hdr_bytes = if was_auto {
                &self.pending_in[self.pending_pos..]
            } else {
                data
            };
            let take = (13 - self.alone_hdr_len).min(new_hdr_bytes.len());
            if take > 0 {
                self.alone_hdr[self.alone_hdr_len..self.alone_hdr_len + take]
                    .copy_from_slice(&new_hdr_bytes[..take]);
                self.alone_hdr_len += take;
                validate_alone_header_prefix(&self.alone_hdr[..self.alone_hdr_len], error_cls)?;
            }
        }

        let limit: Option<usize> = if max_length < 0 {
            None
        } else {
            Some(max_length as usize)
        };

        let mut out = Vec::new();
        let mut scratch = [0u8; 16384];

        loop {
            let step = match limit {
                Some(lim) => {
                    let rem = lim.saturating_sub(out.len());
                    if rem == 0 {
                        self.needs_input = false;
                        self.compact_pending();
                        return Ok(out);
                    }
                    rem.min(scratch.len())
                }
                None => scratch.len(),
            };

            let input_slice = &self.pending_in[self.pending_pos..];
            let res = match &mut self.backend {
                DecompressorBackend::Auto { .. } => unreachable!(),
                DecompressorBackend::Xz(s) => {
                    let r = s.process(input_slice, &mut scratch[..step], LzAction::Run);
                    if let Some(ct) = s.check_type() {
                        self.check = check_type_to_id(ct);
                    }
                    r
                }
                DecompressorBackend::Alone(s) | DecompressorBackend::RawLzma1(s) => {
                    s.process(input_slice, &mut scratch[..step], LzAction::Run)
                }
                DecompressorBackend::RawLzma2(s) => {
                    s.process(input_slice, &mut scratch[..step], LzAction::Run)
                }
            };

            let sr = res.map_err(|e| {
                let msg = if e.kind() == io::ErrorKind::OutOfMemory {
                    "Memory usage limit exceeded".to_string()
                } else {
                    format!("Corrupt input data: {e}")
                };
                raise_lzma_error(error_cls, msg)
            })?;

            self.pending_pos += sr.bytes_consumed;
            if sr.bytes_produced > 0 {
                out.extend_from_slice(&scratch[..sr.bytes_produced]);
            }

            if sr.status == LzStatus::StreamEnd {
                self.eof = true;
                self.needs_input = false;
                let mut unused = Vec::new();
                if let DecompressorBackend::Alone(s) | DecompressorBackend::RawLzma1(s) =
                    &self.backend
                {
                    unused.extend_from_slice(s.unused_input());
                }
                unused.extend_from_slice(&self.pending_in[self.pending_pos..]);
                self.unused_data = unused;
                self.pending_in.clear();
                self.pending_pos = 0;
                return Ok(out);
            }

            if sr.bytes_consumed == 0 && sr.bytes_produced == 0 {
                self.needs_input = true;
                self.compact_pending();
                return Ok(out);
            }
        }
    }
}

struct LzmaCompressorOps;
const LZMA_COMPRESSOR_OPS: &LzmaCompressorOps = &LzmaCompressorOps;

impl BuiltinTypeOps for LzmaCompressorOps {
    fn type_name(&self) -> &'static str {
        "_lzma_compressor"
    }
}

struct LzmaDecompressorOps;
const LZMA_DECOMPRESSOR_OPS: &LzmaDecompressorOps = &LzmaDecompressorOps;

impl BuiltinTypeOps for LzmaDecompressorOps {
    fn type_name(&self) -> &'static str {
        "_lzma_decompressor"
    }
}

fn wrap_compressor(state: LzmaCompressorState) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(LZMA_COMPRESSOR_OPS, boxed)
}

fn wrap_decompressor(state: LzmaDecompressorState) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(LZMA_DECOMPRESSOR_OPS, boxed)
}

fn with_compressor_mut<R>(
    val: &Value,
    action: impl FnOnce(&mut LzmaCompressorState) -> R,
) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid lzma compressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<LzmaCompressorOps>(ops) {
        return Err(PyError::runtime("invalid lzma compressor state".into()));
    }
    let mut borrow = state.borrow_mut();
    let c = borrow
        .downcast_mut::<LzmaCompressorState>()
        .ok_or_else(|| PyError::runtime("invalid lzma compressor state".into()))?;
    Ok(action(c))
}

fn with_decompressor_mut<R>(
    val: &Value,
    action: impl FnOnce(&mut LzmaDecompressorState) -> R,
) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::runtime("invalid lzma decompressor state".into()));
    };
    if !pyrust_core::builtin_ops_is::<LzmaDecompressorOps>(ops) {
        return Err(PyError::runtime("invalid lzma decompressor state".into()));
    }
    let mut borrow = state.borrow_mut();
    let d = borrow
        .downcast_mut::<LzmaDecompressorState>()
        .ok_or_else(|| PyError::runtime("invalid lzma decompressor state".into()))?;
    Ok(action(d))
}

// ── Buffer & integer extraction helpers ──────────────────────────────────────

fn try_extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = val.kind() {
        return Some(rc.as_ref().clone());
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        return Some(snap);
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_bytes_checked(val).ok();
    }
    if let Some(backing) = builtin_data_backing(val) {
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Some(rc.as_ref().clone());
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
            return Some(snap);
        }
    }
    if let ValueKind::PyInstance(inst_rc) = val.kind() {
        let inst = inst_rc.borrow();
        let is_array = {
            let cls = inst.class.borrow();
            cls.name == "array"
                && cls
                    .attrs
                    .get("__module__")
                    .and_then(|v| v.as_str())
                    .is_some_and(|m| m == "array")
        };
        if is_array
            && let Some(buf) = inst.attrs.get("_buf")
            && let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(buf)
        {
            return Some(snap);
        }
    }
    None
}

fn extract_bytes_like(val: &Value) -> Result<Vec<u8>> {
    try_extract_bytes_like(val).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(val)
            ),
        )
    })
}

fn resolve_index_value(interp: &mut Interpreter, val: &Value) -> Result<Value> {
    match val.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => Ok(val.clone()),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
            if let Some(indexed) = interp.try_value_to_index(val)? {
                Ok(indexed)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(val)
                    ),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_c_int(interp: &mut Interpreter, val: &Value) -> Result<i32> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(i32::from(b)),
        ValueKind::Int(n) => i32::try_from(n).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        }),
        ValueKind::BigInt(bi) => bi.to_i32().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_i64(interp: &mut Interpreter, val: &Value) -> Result<i64> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(i64::from(b)),
        ValueKind::Int(n) => Ok(n),
        ValueKind::BigInt(bi) => bi.to_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_u32(interp: &mut Interpreter, val: &Value) -> Result<u32> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(u32::from(b)),
        ValueKind::Int(n) => u32::try_from(n).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int out of range for unsigned 32-bit integer".to_string(),
            )
        }),
        ValueKind::BigInt(bi) => bi.to_u32().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int out of range for unsigned 32-bit integer".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_u64(interp: &mut Interpreter, val: &Value) -> Result<u64> {
    let idx = resolve_index_value(interp, val)?;
    match idx.kind() {
        ValueKind::Bool(b) => Ok(u64::from(b)),
        ValueKind::Int(n) => u64::try_from(n).map_err(|_| {
            PyError::named(
                "OverflowError",
                "Python int out of range for unsigned 64-bit integer".to_string(),
            )
        }),
        ValueKind::BigInt(bi) => bi.to_u64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int out of range for unsigned 64-bit integer".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

pyrust_module! {
    fn _is_check_supported(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_is_check_supported expects 1 arg".into()));
        }
        let check_id = extract_c_int(_interp, &args[0].value)?;
        let supported = matches!(
            check_id,
            CHECK_NONE | CHECK_CRC32 | CHECK_CRC64 | CHECK_SHA256
        );
        Ok(Value::bool_(supported))
    }

    fn _compressor_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 5 {
            return Err(PyError::runtime("_compressor_new expects 5 args".into()));
        }
        let error_cls = &args[0].value;
        let format = extract_c_int(_interp, &args[1].value)?;
        let check = extract_c_int(_interp, &args[2].value)?;
        let preset = if matches!(args[3].value.kind(), ValueKind::None) {
            None
        } else {
            Some(extract_u32(_interp, &args[3].value)?)
        };
        let filters = if matches!(args[4].value.kind(), ValueKind::None) {
            None
        } else {
            Some(parse_filter_chain(_interp, &args[4].value, error_cls)?)
        };
        let state = LzmaCompressorState::new(format, check, preset, filters, error_cls)?;
        Ok(wrap_compressor(state))
    }

    fn _compressor_compress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::runtime("_compressor_compress expects 3 args".into()));
        }
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[2].value)?;
        let out = with_compressor_mut(&args[1].value, |c| c.compress(&data, error_cls))??;
        Ok(Value::bytes(out))
    }

    fn _compressor_flush(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::runtime("_compressor_flush expects 2 args".into()));
        }
        let error_cls = &args[0].value;
        let out = with_compressor_mut(&args[1].value, |c| c.flush(error_cls))??;
        Ok(Value::bytes(out))
    }

    fn _decompressor_new(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 4 {
            return Err(PyError::runtime("_decompressor_new expects 4 args".into()));
        }
        let error_cls = &args[0].value;
        let format = extract_c_int(_interp, &args[1].value)?;
        let memlimit = if matches!(args[2].value.kind(), ValueKind::None) {
            None
        } else {
            Some(extract_u64(_interp, &args[2].value)?)
        };
        let filters = if matches!(args[3].value.kind(), ValueKind::None) {
            None
        } else {
            Some(parse_filter_chain(_interp, &args[3].value, error_cls)?)
        };
        let state = LzmaDecompressorState::new(format, memlimit, filters, error_cls)?;
        let init_check = state.check;
        Ok(Value::tuple(vec![
            wrap_decompressor(state),
            Value::int(i64::from(init_check)),
        ]))
    }

    fn _decompressor_decompress(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 4 {
            return Err(PyError::runtime("_decompressor_decompress expects 4 args".into()));
        }
        let error_cls = &args[0].value;
        let data = extract_bytes_like(&args[2].value)?;
        let max_length = extract_i64(_interp, &args[3].value)?;
        let result = with_decompressor_mut(&args[1].value, |d| {
            let out = d.decompress(&data, max_length, error_cls)?;
            Ok::<Value, PyError>(Value::tuple(vec![
                Value::bytes(out),
                Value::int(i64::from(d.check)),
                Value::bool_(d.eof),
                Value::bool_(d.needs_input),
                Value::bytes(d.unused_data.clone()),
            ]))
        })??;
        Ok(result)
    }
}

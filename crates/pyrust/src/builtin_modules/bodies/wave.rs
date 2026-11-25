// `wave` module — 100% native Rust implementation of Python 3.12's `wave`
// RIFF/WAVE PCM reader and writer (issue #3259).
//
// Provides `Error`, `_wave_params` (`pyrust_struct_seq!`), `Wave_read`,
// `Wave_write`, and `open` with byte-for-byte CPython 3.12 RIFF/WAVE header
// and frame parity for filenames, `os.PathLike`, and file-like objects
// (including `io.BytesIO`).
//
// Reference: <https://docs.python.org/3/library/wave.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBytesLike, PyIndex, PyNumber, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{Value, ValueKind};

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

pyrust_struct_seq! {
    pub(crate) _wave_params in "wave" {
        fields: [nchannels, sampwidth, framerate, nframes, comptype, compname],
    }
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn raise_wave_error(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(wave_mod) = interp.load_module("wave")
        && let Ok(err_cls) = interp.get_attr(&wave_mod, "Error")
        && let Ok(inst) = call_pos(interp, err_cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named("Exception", msg)
}

fn is_path_arg(interp: &mut Interpreter, obj: &Value) -> bool {
    matches!(obj.kind(), ValueKind::Str(_) | ValueKind::Bytes(_))
        || interp.get_attr(obj, "__fspath__").is_ok()
}

fn open_if_path(interp: &mut Interpreter, f: Value, mode: &str) -> Result<(Value, Option<Value>)> {
    if is_path_arg(interp, &f) {
        let opened = call_pos(
            interp,
            Value::builtin_function("open"),
            &[f, Value::string(mode)],
        )?;
        Ok((opened.clone(), Some(opened)))
    } else {
        Ok((f, None))
    }
}

fn fp_read(interp: &mut Interpreter, fp: &Value, n: usize) -> Result<Vec<u8>> {
    let val = call_method(interp, fp, "read", &[Value::int(n as i64)])?;
    let bytes = PyBytesLike::try_from_value_with_interp(interp, &val, "read", "data")?;
    Ok(bytes.as_bytes().to_vec())
}

fn fp_write(interp: &mut Interpreter, fp: &Value, data: &[u8]) -> Result<()> {
    call_method(interp, fp, "write", &[Value::bytes(data.to_vec())])?;
    Ok(())
}

fn fp_tell(interp: &mut Interpreter, fp: &Value) -> Option<u64> {
    let val = call_method(interp, fp, "tell", &[]).ok()?;
    let pos = val.as_int()?;
    u64::try_from(pos).ok()
}

fn fp_seek(interp: &mut Interpreter, fp: &Value, offset: i64, whence: i64) -> Result<()> {
    call_method(
        interp,
        fp,
        "seek",
        &[Value::int(offset), Value::int(whence)],
    )?;
    Ok(())
}

fn fp_skip(interp: &mut Interpreter, fp: &Value, seekable: bool, mut remaining: u64) -> Result<()> {
    if remaining == 0 {
        return Ok(());
    }
    if seekable && fp_seek(interp, fp, remaining as i64, 1).is_ok() {
        return Ok(());
    }
    while remaining > 0 {
        let to_read = remaining.min(8192) as usize;
        let chunk = fp_read(interp, fp, to_read)?;
        if chunk.is_empty() {
            return Err(PyError::named("EOFError", ""));
        }
        remaining -= chunk.len() as u64;
    }
    Ok(())
}

fn byteswap_samples(data: &[u8], width: usize) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    for (src, dst) in data.chunks_exact(width).zip(out.chunks_exact_mut(width)) {
        for j in 0..width {
            dst[width - 1 - j] = src[j];
        }
    }
    out
}

// ── Wave_read State & Header Parser ──────────────────────────────────────────

struct WaveReadState {
    file: Option<Value>,
    i_opened_the_file: Option<Value>,
    nchannels: usize,
    sampwidth: usize,
    framerate: usize,
    nframes: usize,
    framesize: usize,
    comptype: String,
    compname: String,
    soundpos: usize,
    data_start: Option<u64>,
    data_size: u64,
    data_read: u64,
    data_seek_needed: bool,
}

impl WaveReadState {
    fn parse_from_file(
        interp: &mut Interpreter,
        file: Value,
        i_opened: Option<Value>,
    ) -> Result<Self> {
        // Read RIFF container header (8 bytes) + WAVE form ID (4 bytes).
        let riff_id = fp_read(interp, &file, 4)?;
        if riff_id.len() < 4 {
            return Err(PyError::named("EOFError", ""));
        }
        let riff_size_bytes = fp_read(interp, &file, 4)?;
        if riff_size_bytes.len() < 4 {
            return Err(PyError::named("EOFError", ""));
        }
        let riff_size = u32::from_le_bytes(riff_size_bytes.try_into().unwrap()) as u64;
        if riff_id != b"RIFF" {
            return Err(raise_wave_error(interp, "file does not start with RIFF id"));
        }
        let wave_id = fp_read(interp, &file, 4)?;
        if wave_id != b"WAVE" {
            return Err(raise_wave_error(interp, "not a WAVE file"));
        }

        let seekable = fp_tell(interp, &file).is_some();
        let mut riff_read: u64 = 4;

        let mut fmt_read = false;
        let mut nchannels: usize = 0;
        let mut sampwidth: usize = 0;
        let mut framerate: usize = 0;
        let mut framesize: usize = 0;
        let mut data_found = false;
        let mut data_start: Option<u64> = None;
        let mut data_size: u64 = 0;

        while riff_read + 8 <= riff_size {
            let chunk_id = fp_read(interp, &file, 4)?;
            if chunk_id.len() < 4 {
                break;
            }
            let chunk_size_bytes = fp_read(interp, &file, 4)?;
            if chunk_size_bytes.len() < 4 {
                break;
            }
            let chunk_size = u32::from_le_bytes(chunk_size_bytes.try_into().unwrap()) as u64;
            riff_read += 8;

            if chunk_id == b"fmt " {
                if chunk_size < 16 {
                    return Err(PyError::named("EOFError", ""));
                }
                let fmt_hdr = fp_read(interp, &file, 16)?;
                if fmt_hdr.len() < 16 {
                    return Err(PyError::named("EOFError", ""));
                }
                let format_tag = u16::from_le_bytes([fmt_hdr[0], fmt_hdr[1]]);
                let ch = u16::from_le_bytes([fmt_hdr[2], fmt_hdr[3]]) as usize;
                let rate =
                    u32::from_le_bytes([fmt_hdr[4], fmt_hdr[5], fmt_hdr[6], fmt_hdr[7]]) as usize;
                let bits_per_sample = u16::from_le_bytes([fmt_hdr[14], fmt_hdr[15]]) as usize;
                let mut consumed: u64 = 16;

                if format_tag != WAVE_FORMAT_PCM && format_tag != WAVE_FORMAT_EXTENSIBLE {
                    return Err(raise_wave_error(
                        interp,
                        &format!("unknown format: {format_tag}"),
                    ));
                }
                if format_tag == WAVE_FORMAT_EXTENSIBLE {
                    if chunk_size < 40 {
                        return Err(PyError::named("EOFError", ""));
                    }
                    let ext = fp_read(interp, &file, 24)?;
                    if ext.len() < 24 {
                        return Err(PyError::named("EOFError", ""));
                    }
                    consumed += 24;
                    if ext[8..24] != KSDATAFORMAT_SUBTYPE_PCM {
                        return Err(raise_wave_error(interp, "unknown extended format"));
                    }
                }
                let sw = bits_per_sample.div_ceil(8);
                if sw == 0 {
                    return Err(raise_wave_error(interp, "bad sample width"));
                }
                if ch == 0 {
                    return Err(raise_wave_error(interp, "bad # of channels"));
                }
                nchannels = ch;
                sampwidth = sw;
                framerate = rate;
                framesize = nchannels * sampwidth;
                fmt_read = true;

                let pad = chunk_size & 1;
                let leftover = chunk_size.saturating_sub(consumed) + pad;
                fp_skip(interp, &file, seekable, leftover)?;
                riff_read += chunk_size + pad;
            } else if chunk_id == b"data" {
                if !fmt_read {
                    return Err(raise_wave_error(interp, "data chunk before fmt chunk"));
                }
                data_found = true;
                data_start = fp_tell(interp, &file);
                data_size = chunk_size;
                break;
            } else {
                let pad = chunk_size & 1;
                fp_skip(interp, &file, seekable, chunk_size + pad)?;
                riff_read += chunk_size + pad;
            }
        }

        if !fmt_read || !data_found {
            return Err(raise_wave_error(
                interp,
                "fmt chunk and/or data chunk missing",
            ));
        }

        let nframes = (data_size as usize) / framesize;
        Ok(Self {
            file: Some(file),
            i_opened_the_file: i_opened,
            nchannels,
            sampwidth,
            framerate,
            nframes,
            framesize,
            comptype: "NONE".to_string(),
            compname: "not compressed".to_string(),
            soundpos: 0,
            data_start,
            data_size,
            data_read: 0,
            data_seek_needed: false,
        })
    }
}

// ── Wave_write State & Header Writer ─────────────────────────────────────────

struct WaveWriteState {
    file: Option<Value>,
    i_opened_the_file: Option<Value>,
    nchannels: usize,
    sampwidth: usize,
    framerate: usize,
    nframes: usize,
    comptype: Option<String>,
    compname: Option<String>,
    nframeswritten: usize,
    datawritten: u64,
    datalength: u64,
    headerwritten: bool,
    form_length_pos: Option<u64>,
    data_length_pos: Option<u64>,
}

impl WaveWriteState {
    fn new(file: Value, i_opened: Option<Value>) -> Self {
        Self {
            file: Some(file),
            i_opened_the_file: i_opened,
            nchannels: 0,
            sampwidth: 0,
            framerate: 0,
            nframes: 0,
            comptype: Some("NONE".to_string()),
            compname: Some("not compressed".to_string()),
            nframeswritten: 0,
            datawritten: 0,
            datalength: 0,
            headerwritten: false,
            form_length_pos: None,
            data_length_pos: None,
        }
    }

    fn ensure_header_written(&mut self, interp: &mut Interpreter, datasize: usize) -> Result<()> {
        if self.headerwritten {
            return Ok(());
        }
        if self.nchannels == 0 {
            return Err(raise_wave_error(interp, "# channels not specified"));
        }
        if self.sampwidth == 0 {
            return Err(raise_wave_error(interp, "sample width not specified"));
        }
        if self.framerate == 0 {
            return Err(raise_wave_error(interp, "sampling rate not specified"));
        }
        self.write_header(interp, datasize)
    }

    fn write_header(&mut self, interp: &mut Interpreter, initlength: usize) -> Result<()> {
        let Some(ref fp) = self.file else {
            return Err(PyError::named("ValueError", "I/O operation on closed file"));
        };
        fp_write(interp, fp, b"RIFF")?;
        if self.nframes == 0 {
            self.nframes = initlength / (self.nchannels * self.sampwidth);
        }
        self.datalength = (self.nframes as u64) * (self.nchannels as u64) * (self.sampwidth as u64);
        self.form_length_pos = fp_tell(interp, fp);

        let form_len = (36 + self.datalength) as u32;
        let byte_rate = (self.nchannels * self.framerate * self.sampwidth) as u32;
        let block_align = (self.nchannels * self.sampwidth) as u16;
        let bits_per_sample = (self.sampwidth * 8) as u16;

        let mut hdr = Vec::with_capacity(36);
        hdr.extend_from_slice(&form_len.to_le_bytes());
        hdr.extend_from_slice(b"WAVE");
        hdr.extend_from_slice(b"fmt ");
        hdr.extend_from_slice(&16u32.to_le_bytes());
        hdr.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
        hdr.extend_from_slice(&(self.nchannels as u16).to_le_bytes());
        hdr.extend_from_slice(&(self.framerate as u32).to_le_bytes());
        hdr.extend_from_slice(&byte_rate.to_le_bytes());
        hdr.extend_from_slice(&block_align.to_le_bytes());
        hdr.extend_from_slice(&bits_per_sample.to_le_bytes());
        hdr.extend_from_slice(b"data");
        fp_write(interp, fp, &hdr)?;

        if self.form_length_pos.is_some() {
            self.data_length_pos = fp_tell(interp, fp);
        }
        fp_write(interp, fp, &(self.datalength as u32).to_le_bytes())?;
        self.headerwritten = true;
        Ok(())
    }

    fn patch_header(&mut self, interp: &mut Interpreter) -> Result<()> {
        if !self.headerwritten || self.datawritten == self.datalength {
            return Ok(());
        }
        let Some(ref fp) = self.file else {
            return Ok(());
        };
        let (Some(form_pos), Some(data_pos)) = (self.form_length_pos, self.data_length_pos) else {
            return Ok(());
        };
        let curpos = fp_tell(interp, fp)
            .ok_or_else(|| PyError::named("OSError", "cannot tell() on wave output file"))?;
        fp_seek(interp, fp, form_pos as i64, 0)?;
        fp_write(interp, fp, &((36 + self.datawritten) as u32).to_le_bytes())?;
        fp_seek(interp, fp, data_pos as i64, 0)?;
        fp_write(interp, fp, &(self.datawritten as u32).to_le_bytes())?;
        fp_seek(interp, fp, curpos as i64, 0)?;
        self.datalength = self.datawritten;
        Ok(())
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("open"),
            Value::string("Error"),
            Value::string("Wave_read"),
            Value::string("Wave_write"),
        ]),
        "WAVE_FORMAT_PCM" => Value::int(WAVE_FORMAT_PCM as i64),
        "WAVE_FORMAT_EXTENSIBLE" => Value::int(WAVE_FORMAT_EXTENSIBLE as i64),
        "KSDATAFORMAT_SUBTYPE_PCM" => Value::bytes(KSDATAFORMAT_SUBTYPE_PCM.to_vec()),
        "_wave_params" => _wave_params::class(),
    }

    /// Exception raised when a WAVE file violates the RIFF/WAVE specification
    /// or an invalid parameter/operation is attempted.
    exception Error;

    #[native_state(WaveReadState)]
    class Wave_read {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
        ) -> Result<Value> {
            let (fp, opened) = open_if_path(_interp, f.0, "rb")?;
            match WaveReadState::parse_from_file(_interp, fp, opened.clone()) {
                Ok(state) => {
                    set_state(self_val.0, state)?;
                    Ok(Value::none())
                }
                Err(err) => {
                    if let Some(ref own_fp) = opened {
                        let _ = call_method(_interp, own_fp, "close", &[]);
                    }
                    Err(err)
                }
            }
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "close", &[])?;
            Ok(Value::none())
        }

        fn getfp(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(st.file.clone().unwrap_or_else(Value::none))
        }

        fn getnchannels(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::int(st.nchannels as i64))
        }

        fn getsampwidth(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::int(st.sampwidth as i64))
        }

        fn getframerate(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::int(st.framerate as i64))
        }

        fn getnframes(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::int(st.nframes as i64))
        }

        fn getcomptype(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::string(&st.comptype))
        }

        fn getcompname(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::string(&st.compname))
        }

        fn getparams(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(_wave_params::make([
                Value::int(st.nchannels as i64),
                Value::int(st.sampwidth as i64),
                Value::int(st.framerate as i64),
                Value::int(st.nframes as i64),
                Value::string(&st.comptype),
                Value::string(&st.compname),
            ]))
        }

        fn getmarkers(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn getmark(
            #[positional_only] _self: PyValueRef<'_>,
            _id: PyValue,
        ) -> Result<Value> {
            Err(raise_wave_error(_interp, "no marks"))
        }

        fn tell(#[state] st: &WaveReadState) -> Result<Value> {
            Ok(Value::int(st.soundpos as i64))
        }

        fn rewind(#[state_mut] st: &mut WaveReadState) -> Result<Value> {
            st.data_seek_needed = true;
            st.soundpos = 0;
            Ok(Value::none())
        }

        fn setpos(
            #[positional_only] self_val: PyValueRef<'_>,
            pos: PyIndex,
        ) -> Result<Value> {
            let p = pos.expect_i64(FN_NAME, "pos")?;
            let nframes = with_state(self_val.0, |st| Ok(st.nframes as i64))?;
            if p < 0 || p > nframes {
                return Err(raise_wave_error(_interp, "position not in range"));
            }
            with_state_mut(self_val.0, |st| {
                st.soundpos = p as usize;
                st.data_seek_needed = true;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn readframes(
            #[positional_only] self_val: PyValueRef<'_>,
            nframes: PyIndex,
        ) -> Result<Value> {
            let requested = nframes.expect_i64(FN_NAME, "nframes")?;
            let (fp, need_seek, data_start, soundpos, framesize, sampwidth, data_size, data_read) =
                with_state(self_val.0, |st| {
                    let fp = st
                        .file
                        .clone()
                        .ok_or_else(|| PyError::named("ValueError", "I/O operation on closed file"))?;
                    Ok((
                        fp,
                        st.data_seek_needed,
                        st.data_start,
                        st.soundpos,
                        st.framesize,
                        st.sampwidth,
                        st.data_size,
                        st.data_read,
                    ))
                })?;

            let mut cur_data_read = data_read;
            if need_seek {
                let start = data_start
                    .ok_or_else(|| PyError::named("OSError", "cannot seek"))?;
                let byte_offset = (soundpos as u64) * (framesize as u64);
                fp_seek(_interp, &fp, (start + byte_offset) as i64, 0)?;
                cur_data_read = byte_offset;
                with_state_mut(self_val.0, |st| {
                    st.data_read = cur_data_read;
                    st.data_seek_needed = false;
                    Ok(())
                })?;
            }

            if requested == 0 {
                return Ok(Value::bytes(Vec::new()));
            }
            if cur_data_read >= data_size {
                return Ok(Value::bytes(Vec::new()));
            }
            let remaining = data_size - cur_data_read;
            let to_read = if requested < 0 {
                remaining as usize
            } else {
                ((requested as u64).saturating_mul(framesize as u64)).min(remaining) as usize
            };
            let mut data = fp_read(_interp, &fp, to_read)?;
            let read_len = data.len() as u64;
            if cfg!(target_endian = "big") && sampwidth != 1 {
                data = byteswap_samples(&data, sampwidth);
            }
            let frames_got = data.len() / framesize;
            with_state_mut(self_val.0, |st| {
                st.data_read += read_len;
                st.soundpos += frames_got;
                Ok(())
            })?;
            Ok(Value::bytes(data))
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let own_fp = with_state_mut(self_val.0, |st| {
                st.file = None;
                Ok(st.i_opened_the_file.take())
            })?;
            if let Some(fp) = own_fp {
                call_method(_interp, &fp, "close", &[])?;
            }
            Ok(Value::none())
        }
    }

    #[native_state(WaveWriteState)]
    class Wave_write {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
        ) -> Result<Value> {
            let (fp, opened) = open_if_path(_interp, f.0, "wb")?;
            set_state(self_val.0, WaveWriteState::new(fp, opened))?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "close", &[])?;
            Ok(Value::none())
        }

        fn setnchannels(
            #[positional_only] self_val: PyValueRef<'_>,
            nchannels: PyIndex,
        ) -> Result<Value> {
            let n = nchannels.expect_i64(FN_NAME, "nchannels")?;
            let started = with_state(self_val.0, |st| Ok(st.datawritten > 0))?;
            if started {
                return Err(raise_wave_error(
                    _interp,
                    "cannot change parameters after starting to write",
                ));
            }
            if n < 1 {
                return Err(raise_wave_error(_interp, "bad # of channels"));
            }
            with_state_mut(self_val.0, |st| {
                st.nchannels = n as usize;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn getnchannels(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let n = with_state(self_val.0, |st| Ok(st.nchannels))?;
            if n == 0 {
                return Err(raise_wave_error(_interp, "number of channels not set"));
            }
            Ok(Value::int(n as i64))
        }

        fn setsampwidth(
            #[positional_only] self_val: PyValueRef<'_>,
            sampwidth: PyIndex,
        ) -> Result<Value> {
            let w = sampwidth.expect_i64(FN_NAME, "sampwidth")?;
            let started = with_state(self_val.0, |st| Ok(st.datawritten > 0))?;
            if started {
                return Err(raise_wave_error(
                    _interp,
                    "cannot change parameters after starting to write",
                ));
            }
            if !(1..=4).contains(&w) {
                return Err(raise_wave_error(_interp, "bad sample width"));
            }
            with_state_mut(self_val.0, |st| {
                st.sampwidth = w as usize;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn getsampwidth(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let w = with_state(self_val.0, |st| Ok(st.sampwidth))?;
            if w == 0 {
                return Err(raise_wave_error(_interp, "sample width not set"));
            }
            Ok(Value::int(w as i64))
        }

        fn setframerate(
            #[positional_only] self_val: PyValueRef<'_>,
            framerate: PyNumber,
        ) -> Result<Value> {
            let rate = framerate.as_f64();
            let started = with_state(self_val.0, |st| Ok(st.datawritten > 0))?;
            if started {
                return Err(raise_wave_error(
                    _interp,
                    "cannot change parameters after starting to write",
                ));
            }
            if rate <= 0.0 || !rate.is_finite() {
                return Err(raise_wave_error(_interp, "bad frame rate"));
            }
            let rounded = rate.round() as usize;
            with_state_mut(self_val.0, |st| {
                st.framerate = rounded;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn getframerate(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let r = with_state(self_val.0, |st| Ok(st.framerate))?;
            if r == 0 {
                return Err(raise_wave_error(_interp, "frame rate not set"));
            }
            Ok(Value::int(r as i64))
        }

        fn setnframes(
            #[positional_only] self_val: PyValueRef<'_>,
            nframes: PyIndex,
        ) -> Result<Value> {
            let n = nframes.expect_i64(FN_NAME, "nframes")?;
            let started = with_state(self_val.0, |st| Ok(st.datawritten > 0))?;
            if started {
                return Err(raise_wave_error(
                    _interp,
                    "cannot change parameters after starting to write",
                ));
            }
            with_state_mut(self_val.0, |st| {
                st.nframes = n.max(0) as usize;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn getnframes(#[state] st: &WaveWriteState) -> Result<Value> {
            Ok(Value::int(st.nframeswritten as i64))
        }

        fn setcomptype(
            #[positional_only] self_val: PyValueRef<'_>,
            comptype: PyStr<'_>,
            compname: PyStr<'_>,
        ) -> Result<Value> {
            let started = with_state(self_val.0, |st| Ok(st.datawritten > 0))?;
            if started {
                return Err(raise_wave_error(
                    _interp,
                    "cannot change parameters after starting to write",
                ));
            }
            if &*comptype != "NONE" {
                return Err(raise_wave_error(_interp, "unsupported compression type"));
            }
            with_state_mut(self_val.0, |st| {
                st.comptype = Some(comptype.to_string());
                st.compname = Some(compname.to_string());
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn getcomptype(#[state] st: &WaveWriteState) -> Result<Value> {
            match &st.comptype {
                Some(s) => Ok(Value::string(s)),
                None => Err(PyError::named(
                    "AttributeError",
                    "'Wave_write' object has no attribute '_comptype'",
                )),
            }
        }

        fn getcompname(#[state] st: &WaveWriteState) -> Result<Value> {
            match &st.compname {
                Some(s) => Ok(Value::string(s)),
                None => Err(PyError::named(
                    "AttributeError",
                    "'Wave_write' object has no attribute '_compname'",
                )),
            }
        }

        fn setparams(
            #[positional_only] self_val: PyValueRef<'_>,
            params: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&params.0)?;
            if items.len() != 6 {
                return Err(PyError::named(
                    "ValueError",
                    format!("expected 6 values to unpack, got {}", items.len()),
                ));
            }
            let started = with_state(self_val.0, |st| Ok(st.datawritten > 0))?;
            if started {
                return Err(raise_wave_error(
                    _interp,
                    "cannot change parameters after starting to write",
                ));
            }
            call_method(_interp, self_val.0, "setnchannels", &[items[0].clone()])?;
            call_method(_interp, self_val.0, "setsampwidth", &[items[1].clone()])?;
            call_method(_interp, self_val.0, "setframerate", &[items[2].clone()])?;
            call_method(_interp, self_val.0, "setnframes", &[items[3].clone()])?;
            call_method(
                _interp,
                self_val.0,
                "setcomptype",
                &[items[4].clone(), items[5].clone()],
            )?;
            Ok(Value::none())
        }

        fn getparams(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (nc, sw, fr, nf, ct, cn) = with_state(self_val.0, |st| {
                Ok((
                    st.nchannels,
                    st.sampwidth,
                    st.framerate,
                    st.nframes,
                    st.comptype.clone(),
                    st.compname.clone(),
                ))
            })?;
            if nc == 0 || sw == 0 || fr == 0 {
                return Err(raise_wave_error(_interp, "not all parameters set"));
            }
            let ct = ct.unwrap_or_else(|| "NONE".to_string());
            let cn = cn.unwrap_or_else(|| "not compressed".to_string());
            Ok(_wave_params::make([
                Value::int(nc as i64),
                Value::int(sw as i64),
                Value::int(fr as i64),
                Value::int(nf as i64),
                Value::string(ct),
                Value::string(cn),
            ]))
        }

        fn setmark(
            #[positional_only] _self: PyValueRef<'_>,
            _id: PyValue,
            _pos: PyValue,
            _name: PyValue,
        ) -> Result<Value> {
            Err(raise_wave_error(_interp, "setmark() not supported"))
        }

        fn getmark(
            #[positional_only] _self: PyValueRef<'_>,
            _id: PyValue,
        ) -> Result<Value> {
            Err(raise_wave_error(_interp, "no marks"))
        }

        fn getmarkers(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn tell(#[state] st: &WaveWriteState) -> Result<Value> {
            Ok(Value::int(st.nframeswritten as i64))
        }

        fn writeframesraw(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            let raw = data.as_bytes().to_vec();
            let raw_len = raw.len();
            with_state_mut(self_val.0, |st| {
                st.ensure_header_written(_interp, raw_len)?;
                let framesize = st.sampwidth * st.nchannels;
                let nframes = raw_len / framesize;
                let out_bytes = if cfg!(target_endian = "big") && st.sampwidth != 1 {
                    byteswap_samples(&raw, st.sampwidth)
                } else {
                    raw
                };
                let fp = st
                    .file
                    .clone()
                    .ok_or_else(|| PyError::named("ValueError", "I/O operation on closed file"))?;
                fp_write(_interp, &fp, &out_bytes)?;
                st.datawritten += out_bytes.len() as u64;
                st.nframeswritten += nframes;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn writeframes(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            let raw = data.as_bytes().to_vec();
            let raw_len = raw.len();
            with_state_mut(self_val.0, |st| {
                st.ensure_header_written(_interp, raw_len)?;
                let framesize = st.sampwidth * st.nchannels;
                let nframes = raw_len / framesize;
                let out_bytes = if cfg!(target_endian = "big") && st.sampwidth != 1 {
                    byteswap_samples(&raw, st.sampwidth)
                } else {
                    raw
                };
                let fp = st
                    .file
                    .clone()
                    .ok_or_else(|| PyError::named("ValueError", "I/O operation on closed file"))?;
                fp_write(_interp, &fp, &out_bytes)?;
                st.datawritten += out_bytes.len() as u64;
                st.nframeswritten += nframes;
                if st.datalength != st.datawritten {
                    st.patch_header(_interp)?;
                }
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut own_fp = None;
            let res = with_state_mut(self_val.0, |st| {
                let inner = if st.file.is_some() {
                    let r1 = st.ensure_header_written(_interp, 0);
                    let r2 = if r1.is_ok() && st.datalength != st.datawritten {
                        st.patch_header(_interp)
                    } else {
                        Ok(())
                    };
                    let r3 = if let Some(ref fp) = st.file {
                        if let Ok(flush_fn) = _interp.get_attr(fp, "flush") {
                            call_pos(_interp, flush_fn, &[]).map(|_| ())
                        } else {
                            Ok(())
                        }
                    } else {
                        Ok(())
                    };
                    r1.and(r2).and(r3)
                } else {
                    Ok(())
                };
                st.file = None;
                own_fp = st.i_opened_the_file.take();
                inner
            });
            if let Some(fp) = own_fp {
                let _ = call_method(_interp, &fp, "close", &[]);
            }
            res?;
            Ok(Value::none())
        }
    }

    fn open(
        f: PyValue,
        #[default(OptionalArg::Omitted)] mode: OptValueRef<'_>,
    ) -> Result<Value> {
        let mode_str = match mode.as_value() {
            Some(v) if !v.is_none() => match v.as_str() {
                Some(s) => s.to_string(),
                None => {
                    let _ = render_value_repr(_interp, v);
                    return Err(raise_wave_error(
                        _interp,
                        "mode must be 'r', 'rb', 'w', or 'wb'",
                    ));
                }
            },
            _ => {
                if let Ok(m) = _interp.get_attr(&f.0, "mode")
                    && let Some(s) = m.as_str()
                {
                    s.to_string()
                } else {
                    "rb".to_string()
                }
            }
        };

        let wave_mod = _interp.load_module("wave")?;
        match mode_str.as_str() {
            "r" | "rb" => {
                let cls = _interp.get_attr(&wave_mod, "Wave_read")?;
                call_pos(_interp, cls, &[f.0])
            }
            "w" | "wb" => {
                let cls = _interp.get_attr(&wave_mod, "Wave_write")?;
                call_pos(_interp, cls, &[f.0])
            }
            _ => Err(raise_wave_error(
                _interp,
                "mode must be 'r', 'rb', 'w', or 'wb'",
            )),
        }
    }
}

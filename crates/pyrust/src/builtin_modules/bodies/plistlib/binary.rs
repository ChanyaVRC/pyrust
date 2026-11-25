// Apple Binary Plist (`bplist00` / `FMT_BINARY`) parser and serializer for
// `plistlib` (issue #3259).
// Included directly into `bodies/plistlib.rs`.

use std::collections::HashMap;

fn is_fmt_binary(header: &[u8]) -> bool {
    header.len() >= 8 && &header[..8] == b"bplist00"
}

fn count_to_size(count: usize) -> u8 {
    if count < (1 << 8) {
        1
    } else if count < (1 << 16) {
        2
    } else if (count as u64) < (1u64 << 32) {
        4
    } else {
        8
    }
}

fn write_be_uint(out: &mut Vec<u8>, value: u64, size: u8) {
    match size {
        1 => out.push(value as u8),
        2 => out.extend_from_slice(&(value as u16).to_be_bytes()),
        4 => out.extend_from_slice(&(value as u32).to_be_bytes()),
        8 => out.extend_from_slice(&value.to_be_bytes()),
        _ => unreachable!("invalid binary plist integer size"),
    }
}

fn read_be_uint(bytes: &[u8]) -> u64 {
    let mut acc = 0u64;
    for &b in bytes {
        acc = (acc << 8) | (b as u64);
    }
    acc
}

// ── Binary Plist Reader ──────────────────────────────────────────────────────

struct BinaryPlistReader<'a> {
    data: &'a [u8],
    ref_size: usize,
    object_offsets: Vec<usize>,
    objects: Vec<Option<Value>>,
    dict_type: Value,
}

impl<'a> BinaryPlistReader<'a> {
    fn parse(interp: &mut Interpreter, data: &'a [u8], dict_type: Value) -> Result<Value> {
        if data.len() < 40 || &data[..8] != b"bplist00" {
            return Err(raise_invalid_file(interp, "Invalid file"));
        }
        let trailer = &data[data.len() - 32..];
        let offset_size = trailer[6] as usize;
        let ref_size = trailer[7] as usize;
        let num_objects = u64::from_be_bytes(trailer[8..16].try_into().unwrap());
        let top_object = u64::from_be_bytes(trailer[16..24].try_into().unwrap());
        let offset_table_offset = u64::from_be_bytes(trailer[24..32].try_into().unwrap());

        if offset_size == 0 || offset_size > 8 || ref_size == 0 || ref_size > 8 {
            return Err(raise_invalid_file(interp, "Invalid file"));
        }
        let num_objects_usize =
            usize::try_from(num_objects).map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
        let top_object_usize =
            usize::try_from(top_object).map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
        let offset_table_start = usize::try_from(offset_table_offset)
            .map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
        let table_bytes = num_objects_usize
            .checked_mul(offset_size)
            .ok_or_else(|| raise_invalid_file(interp, "Invalid file"))?;
        let table_end = offset_table_start
            .checked_add(table_bytes)
            .ok_or_else(|| raise_invalid_file(interp, "Invalid file"))?;
        if table_end > data.len() - 32 || top_object_usize >= num_objects_usize {
            return Err(raise_invalid_file(interp, "Invalid file"));
        }

        let mut object_offsets = Vec::with_capacity(num_objects_usize);
        let mut cursor = offset_table_start;
        for _ in 0..num_objects_usize {
            let off = read_be_uint(&data[cursor..cursor + offset_size]);
            let off_usize =
                usize::try_from(off).map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
            if off_usize >= data.len() - 32 {
                return Err(raise_invalid_file(interp, "Invalid file"));
            }
            object_offsets.push(off_usize);
            cursor += offset_size;
        }

        let mut reader = Self {
            data,
            ref_size,
            object_offsets,
            objects: vec![None; num_objects_usize],
            dict_type,
        };
        reader.read_object(interp, top_object_usize)
    }

    fn read_slice(
        &self,
        interp: &mut Interpreter,
        pos: &mut usize,
        len: usize,
    ) -> Result<&'a [u8]> {
        let end = pos
            .checked_add(len)
            .ok_or_else(|| raise_invalid_file(interp, "Invalid file"))?;
        if end > self.data.len() {
            return Err(raise_invalid_file(interp, "Invalid file"));
        }
        let slice = &self.data[*pos..end];
        *pos = end;
        Ok(slice)
    }

    fn read_size(&self, interp: &mut Interpreter, pos: &mut usize, token_l: u8) -> Result<usize> {
        if token_l != 0x0F {
            return Ok(token_l as usize);
        }
        let marker = self.read_slice(interp, pos, 1)?[0];
        let m = marker & 0x03;
        let nbytes = 1usize << m;
        let bytes = self.read_slice(interp, pos, nbytes)?;
        let sz = read_be_uint(bytes);
        usize::try_from(sz).map_err(|_| raise_invalid_file(interp, "Invalid file"))
    }

    fn read_refs(
        &self,
        interp: &mut Interpreter,
        pos: &mut usize,
        count: usize,
    ) -> Result<Vec<usize>> {
        let total = count
            .checked_mul(self.ref_size)
            .ok_or_else(|| raise_invalid_file(interp, "Invalid file"))?;
        let raw = self.read_slice(interp, pos, total)?;
        let mut refs = Vec::with_capacity(count);
        for chunk in raw.chunks_exact(self.ref_size) {
            let r = read_be_uint(chunk);
            let r_usize =
                usize::try_from(r).map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
            if r_usize >= self.object_offsets.len() {
                return Err(raise_invalid_file(interp, "Invalid file"));
            }
            refs.push(r_usize);
        }
        Ok(refs)
    }

    fn read_object(&mut self, interp: &mut Interpreter, obj_ref: usize) -> Result<Value> {
        if obj_ref >= self.objects.len() {
            return Err(raise_invalid_file(interp, "Invalid file"));
        }
        if let Some(existing) = &self.objects[obj_ref] {
            return Ok(existing.clone());
        }

        let mut pos = self.object_offsets[obj_ref];
        let token = self.read_slice(interp, &mut pos, 1)?[0];
        let token_h = token & 0xF0;
        let token_l = token & 0x0F;

        let result = match token {
            0x00 => Value::none(),
            0x08 => Value::bool_(false),
            0x09 => Value::bool_(true),
            0x0F => Value::bytes(Vec::new()),
            0x22 => {
                let bytes = self.read_slice(interp, &mut pos, 4)?;
                let f = f32::from_be_bytes(bytes.try_into().unwrap()) as f64;
                Value::float(f)
            }
            0x23 => {
                let bytes = self.read_slice(interp, &mut pos, 8)?;
                let f = f64::from_be_bytes(bytes.try_into().unwrap());
                Value::float(f)
            }
            0x33 => {
                let bytes = self.read_slice(interp, &mut pos, 8)?;
                let secs = f64::from_be_bytes(bytes.try_into().unwrap());
                make_datetime_from_apple_timestamp(interp, secs)
                    .map_err(|_| raise_invalid_file(interp, "Invalid file"))?
            }
            _ => match token_h {
                0x10 => {
                    if token_l > 4 {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    }
                    let nbytes = 1usize << token_l;
                    let bytes = self.read_slice(interp, &mut pos, nbytes)?;
                    if token_l < 3 {
                        Value::int(read_be_uint(bytes) as i64)
                    } else if token_l == 3 {
                        Value::int(i64::from_be_bytes(bytes.try_into().unwrap()))
                    } else {
                        let v = i128::from_be_bytes(bytes.try_into().unwrap());
                        if let Ok(i) = i64::try_from(v) {
                            Value::int(i)
                        } else {
                            Value::bigint(num_bigint::BigInt::from(v))
                        }
                    }
                }
                0x40 => {
                    let len = self.read_size(interp, &mut pos, token_l)?;
                    let bytes = self.read_slice(interp, &mut pos, len)?;
                    Value::bytes(bytes.to_vec())
                }
                0x50 => {
                    let len = self.read_size(interp, &mut pos, token_l)?;
                    let bytes = self.read_slice(interp, &mut pos, len)?;
                    if !bytes.is_ascii() {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    }
                    let s = std::str::from_utf8(bytes)
                        .map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
                    Value::string(s)
                }
                0x60 => {
                    let count = self.read_size(interp, &mut pos, token_l)?;
                    let nbytes = count
                        .checked_mul(2)
                        .ok_or_else(|| raise_invalid_file(interp, "Invalid file"))?;
                    let bytes = self.read_slice(interp, &mut pos, nbytes)?;
                    let units: Vec<u16> = bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&c| u16::from_be_bytes(c))
                        .collect();
                    let s = String::from_utf16(&units)
                        .map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
                    Value::string(s)
                }
                0x80 => {
                    let nbytes = 1usize + (token_l as usize);
                    if nbytes > 8 {
                        return Err(raise_invalid_file(interp, "Invalid file"));
                    }
                    let bytes = self.read_slice(interp, &mut pos, nbytes)?;
                    let uid_val = read_be_uint(bytes);
                    let num_val = if let Ok(i) = i64::try_from(uid_val) {
                        Value::int(i)
                    } else {
                        Value::bigint(num_bigint::BigInt::from(uid_val))
                    };
                    let plist_mod = interp.load_module("plistlib")?;
                    let uid_cls = interp.get_attr(&plist_mod, "UID")?;
                    call_pos(interp, uid_cls, &[num_val])?
                }
                0xA0 => {
                    let count = self.read_size(interp, &mut pos, token_l)?;
                    let obj_refs = self.read_refs(interp, &mut pos, count)?;
                    let list = Value::list(Vec::with_capacity(count));
                    self.objects[obj_ref] = Some(list.clone());
                    for r in obj_refs {
                        let elem = self.read_object(interp, r)?;
                        call_method(interp, &list, "append", &[elem])?;
                    }
                    list
                }
                0xD0 => {
                    let count = self.read_size(interp, &mut pos, token_l)?;
                    let key_refs = self.read_refs(interp, &mut pos, count)?;
                    let val_refs = self.read_refs(interp, &mut pos, count)?;
                    let dict_val = call_pos(interp, self.dict_type.clone(), &[])?;
                    self.objects[obj_ref] = Some(dict_val.clone());
                    for (kr, vr) in key_refs.into_iter().zip(val_refs) {
                        let k = self.read_object(interp, kr)?;
                        let v = self.read_object(interp, vr)?;
                        call_method(interp, &dict_val, "__setitem__", &[k, v])
                            .map_err(|_| raise_invalid_file(interp, "Invalid file"))?;
                    }
                    dict_val
                }
                _ => return Err(raise_invalid_file(interp, "Invalid file")),
            },
        };

        self.objects[obj_ref] = Some(result.clone());
        Ok(result)
    }
}

fn parse_binary_plist(interp: &mut Interpreter, raw: &[u8], dict_type: Value) -> Result<Value> {
    BinaryPlistReader::parse(interp, raw, dict_type)
}

// ── Binary Plist Writer ──────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Eq, Hash)]
enum ScalarKey {
    Str(String),
    Bool(bool),
    Int(i128),
    Float(u64),
    Bytes(Vec<u8>),
    DateTime(i32, u8, u8, u8, u8, u8, u32),
}

enum FlattenKey {
    Scalar(ScalarKey),
    ObjId(u128),
}

fn value_obj_id(val: &Value) -> u128 {
    let id_val = val.object_id();
    match id_val.kind() {
        ValueKind::Int(n) => n as u64 as u128,
        ValueKind::BigInt(bi) => u128::try_from(bi).unwrap_or(0),
        _ => 0,
    }
}

fn classify_flatten_key(interp: &mut Interpreter, val: &Value) -> Result<FlattenKey> {
    if pyrust_builtins::bytearray::as_bytearray_snapshot(val).is_some() {
        return Ok(FlattenKey::ObjId(value_obj_id(val)));
    }
    match val.kind() {
        ValueKind::Str(s) => Ok(FlattenKey::Scalar(ScalarKey::Str(s.to_string()))),
        ValueKind::Bool(b) => Ok(FlattenKey::Scalar(ScalarKey::Bool(b))),
        ValueKind::Int(n) => Ok(FlattenKey::Scalar(ScalarKey::Int(n as i128))),
        ValueKind::BigInt(bi) => {
            if let Ok(n) = i128::try_from(bi) {
                Ok(FlattenKey::Scalar(ScalarKey::Int(n)))
            } else {
                Err(PyError::named("OverflowError", bi.to_string()))
            }
        }
        ValueKind::Float(f) => {
            if f.is_nan() {
                Ok(FlattenKey::ObjId(value_obj_id(val)))
            } else {
                let normalized = if f == 0.0 {
                    0.0f64.to_bits()
                } else {
                    f.to_bits()
                };
                Ok(FlattenKey::Scalar(ScalarKey::Float(normalized)))
            }
        }
        ValueKind::Bytes(b) => Ok(FlattenKey::Scalar(ScalarKey::Bytes(b.as_slice().to_vec()))),
        ValueKind::PyInstance(_) => {
            if is_datetime_instance(interp, val)? {
                let parts = extract_datetime_parts(interp, val)?;
                Ok(FlattenKey::Scalar(ScalarKey::DateTime(
                    parts.0, parts.1, parts.2, parts.3, parts.4, parts.5, parts.6,
                )))
            } else {
                Ok(FlattenKey::ObjId(value_obj_id(val)))
            }
        }
        _ => Ok(FlattenKey::ObjId(value_obj_id(val))),
    }
}

struct BinaryPlistWriter {
    sort_keys: bool,
    skipkeys: bool,
    objlist: Vec<Value>,
    scalar_table: HashMap<ScalarKey, usize>,
    id_table: HashMap<u128, usize>,
    object_offsets: Vec<u64>,
    ref_size: u8,
    out: Vec<u8>,
}

impl BinaryPlistWriter {
    fn new(sort_keys: bool, skipkeys: bool) -> Self {
        Self {
            sort_keys,
            skipkeys,
            objlist: Vec::new(),
            scalar_table: HashMap::new(),
            id_table: HashMap::new(),
            object_offsets: Vec::new(),
            ref_size: 1,
            out: Vec::new(),
        }
    }

    fn write_root(mut self, interp: &mut Interpreter, value: &Value) -> Result<Vec<u8>> {
        self.flatten(interp, value)?;

        let num_objects = self.objlist.len();
        self.object_offsets = vec![0u64; num_objects];
        self.ref_size = count_to_size(num_objects);

        self.out.extend_from_slice(b"bplist00");

        let objs = self.objlist.clone();
        for obj in &objs {
            self.write_object(interp, obj)?;
        }

        let top_object = self.get_refnum(interp, value)? as u64;
        let offset_table_offset = self.out.len() as u64;
        let offset_size = count_to_size(offset_table_offset as usize);
        for &off in &self.object_offsets {
            write_be_uint(&mut self.out, off, offset_size);
        }

        // 32-byte trailer: 5x, sort_version(B), offset_size(B), ref_size(B),
        // num_objects(Q), top_object(Q), offset_table_offset(Q)
        self.out.extend_from_slice(&[0u8; 5]);
        self.out.push(0u8); // sort_version
        self.out.push(offset_size);
        self.out.push(self.ref_size);
        self.out
            .extend_from_slice(&(num_objects as u64).to_be_bytes());
        self.out.extend_from_slice(&top_object.to_be_bytes());
        self.out
            .extend_from_slice(&offset_table_offset.to_be_bytes());

        Ok(self.out)
    }

    fn flatten(&mut self, interp: &mut Interpreter, value: &Value) -> Result<()> {
        let key = classify_flatten_key(interp, value)?;
        match &key {
            FlattenKey::Scalar(sk) => {
                if self.scalar_table.contains_key(sk) {
                    return Ok(());
                }
            }
            FlattenKey::ObjId(id) => {
                if self.id_table.contains_key(id) {
                    return Ok(());
                }
            }
        }

        let refnum = self.objlist.len();
        self.objlist.push(value.clone());
        match key {
            FlattenKey::Scalar(sk) => {
                self.scalar_table.insert(sk, refnum);
            }
            FlattenKey::ObjId(id) => {
                self.id_table.insert(id, refnum);
            }
        }

        match value.kind() {
            ValueKind::Dict(_) => {
                let items = collect_dict_items(interp, value, self.sort_keys, self.skipkeys)?;
                let mut vals = Vec::with_capacity(items.len());
                for (k, v) in items {
                    self.flatten(interp, &Value::string(k))?;
                    vals.push(v);
                }
                for v in vals {
                    self.flatten(interp, &v)?;
                }
            }
            ValueKind::List(items) => {
                let cloned = items.to_vec();
                for o in &cloned {
                    self.flatten(interp, o)?;
                }
            }
            ValueKind::Tuple(items) => {
                for o in items.iter() {
                    self.flatten(interp, o)?;
                }
            }
            ValueKind::PyInstance(_) => {
                if is_dict_subclass(interp, value)? {
                    let items = collect_dict_items(interp, value, self.sort_keys, self.skipkeys)?;
                    let mut vals = Vec::with_capacity(items.len());
                    for (k, v) in items {
                        self.flatten(interp, &Value::string(k))?;
                        vals.push(v);
                    }
                    for v in vals {
                        self.flatten(interp, &v)?;
                    }
                } else if is_list_or_tuple_subclass(interp, value)? {
                    let items = interp.collect_iterable(value)?;
                    for o in &items {
                        self.flatten(interp, o)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn get_refnum(&self, interp: &mut Interpreter, value: &Value) -> Result<usize> {
        match classify_flatten_key(interp, value)? {
            FlattenKey::Scalar(sk) => self
                .scalar_table
                .get(&sk)
                .copied()
                .ok_or_else(|| PyError::runtime("missing scalar refnum in binary plist".into())),
            FlattenKey::ObjId(id) => self
                .id_table
                .get(&id)
                .copied()
                .ok_or_else(|| PyError::runtime("missing object refnum in binary plist".into())),
        }
    }

    fn write_size(&mut self, token: u8, size: usize) {
        if size < 15 {
            self.out.push(token | (size as u8));
        } else if size < (1 << 8) {
            self.out.push(token | 0x0F);
            self.out.push(0x10);
            self.out.push(size as u8);
        } else if size < (1 << 16) {
            self.out.push(token | 0x0F);
            self.out.push(0x11);
            self.out.extend_from_slice(&(size as u16).to_be_bytes());
        } else if (size as u64) < (1u64 << 32) {
            self.out.push(token | 0x0F);
            self.out.push(0x12);
            self.out.extend_from_slice(&(size as u32).to_be_bytes());
        } else {
            self.out.push(token | 0x0F);
            self.out.push(0x13);
            self.out.extend_from_slice(&(size as u64).to_be_bytes());
        }
    }

    fn write_int_value(&mut self, n: i128) -> Result<()> {
        if n < 0 {
            if let Ok(v) = i64::try_from(n) {
                self.out.push(0x13);
                self.out.extend_from_slice(&v.to_be_bytes());
                Ok(())
            } else {
                Err(PyError::named("OverflowError", n.to_string()))
            }
        } else if n < (1i128 << 8) {
            self.out.push(0x10);
            self.out.push(n as u8);
            Ok(())
        } else if n < (1i128 << 16) {
            self.out.push(0x11);
            self.out.extend_from_slice(&(n as u16).to_be_bytes());
            Ok(())
        } else if n < (1i128 << 32) {
            self.out.push(0x12);
            self.out.extend_from_slice(&(n as u32).to_be_bytes());
            Ok(())
        } else if n < (1i128 << 63) {
            self.out.push(0x13);
            self.out.extend_from_slice(&(n as u64).to_be_bytes());
            Ok(())
        } else if n < (1i128 << 64) {
            self.out.push(0x14);
            self.out.extend_from_slice(&n.to_be_bytes());
            Ok(())
        } else {
            Err(PyError::named("OverflowError", n.to_string()))
        }
    }

    fn write_string(&mut self, s: &str) {
        if s.is_ascii() {
            self.write_size(0x50, s.len());
            self.out.extend_from_slice(s.as_bytes());
        } else {
            let units: Vec<u16> = s.encode_utf16().collect();
            self.write_size(0x60, units.len());
            for u in units {
                self.out.extend_from_slice(&u.to_be_bytes());
            }
        }
    }

    fn write_uid(&mut self, uid_data: i128) -> Result<()> {
        if uid_data < 0 {
            return Err(PyError::named("ValueError", "UIDs must be positive"));
        }
        if uid_data < (1i128 << 8) {
            self.out.push(0x80);
            self.out.push(uid_data as u8);
        } else if uid_data < (1i128 << 16) {
            self.out.push(0x81);
            self.out.extend_from_slice(&(uid_data as u16).to_be_bytes());
        } else if uid_data < (1i128 << 32) {
            self.out.push(0x83);
            self.out.extend_from_slice(&(uid_data as u32).to_be_bytes());
        } else if uid_data < (1i128 << 64) {
            self.out.push(0x87);
            self.out.extend_from_slice(&(uid_data as u64).to_be_bytes());
        } else {
            return Err(PyError::named("OverflowError", uid_data.to_string()));
        }
        Ok(())
    }

    fn write_object(&mut self, interp: &mut Interpreter, value: &Value) -> Result<()> {
        let obj_ref = self.get_refnum(interp, value)?;
        self.object_offsets[obj_ref] = self.out.len() as u64;

        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(value) {
            self.write_size(0x40, snap.len());
            self.out.extend_from_slice(&snap);
            return Ok(());
        }

        match value.kind() {
            ValueKind::None => self.out.push(0x00),
            ValueKind::Bool(false) => self.out.push(0x08),
            ValueKind::Bool(true) => self.out.push(0x09),
            ValueKind::Int(n) => self.write_int_value(n as i128)?,
            ValueKind::BigInt(bi) => {
                let n = i128::try_from(bi)
                    .map_err(|_| PyError::named("OverflowError", bi.to_string()))?;
                self.write_int_value(n)?;
            }
            ValueKind::Float(f) => {
                self.out.push(0x23);
                self.out.extend_from_slice(&f.to_be_bytes());
            }
            ValueKind::Bytes(b) => {
                self.write_size(0x40, b.len());
                self.out.extend_from_slice(b.as_slice());
            }
            ValueKind::Str(s) => self.write_string(s),
            ValueKind::List(items) => {
                let cloned = items.to_vec();
                self.write_array_refs(interp, &cloned)?;
            }
            ValueKind::Tuple(items) => {
                self.write_array_refs(interp, items)?;
            }
            ValueKind::Dict(_) => {
                self.write_dict_refs(interp, value)?;
            }
            ValueKind::PyInstance(_) => {
                if is_uid_instance(interp, value)? {
                    let data_val = interp.get_attr(value, "data")?;
                    let uid_n = extract_int_i128(interp, &data_val)?;
                    self.write_uid(uid_n)?;
                } else if is_datetime_instance(interp, value)? {
                    let secs = datetime_to_apple_timestamp(interp, value)?;
                    self.out.push(0x33);
                    self.out.extend_from_slice(&secs.to_be_bytes());
                } else if is_dict_subclass(interp, value)? {
                    self.write_dict_refs(interp, value)?;
                } else if is_str_subclass(interp, value)? {
                    let s = extract_str_value(interp, value)?;
                    self.write_string(&s);
                } else if is_int_subclass(interp, value)? {
                    let n = extract_int_i128(interp, value)?;
                    self.write_int_value(n)?;
                } else if is_list_or_tuple_subclass(interp, value)? {
                    let items = interp.collect_iterable(value)?;
                    self.write_array_refs(interp, &items)?;
                } else {
                    let repr = render_value_repr(interp, value)?;
                    return Err(PyError::named("TypeError", repr));
                }
            }
            _ => {
                let repr = render_value_repr(interp, value)?;
                return Err(PyError::named("TypeError", repr));
            }
        }
        Ok(())
    }

    fn write_array_refs(&mut self, interp: &mut Interpreter, items: &[Value]) -> Result<()> {
        let mut refs = Vec::with_capacity(items.len());
        for o in items {
            refs.push(self.get_refnum(interp, o)?);
        }
        self.write_size(0xA0, refs.len());
        for r in refs {
            write_be_uint(&mut self.out, r as u64, self.ref_size);
        }
        Ok(())
    }

    fn write_dict_refs(&mut self, interp: &mut Interpreter, dict_val: &Value) -> Result<()> {
        let items = collect_dict_items(interp, dict_val, self.sort_keys, self.skipkeys)?;
        let mut key_refs = Vec::with_capacity(items.len());
        let mut val_refs = Vec::with_capacity(items.len());
        for (k, v) in items {
            key_refs.push(self.get_refnum(interp, &Value::string(k))?);
            val_refs.push(self.get_refnum(interp, &v)?);
        }
        self.write_size(0xD0, key_refs.len());
        for r in key_refs {
            write_be_uint(&mut self.out, r as u64, self.ref_size);
        }
        for r in val_refs {
            write_be_uint(&mut self.out, r as u64, self.ref_size);
        }
        Ok(())
    }
}

fn dump_binary_plist(
    interp: &mut Interpreter,
    value: &Value,
    sort_keys: bool,
    skipkeys: bool,
) -> Result<Vec<u8>> {
    BinaryPlistWriter::new(sort_keys, skipkeys).write_root(interp, value)
}

impl Interpreter {
    fn slice_index_from_value(value: &Value) -> Result<i64> {
        match value.kind() {
            ValueKind::Int(i) => Ok(i),
            ValueKind::Bool(b) => Ok(if b { 1 } else { 0 }),
            _ => Err(PyError::Runtime("slice indices must be integers".to_string())),
        }
    }

    fn resolve_slice_bounds(
        len: i64,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<(i64, i64, i64)> {
        let step = match st {
            None => 1,
            Some(v) if v.is_none() => 1,
            Some(v) => {
                let s = Self::slice_index_from_value(v)?;
                if s == 0 {
                    return Err(PyError::Runtime("slice step cannot be zero".to_string()));
                }
                s
            }
        };

        let normalize = |idx: i64| -> i64 {
            if idx < 0 {
                (idx + len).clamp(0, len)
            } else {
                idx.clamp(0, len)
            }
        };

        let start_default = if step > 0 { 0 } else { len - 1 };
        let end_default = if step > 0 { len } else { -1 };

        let start = match lo {
            None => start_default,
            Some(v) if v.is_none() => start_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        let end = match hi {
            None => end_default,
            Some(v) if v.is_none() => end_default,
            Some(v) => {
                let i = Self::slice_index_from_value(v)?;
                if step > 0 {
                    normalize(i)
                } else if i < 0 {
                    (i + len).clamp(-1, len - 1)
                } else {
                    i.clamp(-1, len - 1)
                }
            }
        };

        Ok((start, end, step))
    }

    fn slice_target_indices(len: i64, start: i64, end: i64, step: i64) -> Vec<usize> {
        let mut targets = Vec::new();
        let mut i = start;

        if step > 0 {
            while i < end {
                if i >= 0 && i < len {
                    targets.push(i as usize);
                }
                i += step;
            }
        } else {
            while i > end {
                if i >= 0 && i < len {
                    targets.push(i as usize);
                }
                i += step;
            }
        }
        targets
    }

    /// If `key` is a 3-element tuple produced by the slice compiler, unpack it.
    /// Returns `Some((lo, hi, step))` where each is `None` for a missing bound.
    pub(crate) fn unpack_slice_key(key: &Value) -> Option<(Option<Value>, Option<Value>, Option<Value>)> {
        if let ValueKind::Tuple(elems) = key.kind()
            && elems.len() == 3 {
                let opt = |v: &Value| if v.is_none() { None } else { Some(v.clone()) };
                return Some((opt(&elems[0]), opt(&elems[1]), opt(&elems[2])));
            }
        None
    }

    /// Slice-assign: `items[lo:hi:step] = new_items`.
    pub(crate) fn slice_setitem(
        items: &mut Vec<Value>,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
        new_items: Vec<Value>,
    ) -> Result<()> {
        let len = items.len() as i64;
        let (start, end, step) = Self::resolve_slice_bounds(len, lo, hi, st)?;
        if step == 1 {
            let s = start as usize;
            let e = end as usize;
            items.splice(s..e, new_items);
        } else {
            let indices = Self::slice_target_indices(len, start, end, step);
            if indices.len() != new_items.len() {
                return Err(PyError::Runtime(
                    "attempt to assign sequence of wrong size".to_string(),
                ));
            }
            for (ix, val) in indices.into_iter().zip(new_items) {
                items[ix] = val;
            }
        }
        Ok(())
    }

    /// Slice-delete: `del items[lo:hi:step]` (equivalent to `items[lo:hi:step] = []`).
    pub(crate) fn slice_delitem(
        items: &mut Vec<Value>,
        lo: Option<&Value>,
        hi: Option<&Value>,
        st: Option<&Value>,
    ) -> Result<()> {
        let len = items.len() as i64;
        let (start, end, step) = Self::resolve_slice_bounds(len, lo, hi, st)?;
        let indices = Self::slice_target_indices(len, start, end, step);
        // Remove in reverse so indices stay valid.
        let mut sorted = indices;
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for ix in sorted {
            items.remove(ix);
        }
        Ok(())
    }

    fn coerce_to_exception(&self, value: Value) -> Result<Value> {
        match value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                if is_exception_class(&instance.borrow().class) {
                    Ok(Value::py_instance(instance))
                } else {
                    Err(PyError::Runtime(
                        "exceptions must derive from Exception".to_string(),
                    ))
                }
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if is_exception_class(&class) {
                    Ok(instantiate_exception(class, Vec::new()))
                } else {
                    Err(PyError::Runtime(
                        "exceptions must derive from Exception".to_string(),
                    ))
                }
            }
            _ => Err(PyError::Runtime(
                "exceptions must derive from Exception".to_string(),
            )),
        }
    }

    fn instantiate_named_exception(&self, name: &str, message: String) -> Result<Value> {
        let class = match lookup_name_in_module(&self.env, name) {
            Some(v) => match v.kind() {
                ValueKind::PyClass(c) => Rc::clone(c),
                _ => return Err(PyError::Runtime(format!(
                    "built-in exception '{}' is not defined",
                    name
                ))),
            },
            None => return Err(PyError::Runtime(format!(
                "built-in exception '{}' is not defined",
                name
            ))),
        };
        Ok(instantiate_exception(class, vec![Value::string(message)]))
    }

    fn exception_matches(&self, exception: &Value, kind: &Value) -> Result<bool> {
        let instance = match exception.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(false),
        };

        let raised_class = Rc::clone(&instance.borrow().class);
        match kind.kind() {
            ValueKind::PyClass(expected) => {
                let expected = Rc::clone(expected);
                if !is_exception_class(&expected) {
                    return Err(PyError::Runtime(
                        "except clause must reference an exception class".to_string(),
                    ));
                }
                Ok(class_is_subclass_of(&raised_class, &expected))
            }
            ValueKind::Tuple(items) => {
                for item in items {
                    match item.kind() {
                        ValueKind::PyClass(expected) => {
                            let expected = Rc::clone(expected);
                            if !is_exception_class(&expected) {
                                return Err(PyError::Runtime(
                                    "except clause must reference an exception class".to_string(),
                                ));
                            }
                            if class_is_subclass_of(&raised_class, &expected) {
                                return Ok(true);
                            }
                        }
                        _ => return Err(PyError::Runtime(
                            "except clause must reference an exception class".to_string(),
                        )),
                    }
                }
                Ok(false)
            }
            _ => Err(PyError::Runtime(
                "except clause must reference an exception class".to_string(),
            )),
        }
    }

}

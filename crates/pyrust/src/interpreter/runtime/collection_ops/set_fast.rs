// No-clone fast path for primitive-key set algebra.

#[inline]
fn set_direct_value(v: &Value) -> Option<(Value, bool)> {
    if matches!(v.kind(), ValueKind::Set(_)) {
        return Some((v.clone(), false));
    }
    if pyrust_builtins::frozenset::as_items(v).is_some() {
        return Some((v.clone(), true));
    }
    builtin_data_backing(v).and_then(|backing| set_direct_value(&backing))
}

#[inline]
fn with_set_items<R>(v: &Value, f: impl FnOnce(&PySet) -> R) -> R {
    if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
        return f(&rc);
    }
    v.set_with(f)
        .expect("set_direct_value guarantees a set/frozenset value")
}

#[inline]
fn set_algebra_fast(a: &PySet, b: &PySet, op: SetOp) -> PySet {
    if let SetOp::Or = op {
        let mut out = a.clone();
        out.reserve(b.len());
        for key in b.iter() {
            out.insert(key.clone());
        }
        return out;
    }

    let capacity = match op {
        SetOp::And => a.len().min(b.len()),
        SetOp::Sub => a.len(),
        SetOp::Xor => a.len() + b.len(),
        SetOp::Or => unreachable!(),
    };
    let mut out = PySet::with_capacity_and_hasher(capacity, Default::default());
    match op {
        SetOp::And => {
            for key in a.iter().filter(|key| b.contains(*key)) {
                out.insert(key.clone());
            }
        }
        SetOp::Sub => {
            for key in a.iter().filter(|key| !b.contains(*key)) {
                out.insert(key.clone());
            }
        }
        SetOp::Xor => {
            for key in a.iter().filter(|key| !b.contains(*key)) {
                out.insert(key.clone());
            }
            for key in b.iter().filter(|key| !a.contains(*key)) {
                out.insert(key.clone());
            }
        }
        SetOp::Or => unreachable!(),
    }
    out
}

// `filecmp` module — body for the `filecmp @inject` entry in
// `pyrust_builtin_modules!` (issue #3230).
//
// CPython 3.12's `filecmp` is a pure-Python module providing file and
// directory comparison utilities (`cmp`, `cmpfiles`, `dircmp`, `clear_cache`,
// `DEFAULT_IGNORES`, and `BUFSIZE`).
//
// Reference: <https://docs.python.org/3/library/filecmp.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `filecmp` module.
const FILECMP_PY_SOURCE: &str = include_str!("filecmp_py.py");

/// Public names exported from `FILECMP_PY_SOURCE` onto the `filecmp` module.
const FILECMP_PY_EXPORTS: &[&str] = &[
    "__doc__",
    "__all__",
    "_cache",
    "BUFSIZE",
    "DEFAULT_IGNORES",
    "clear_cache",
    "cmp",
    "_sig",
    "_do_cmp",
    "dircmp",
    "cmpfiles",
    "_cmp",
    "_filter",
    "demo",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(FILECMP_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("filecmp: exec namespace not a dict".into()))?;
    for name in FILECMP_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}

#[cfg(test)]
mod tests {
    use crate::interpreter::Interpreter;

    #[test]
    fn dircmp_supports_shallow_kwarg() {
        let mut interp = Interpreter::default();
        let src = r#"
import filecmp, os, tempfile

with tempfile.TemporaryDirectory() as tmp:
    d1 = os.path.join(tmp, "a")
    d2 = os.path.join(tmp, "b")
    os.mkdir(d1)
    os.mkdir(d2)
    os.mkdir(os.path.join(d1, "sub"))
    os.mkdir(os.path.join(d2, "sub"))
    with open(os.path.join(d1, "f.txt"), "wb") as f:
        f.write(b"hello")
    with open(os.path.join(d2, "f.txt"), "wb") as f:
        f.write(b"world")
    with open(os.path.join(d1, "sub", "g.txt"), "wb") as f:
        f.write(b"same")
    with open(os.path.join(d2, "sub", "g.txt"), "wb") as f:
        f.write(b"same")
    dc = filecmp.dircmp(d1, d2, shallow=False)
    assert dc.shallow is False
    assert dc.diff_files == ["f.txt"], dc.diff_files
    assert dc.subdirs["sub"].shallow is False
    assert dc.subdirs["sub"].same_files == ["g.txt"]
"#;
        interp
            .exec_source(src, None, None)
            .expect("dircmp shallow=False should succeed");
    }
}

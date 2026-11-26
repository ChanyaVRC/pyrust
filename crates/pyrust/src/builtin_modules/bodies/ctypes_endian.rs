// `ctypes._endian` module — 100% native-Rust endian-swapped Structure/Union bases (issue #3265).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::ctypes_rt::other_endian_type;
use crate::builtin_modules::ctypes_rt::types::core_classes;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, PyModule, Value, ValueKind};

pyrust_module! {
    fn _other_endian(#[positional_only] typ: PyValue) -> Result<Value> {
        let ValueKind::PyClass(cls) = typ.0.kind() else {
            return Err(PyError::named(
                "TypeError",
                "This type does not support other endian",
            ));
        };
        let swapped = other_endian_type(_interp, cls)?;
        Ok(Value::py_class(swapped))
    }
}

fn make_swapped_base(
    name: &str,
    base: &Rc<RefCell<PyClass>>,
    meta: &Rc<RefCell<PyClass>>,
) -> Rc<RefCell<PyClass>> {
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("ctypes._endian"));
    attrs.insert("_swappedbytes_".to_string(), Value::none());
    let mut cls = PyClass::new(name, name, Some(Rc::clone(base)), attrs);
    cls.metatype = Some(Rc::clone(meta));
    let cls_rc = Rc::new(RefCell::new(cls));
    base.borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&cls_rc));
    cls_rc
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Ensure `_ctypes` core classes exist.
    let _ = interp.load_module("_ctypes")?;
    let core = core_classes()?;

    let (le_struct, be_struct, le_union, be_union, other_attr) = if cfg!(target_endian = "little") {
        let be_s = make_swapped_base("BigEndianStructure", &core.struct_base, &core.struct_meta);
        let be_u = make_swapped_base("BigEndianUnion", &core.union_base, &core.union_meta);
        (
            Rc::clone(&core.struct_base),
            be_s,
            Rc::clone(&core.union_base),
            be_u,
            "__ctype_be__",
        )
    } else {
        let le_s = make_swapped_base(
            "LittleEndianStructure",
            &core.struct_base,
            &core.struct_meta,
        );
        let le_u = make_swapped_base("LittleEndianUnion", &core.union_base, &core.union_meta);
        (
            le_s,
            Rc::clone(&core.struct_base),
            le_u,
            Rc::clone(&core.union_base),
            "__ctype_le__",
        )
    };

    let mut m = module.borrow_mut();
    m.insert_attr("_OTHER_ENDIAN".to_string(), Value::string(other_attr));
    m.insert_attr(
        "Structure".to_string(),
        Value::py_class(Rc::clone(&core.struct_base)),
    );
    m.insert_attr(
        "Union".to_string(),
        Value::py_class(Rc::clone(&core.union_base)),
    );
    m.insert_attr(
        "LittleEndianStructure".to_string(),
        Value::py_class(le_struct),
    );
    m.insert_attr("BigEndianStructure".to_string(), Value::py_class(be_struct));
    m.insert_attr("LittleEndianUnion".to_string(), Value::py_class(le_union));
    m.insert_attr("BigEndianUnion".to_string(), Value::py_class(be_union));
    Ok(())
}

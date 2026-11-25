// `ipaddress` module — body for the `ipaddress @inject` entry in
// `pyrust_builtin_modules!` (issue #3205).
//
// CPython 3.12's `ipaddress` is a pure-Python module providing IPv4/IPv6
// address, network, and interface manipulation classes and helpers.
//
// Reference: <https://docs.python.org/3/library/ipaddress.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `ipaddress` module.
const IPADDRESS_PY_SOURCE: &str = include_str!("ipaddress_py.py");

/// Public names exported from `IPADDRESS_PY_SOURCE` onto the `ipaddress` module.
const IPADDRESS_PY_EXPORTS: &[&str] = &[
    "IPV4LENGTH",
    "IPV6LENGTH",
    "AddressValueError",
    "NetmaskValueError",
    "ip_address",
    "ip_network",
    "ip_interface",
    "v4_int_to_packed",
    "v6_int_to_packed",
    "summarize_address_range",
    "collapse_addresses",
    "get_mixed_type_key",
    "IPv4Address",
    "IPv4Interface",
    "IPv4Network",
    "IPv6Address",
    "IPv6Interface",
    "IPv6Network",
];

/// Execute `IPADDRESS_PY_SOURCE` once and copy its exported attributes onto the
/// `ipaddress` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source_with_filename(
        IPADDRESS_PY_SOURCE,
        "<frozen ipaddress>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("ipaddress: exec namespace not a dict".into()))?;
    for &name in IPADDRESS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("ipaddress"));
                class.error_name = match name {
                    "AddressValueError" => Some("ipaddress.AddressValueError"),
                    "NetmaskValueError" => Some("ipaddress.NetmaskValueError"),
                    _ => None,
                };
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

pyrust_module! {}

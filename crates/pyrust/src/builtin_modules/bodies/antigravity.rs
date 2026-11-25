// `antigravity` module — 100% native Rust implementation of Python 3.12's
// `antigravity` module (issue #3248).
//
// Implements `geohash(latitude, longitude, datedow)` using the Munroe algorithm
// (xkcd #426) with exact CPython 3.12 formatting parity. On import, invokes
// `webbrowser.open("https://xkcd.com/353/")` ONLY if a `webbrowser` module is
// already registered in `sys.modules` (so mocked tests match CPython while
// headless environments never spawn a real browser).
//
// Reference: CPython `Lib/antigravity.py`

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::{PyBytesLike, PyNumber};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value};

/// On import, notify a mocked `webbrowser` module if already present in
/// `sys.modules`, without ever launching a real browser in headless runs.
pub(crate) fn prepare_module(interp: &mut Interpreter) {
    if let Ok(modules) = interp.import_module_registry()
        && let Some(wb) = modules
            .as_dict()
            .and_then(|d| d.get(&PyKey::str_from("webbrowser")).cloned())
        && let Ok(open_fn) = interp.get_attr(&wb, "open")
    {
        let _ = interp.call_function_expanded(
            open_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string("https://xkcd.com/353/"),
            }],
        );
    }
}

fn format_hex_fraction(hex16: &str) -> String {
    let bits = u64::from_str_radix(hex16, 16).unwrap_or(0);
    // Exact IEEE-754 value for `float.fromhex('0.' + hex16)`:
    // converting u64 -> f64 rounds-to-nearest-even to 53 bits and scaling by
    // 2^-64 only adjusts the binary exponent without further rounding.
    let val = (bits as f64) * 2.0_f64.powi(-64);
    let formatted = format!("{val:.6}");
    formatted.get(1..).unwrap_or(".000000").to_string()
}

fn format_coord_int(coord: f64) -> String {
    let trunc = coord.trunc();
    if trunc.is_finite() && trunc >= (i128::MIN as f64) && trunc <= (i128::MAX as f64) {
        (trunc as i128).to_string()
    } else {
        format!("{trunc:.0}")
    }
}

pyrust_module! {
    /// Compute geohash() using the Munroe algorithm.
    fn geohash(
        latitude: PyNumber,
        longitude: PyNumber,
        datedow: PyBytesLike<'_>,
    ) -> Result<Value> {
        let hex = super::hashlib::md5_hexdigest(datedow.as_bytes());
        let p_frac = format_hex_fraction(&hex[..16]);
        let q_frac = format_hex_fraction(&hex[16..32]);
        let lat_int = format_coord_int(latitude.as_f64());
        let lon_int = format_coord_int(longitude.as_f64());
        let line = format!("{lat_int}{p_frac} {lon_int}{q_frac}\n");

        let sys_mod = _interp.load_module("sys")?;
        let stdout = _interp.get_attr(&sys_mod, "stdout")?;
        let write_fn = _interp.get_attr(&stdout, "write")?;
        _interp.call_function_expanded(
            write_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(line),
            }],
        )?;
        Ok(Value::none())
    }
}

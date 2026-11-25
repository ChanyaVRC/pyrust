//! Declarative macro generating all standard `encodings.<codec>` submodules
//! 100% natively in Rust (issue #3250).
//!
//! Each generated submodule delegates to PyRust's `codecs` registry and exposes:
//! `getregentry()`, `encode`, `decode`, `Codec`, `IncrementalEncoder`,
//! `IncrementalDecoder`, `StreamReader`, and `StreamWriter`.

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_registry::BuiltinReg;
use crate::error::Result;
use crate::interpreter::builtin_args::{PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn lookup_codec_info(interp: &mut Interpreter, canonical_name: &str) -> Result<Value> {
    let codecs_mod = interp.load_module("codecs")?;
    let lookup_fn = interp.get_attr(&codecs_mod, "lookup")?;
    call_positional(interp, lookup_fn, &[Value::string(canonical_name)])
}

fn populate_codec_submodule(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
    canonical_name: &str,
) -> Result<()> {
    let info = lookup_codec_info(interp, canonical_name)?;
    let encode_fn = interp.get_attr(&info, "encode")?;
    let decode_fn = interp.get_attr(&info, "decode")?;
    let inc_enc = interp.get_attr(&info, "incrementalencoder")?;
    let inc_dec = interp.get_attr(&info, "incrementaldecoder")?;
    let stream_reader = interp.get_attr(&info, "streamreader")?;
    let stream_writer = interp.get_attr(&info, "streamwriter")?;

    // Link `Codec` base class to `codecs.Codec`.
    let codecs_mod = interp.load_module("codecs")?;
    let codecs_codec = interp.get_attr(&codecs_mod, "Codec")?;
    if let Some(local_codec) = module.borrow().get_attr_value("Codec")
        && let (ValueKind::PyClass(lc), ValueKind::PyClass(bc)) =
            (local_codec.kind(), codecs_codec.kind())
    {
        link_class_bases(lc, vec![Rc::clone(bc)]);
    }

    let mut m = module.borrow_mut();
    m.insert_attr("encode".to_string(), encode_fn);
    m.insert_attr("decode".to_string(), decode_fn);
    m.insert_attr("IncrementalEncoder".to_string(), inc_enc);
    m.insert_attr("IncrementalDecoder".to_string(), inc_dec);
    m.insert_attr("StreamReader".to_string(), stream_reader);
    m.insert_attr("StreamWriter".to_string(), stream_writer);
    Ok(())
}

macro_rules! define_encoding_codec_submodules {
    (
        $(
            $mod_ident:ident => ($py_name:literal, $canonical:literal)
        ),* $(,)?
    ) => {
        $(
            pub mod $mod_ident {
                use super::*;

                pub(crate) const MODULE_NAME: &str = $py_name;
                #[allow(dead_code)]
                pub(crate) const FN_PREFIX: &str = concat!($py_name, ".");
                const CANONICAL_NAME: &str = $canonical;

                pyrust_module! {
                    class Codec {
                        fn encode(
                            #[positional_only] _self: PyValueRef<'_>,
                            input: PyValueRef<'_>,
                            #[default("strict".into())] errors: PyStr<'_>,
                        ) -> Result<Value> {
                            let info = lookup_codec_info(_interp, CANONICAL_NAME)?;
                            let enc = _interp.get_attr(&info, "encode")?;
                            call_positional(
                                _interp,
                                enc,
                                &[input.0.clone(), Value::string(&*errors)],
                            )
                        }

                        fn decode(
                            #[positional_only] _self: PyValueRef<'_>,
                            input: PyValueRef<'_>,
                            #[default("strict".into())] errors: PyStr<'_>,
                        ) -> Result<Value> {
                            let info = lookup_codec_info(_interp, CANONICAL_NAME)?;
                            let dec = _interp.get_attr(&info, "decode")?;
                            call_positional(
                                _interp,
                                dec,
                                &[input.0.clone(), Value::string(&*errors)],
                            )
                        }
                    }

                    fn getregentry() -> Result<Value> {
                        lookup_codec_info(_interp, CANONICAL_NAME)
                    }
                }
            }
        )*

        pub(crate) fn all_regs() -> Vec<BuiltinReg> {
            let mut all = Vec::new();
            $(
                all.extend_from_slice($mod_ident::regs());
            )*
            all
        }

        pub(crate) fn load_builtin_module(name: &str) -> Option<Value> {
            match name {
                $(
                    $py_name => Some($mod_ident::module()),
                )*
                _ => None,
            }
        }

        pub(crate) fn is_builtin_module(name: &str) -> bool {
            matches!(name, $($py_name)|*)
        }

        pub(crate) fn post_load_init(
            name: &str,
            interp: &mut Interpreter,
            module: &Rc<RefCell<PyModule>>,
        ) -> Result<bool> {
            match name {
                $(
                    $py_name => {
                        populate_codec_submodule(interp, module, $canonical)?;
                        Ok(true)
                    }
                )*
                _ => Ok(false),
            }
        }
    };
}

define_encoding_codec_submodules! {
    encodings_utf_8 => ("encodings.utf_8", "utf-8"),
    encodings_utf_8_sig => ("encodings.utf_8_sig", "utf-8-sig"),
    encodings_ascii => ("encodings.ascii", "ascii"),
    encodings_latin_1 => ("encodings.latin_1", "iso8859-1"),
    encodings_utf_16 => ("encodings.utf_16", "utf-16"),
    encodings_utf_16_le => ("encodings.utf_16_le", "utf-16-le"),
    encodings_utf_16_be => ("encodings.utf_16_be", "utf-16-be"),
    encodings_utf_32 => ("encodings.utf_32", "utf-32"),
    encodings_utf_32_le => ("encodings.utf_32_le", "utf-32-le"),
    encodings_utf_32_be => ("encodings.utf_32_be", "utf-32-be"),
    encodings_cp1252 => ("encodings.cp1252", "cp1252"),
    encodings_raw_unicode_escape => ("encodings.raw_unicode_escape", "raw-unicode-escape"),
    encodings_unicode_escape => ("encodings.unicode_escape", "unicode-escape"),
    encodings_utf_7 => ("encodings.utf_7", "utf-7"),
    encodings_hex_codec => ("encodings.hex_codec", "hex"),
    encodings_base64_codec => ("encodings.base64_codec", "base64"),
    encodings_rot_13 => ("encodings.rot_13", "rot-13"),
    encodings_zlib_codec => ("encodings.zlib_codec", "zlib"),
}

// `_frozen_importlib_external` module — thin 100% native-Rust shim exposing
// CPython 3.12's external/filesystem import machinery (`PathFinder`,
// `FileFinder`, `SourceFileLoader`, `SourcelessFileLoader`,
// `ExtensionFileLoader`, `NamespaceLoader`, `spec_from_file_location`,
// `cache_from_source`, `source_from_cache`, `decode_source`, `MAGIC_NUMBER`,
// and suffix constants).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::PyModule;

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let machinery = interp.load_module("importlib.machinery")?;
    for name in [
        "PathFinder",
        "FileFinder",
        "SourceFileLoader",
        "SourcelessFileLoader",
        "ExtensionFileLoader",
        "NamespaceLoader",
        "SOURCE_SUFFIXES",
        "BYTECODE_SUFFIXES",
        "DEBUG_BYTECODE_SUFFIXES",
        "OPTIMIZED_BYTECODE_SUFFIXES",
        "EXTENSION_SUFFIXES",
        "ALL_SUFFIXES",
    ] {
        if let Ok(val) = interp.get_attr(&machinery, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    let util = interp.load_module("importlib.util")?;
    for name in [
        "spec_from_file_location",
        "cache_from_source",
        "source_from_cache",
        "decode_source",
        "MAGIC_NUMBER",
    ] {
        if let Ok(val) = interp.get_attr(&util, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    Ok(())
}

pyrust_module! {}

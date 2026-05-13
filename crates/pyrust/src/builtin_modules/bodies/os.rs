// `os` module — parent package for `os.path`.
//
// This module exists primarily so that `import os.path` followed by
// `os.path.join(...)` works: pyrust's compiler binds dotted imports
// under their topmost component (CPython package semantics), so it
// needs a real `os` value to look up `path` on.  Beyond that wire-up,
// `os.sep` is exposed as the platform path separator — useful enough
// to ship even though the rest of `os` (`getcwd`, `environ`, `listdir`,
// …) is still out of scope.
//
// ## Submodule identity
//
// The `path` constant below evaluates `super::os_path::module()`,
// which builds a *fresh* `os.path` PyModule every time `os.module()`
// runs — bypassing the interpreter's `module_cache`.  That would
// diverge from CPython (`os.path is direct_os_path` returning False
// for two independent imports), so `Interpreter::load_module` in
// `runtime/env.rs` has a post-processing step that replaces every
// submodule-shaped attr with its cached version after first build.
// The net effect: `os.path` and `import os.path as direct` always
// share identity, regardless of which name the user imported first.
//
// Reference: <https://docs.python.org/3/library/os.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "sep" => Value::string(std::path::MAIN_SEPARATOR.to_string()),
        // Submodule binding — exposed so `import os.path; os.path.join(...)`
        // resolves the `path` attribute on the `os` package value.
        "path" => super::os_path::module(),
    }
}

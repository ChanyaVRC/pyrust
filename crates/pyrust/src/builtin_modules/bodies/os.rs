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
// The `path` attribute is bound to the `os.path` module at module-
// construction time via the `constants` block — every `import os` call
// re-runs `module()` which re-runs `super::os_path::module()`, but
// `module_cache` ensures both modules are only built once per
// interpreter session.
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

// `urllib` package module — exposes `urllib.parse` (issue #3149).
//
// Follows PyRust's built-in package/submodule pattern (`os` + `os.path`,
// `collections` + `collections.abc`): declaring `"parse"` as a `PyModule`
// constant lets `Interpreter::load_module` stitch it to the cached
// `sys.modules["urllib.parse"]` object so `import urllib.parse`,
// `from urllib import parse`, and `from urllib.parse import ...` all share
// exact module identity.
//
// Reference: <https://docs.python.org/3/library/urllib.html>

use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "parse" => super::urllib_parse::module(),
    }
}

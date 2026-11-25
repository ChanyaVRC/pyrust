// `urllib` package module — exposes `parse`, `response`, `error`, and `request`
// (issues #3149, #3223).
//
// Follows PyRust's built-in package/submodule pattern (`os` + `os.path`,
// `collections` + `collections.abc`, `http` + `http.*`): declaring each
// submodule as a `PyModule` constant lets `Interpreter::load_module` stitch
// it to the cached `sys.modules["urllib.*"]` object so `import urllib.*`,
// `from urllib import *`, and `from urllib.* import ...` all share exact
// module identity. Order is topological (`parse` and `response` before `error`,
// and all three before `request`), and `urllib.request` defers `http.client` /
// `email` / `mimetypes` imports until call time to avoid any import cycles.
//
// Reference: <https://docs.python.org/3/library/urllib.html>

use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "parse" => super::urllib_parse::module(),
        "response" => super::urllib_response::module(),
        "error" => super::urllib_error::module(),
        "request" => super::urllib_request::module(),
    }
}

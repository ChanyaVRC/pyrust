// `urllib` package module (issues #3149, #3223).
//
// Submodules (`parse`, `response`, `error`, `request`, `robotparser`) are
// loaded lazily on explicit import, matching CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/urllib.html>

use pyrust_derive::pyrust_module;

pyrust_module! {}

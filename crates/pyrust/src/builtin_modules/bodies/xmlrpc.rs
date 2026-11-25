// `xmlrpc` package module — 100% native-Rust Python 3.12 XML-RPC package
// (issue #3256).
//
// Exposes submodules `client` and `server`.
//
// Reference: <https://docs.python.org/3/library/xmlrpc.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("xmlrpc"),
        "client" => super::xmlrpc_client::module(),
        "server" => super::xmlrpc_server::module(),
    }
}

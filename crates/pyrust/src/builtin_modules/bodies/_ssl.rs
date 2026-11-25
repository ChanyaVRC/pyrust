// `_ssl` module — 100% native-Rust internal TLS extension backing `ssl`
// (issue #3252). Re-exports core exceptions, types, feature flags, and
// constants with exact object identity to `ssl`.

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::ssl_engine::{
    PURPOSE_CLIENT_AUTH_NID, PURPOSE_CLIENT_AUTH_OID, PURPOSE_SERVER_AUTH_NID,
    PURPOSE_SERVER_AUTH_OID,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyBool, PyIndex, PyStr};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

const SSL_REEXPORTS: &[&str] = &[
    "SSLError",
    "SSLZeroReturnError",
    "SSLWantReadError",
    "SSLWantWriteError",
    "SSLSyscallError",
    "SSLEOFError",
    "SSLCertVerificationError",
    "MemoryBIO",
    "SSLObject",
    "CERT_NONE",
    "CERT_OPTIONAL",
    "CERT_REQUIRED",
    "PROTOCOL_TLS",
    "PROTOCOL_SSLv23",
    "PROTOCOL_TLSv1",
    "PROTOCOL_TLSv1_1",
    "PROTOCOL_TLSv1_2",
    "PROTOCOL_TLS_CLIENT",
    "PROTOCOL_TLS_SERVER",
    "OP_ALL",
    "OP_NO_SSLv2",
    "OP_NO_SSLv3",
    "OP_NO_TLSv1",
    "OP_NO_TLSv1_1",
    "OP_NO_TLSv1_2",
    "OP_NO_TLSv1_3",
    "OP_NO_COMPRESSION",
    "OP_CIPHER_SERVER_PREFERENCE",
    "OP_SINGLE_DH_USE",
    "OP_SINGLE_ECDH_USE",
    "OP_NO_TICKET",
    "OP_NO_RENEGOTIATION",
    "OP_ENABLE_MIDDLEBOX_COMPAT",
    "OP_IGNORE_UNEXPECTED_EOF",
    "OP_LEGACY_SERVER_CONNECT",
    "VERIFY_DEFAULT",
    "VERIFY_CRL_CHECK_LEAF",
    "VERIFY_CRL_CHECK_CHAIN",
    "VERIFY_X509_STRICT",
    "VERIFY_ALLOW_PROXY_CERTS",
    "VERIFY_X509_TRUSTED_FIRST",
    "VERIFY_X509_PARTIAL_CHAIN",
    "HAS_SNI",
    "HAS_ALPN",
    "HAS_ECDH",
    "HAS_NPN",
    "HAS_SSLv2",
    "HAS_SSLv3",
    "HAS_TLSv1",
    "HAS_TLSv1_1",
    "HAS_TLSv1_2",
    "HAS_TLSv1_3",
    "HAS_NEVER_CHECK_COMMON_NAME",
    "HAS_PHA",
    "HAS_PSK",
    "OPENSSL_VERSION",
    "OPENSSL_VERSION_INFO",
    "OPENSSL_VERSION_NUMBER",
    "SSL_ERROR_SSL",
    "SSL_ERROR_WANT_READ",
    "SSL_ERROR_WANT_WRITE",
    "SSL_ERROR_WANT_X509_LOOKUP",
    "SSL_ERROR_SYSCALL",
    "SSL_ERROR_ZERO_RETURN",
    "SSL_ERROR_WANT_CONNECT",
    "SSL_ERROR_EOF",
    "SSL_ERROR_INVALID_ERROR_CODE",
    "get_default_verify_paths",
];

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ssl_mod = interp.load_module("ssl")?;
    for &name in SSL_REEXPORTS {
        if let Ok(val) = interp.get_attr(&ssl_mod, name) {
            module.borrow_mut().insert_attr(name.to_string(), val);
        }
    }
    if let Ok(ctx_cls) = interp.get_attr(&ssl_mod, "SSLContext") {
        module
            .borrow_mut()
            .insert_attr("_SSLContext".to_string(), ctx_cls);
    }
    if let Ok(sock_cls) = interp.get_attr(&ssl_mod, "SSLSocket") {
        module
            .borrow_mut()
            .insert_attr("_SSLSocket".to_string(), sock_cls);
    }
    Ok(())
}

pyrust_module! {
    fn txt2obj(
        string: PyStr<'_>,
        #[default(false.into())] _name: PyBool,
    ) -> Result<Value> {
        let _ = _interp;
        match string.0.as_ref() {
            PURPOSE_SERVER_AUTH_OID | "serverAuth" | "TLS Web Server Authentication" => {
                Ok(Value::tuple(vec![
                    Value::int(PURPOSE_SERVER_AUTH_NID),
                    Value::string("serverAuth"),
                    Value::string("TLS Web Server Authentication"),
                    Value::string(PURPOSE_SERVER_AUTH_OID),
                ]))
            }
            PURPOSE_CLIENT_AUTH_OID | "clientAuth" | "TLS Web Client Authentication" => {
                Ok(Value::tuple(vec![
                    Value::int(PURPOSE_CLIENT_AUTH_NID),
                    Value::string("clientAuth"),
                    Value::string("TLS Web Client Authentication"),
                    Value::string(PURPOSE_CLIENT_AUTH_OID),
                ]))
            }
            other => Err(PyError::named(
                "ValueError",
                format!("unknown object '{other}'"),
            )),
        }
    }

    fn nid2obj(nid: PyIndex) -> Result<Value> {
        let _ = _interp;
        match nid.expect_i64(FN_NAME, "nid")? {
            PURPOSE_SERVER_AUTH_NID => Ok(Value::tuple(vec![
                Value::int(PURPOSE_SERVER_AUTH_NID),
                Value::string("serverAuth"),
                Value::string("TLS Web Server Authentication"),
                Value::string(PURPOSE_SERVER_AUTH_OID),
            ])),
            PURPOSE_CLIENT_AUTH_NID => Ok(Value::tuple(vec![
                Value::int(PURPOSE_CLIENT_AUTH_NID),
                Value::string("clientAuth"),
                Value::string("TLS Web Client Authentication"),
                Value::string(PURPOSE_CLIENT_AUTH_OID),
            ])),
            other => Err(PyError::named(
                "ValueError",
                format!("unknown NID {other}"),
            )),
        }
    }
}

// `ssl` module — 100% native-Rust Python 3.12 TLS/SSL wrapper built on
// PyRust's expanded macro toolkit and pure-Rust `rustls` (issue #3252).
//
// ZERO OpenSSL C dependency, ZERO `@inject`, ZERO `*_py.py` files.
//
// Reference: <https://docs.python.org/3.12/library/ssl.html>

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use pyrust_derive::pyrust_module;
use rustls::Connection;

use crate::builtin_modules::macros::native_state::{
    has_native_state, set_native_state, with_native_state, with_native_state_mut,
};
use crate::builtin_modules::ssl_engine::{
    self, CERT_NONE, CERT_OPTIONAL, CERT_REQUIRED, MemoryBioInner, MemoryBioState,
    OPENSSL_VERSION_NUMBER_VAL, OPENSSL_VERSION_STR, PROTOCOL_TLS, PROTOCOL_TLS_CLIENT,
    PROTOCOL_TLS_SERVER, PROTOCOL_TLSV1, PROTOCOL_TLSV1_1, PROTOCOL_TLSV1_2,
    PURPOSE_CLIENT_AUTH_OID, PURPOSE_SERVER_AUTH_NID, PURPOSE_SERVER_AUTH_OID, SSL_ERROR_EOF,
    SSL_ERROR_INVALID_ERROR_CODE, SSL_ERROR_SSL, SSL_ERROR_SYSCALL, SSL_ERROR_WANT_CONNECT,
    SSL_ERROR_WANT_READ, SSL_ERROR_WANT_WRITE, SSL_ERROR_WANT_X509_LOOKUP, SSL_ERROR_ZERO_RETURN,
    SslContextState, TLS_VERSION_MAXIMUM_SUPPORTED, TLS_VERSION_MINIMUM_SUPPORTED,
    TLS_VERSION_SSLV3, TLS_VERSION_TLSV1, TLS_VERSION_TLSV1_1, TLS_VERSION_TLSV1_2,
    TLS_VERSION_TLSV1_3,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyPath, PyStr, PyValue,
    PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

// ── Struct-sequences, IntEnums, and IntFlags ─────────────────────────────────

crate::pyrust_struct_seq! {
    pub(crate) DefaultVerifyPaths in "ssl" {
        fields: [
            cafile,
            capath,
            openssl_cafile_env,
            openssl_cafile,
            openssl_capath_env,
            openssl_capath,
        ],
    }
}

crate::pyrust_int_enum! {
    pub(crate) VerifyMode in "ssl" {
        CERT_NONE = crate::builtin_modules::ssl_engine::CERT_NONE,
        CERT_OPTIONAL = crate::builtin_modules::ssl_engine::CERT_OPTIONAL,
        CERT_REQUIRED = crate::builtin_modules::ssl_engine::CERT_REQUIRED,
    }
}

crate::pyrust_int_enum! {
    pub(crate) TLSVersion in "ssl" {
        MINIMUM_SUPPORTED = crate::builtin_modules::ssl_engine::TLS_VERSION_MINIMUM_SUPPORTED,
        SSLv3 = crate::builtin_modules::ssl_engine::TLS_VERSION_SSLV3,
        TLSv1 = crate::builtin_modules::ssl_engine::TLS_VERSION_TLSV1,
        TLSv1_1 = crate::builtin_modules::ssl_engine::TLS_VERSION_TLSV1_1,
        TLSv1_2 = crate::builtin_modules::ssl_engine::TLS_VERSION_TLSV1_2,
        TLSv1_3 = crate::builtin_modules::ssl_engine::TLS_VERSION_TLSV1_3,
        MAXIMUM_SUPPORTED = crate::builtin_modules::ssl_engine::TLS_VERSION_MAXIMUM_SUPPORTED,
    }
}

crate::pyrust_int_enum! {
    pub(crate) _SSLMethod in "ssl" {
        PROTOCOL_TLS = crate::builtin_modules::ssl_engine::PROTOCOL_TLS,
        PROTOCOL_SSLv23 = crate::builtin_modules::ssl_engine::PROTOCOL_SSLV23,
        PROTOCOL_TLSv1 = crate::builtin_modules::ssl_engine::PROTOCOL_TLSV1,
        PROTOCOL_TLSv1_1 = crate::builtin_modules::ssl_engine::PROTOCOL_TLSV1_1,
        PROTOCOL_TLSv1_2 = crate::builtin_modules::ssl_engine::PROTOCOL_TLSV1_2,
        PROTOCOL_TLS_CLIENT = crate::builtin_modules::ssl_engine::PROTOCOL_TLS_CLIENT,
        PROTOCOL_TLS_SERVER = crate::builtin_modules::ssl_engine::PROTOCOL_TLS_SERVER,
    }
}

crate::pyrust_int_flag! {
    pub(crate) Options in "ssl" {
        OP_ALL = crate::builtin_modules::ssl_engine::OP_ALL,
        OP_NO_SSLv2 = crate::builtin_modules::ssl_engine::OP_NO_SSLV2,
        OP_NO_SSLv3 = crate::builtin_modules::ssl_engine::OP_NO_SSLV3,
        OP_NO_TLSv1 = crate::builtin_modules::ssl_engine::OP_NO_TLSV1,
        OP_NO_TLSv1_1 = crate::builtin_modules::ssl_engine::OP_NO_TLSV1_1,
        OP_NO_TLSv1_2 = crate::builtin_modules::ssl_engine::OP_NO_TLSV1_2,
        OP_NO_TLSv1_3 = crate::builtin_modules::ssl_engine::OP_NO_TLSV1_3,
        OP_NO_COMPRESSION = crate::builtin_modules::ssl_engine::OP_NO_COMPRESSION,
        OP_CIPHER_SERVER_PREFERENCE = crate::builtin_modules::ssl_engine::OP_CIPHER_SERVER_PREFERENCE,
        OP_SINGLE_DH_USE = crate::builtin_modules::ssl_engine::OP_SINGLE_DH_USE,
        OP_SINGLE_ECDH_USE = crate::builtin_modules::ssl_engine::OP_SINGLE_ECDH_USE,
        OP_NO_TICKET = crate::builtin_modules::ssl_engine::OP_NO_TICKET,
        OP_NO_RENEGOTIATION = crate::builtin_modules::ssl_engine::OP_NO_RENEGOTIATION,
        OP_ENABLE_MIDDLEBOX_COMPAT = crate::builtin_modules::ssl_engine::OP_ENABLE_MIDDLEBOX_COMPAT,
        OP_IGNORE_UNEXPECTED_EOF = crate::builtin_modules::ssl_engine::OP_IGNORE_UNEXPECTED_EOF,
        OP_LEGACY_SERVER_CONNECT = crate::builtin_modules::ssl_engine::OP_LEGACY_SERVER_CONNECT,
    }
}

crate::pyrust_int_flag! {
    pub(crate) VerifyFlags in "ssl" {
        VERIFY_DEFAULT = crate::builtin_modules::ssl_engine::VERIFY_DEFAULT,
        VERIFY_CRL_CHECK_LEAF = crate::builtin_modules::ssl_engine::VERIFY_CRL_CHECK_LEAF,
        VERIFY_CRL_CHECK_CHAIN = crate::builtin_modules::ssl_engine::VERIFY_CRL_CHECK_CHAIN,
        VERIFY_X509_STRICT = crate::builtin_modules::ssl_engine::VERIFY_X509_STRICT,
        VERIFY_ALLOW_PROXY_CERTS = crate::builtin_modules::ssl_engine::VERIFY_ALLOW_PROXY_CERTS,
        VERIFY_X509_TRUSTED_FIRST = crate::builtin_modules::ssl_engine::VERIFY_X509_TRUSTED_FIRST,
        VERIFY_X509_PARTIAL_CHAIN = crate::builtin_modules::ssl_engine::VERIFY_X509_PARTIAL_CHAIN,
    }
}

crate::pyrust_int_enum! {
    pub(crate) Purpose in "ssl" {
        SERVER_AUTH = crate::builtin_modules::ssl_engine::PURPOSE_SERVER_AUTH_NID,
        CLIENT_AUTH = crate::builtin_modules::ssl_engine::PURPOSE_CLIENT_AUTH_NID,
    }
}

// ── Native state types for SSLObject & SSLSocket ─────────────────────────────

struct SslObjectState {
    conn: Connection,
    incoming: Rc<RefCell<MemoryBioInner>>,
    outgoing: Rc<RefCell<MemoryBioInner>>,
    context: Value,
    server_side: bool,
    server_hostname: Option<String>,
    verify_mode: i64,
    handshake_done: bool,
    shutdown_sent: bool,
}

struct SslSocketState {
    conn: Option<Connection>,
    context: Value,
    server_side: bool,
    server_hostname: Option<String>,
    do_handshake_on_connect: bool,
    suppress_ragged_eofs: bool,
    verify_mode: i64,
    handshake_done: bool,
    connected: bool,
}

// ── Helper functions ─────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn ssl_mod_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("ssl")?;
    interp.get_attr(&m, name)
}

fn enum_wrap(interp: &mut Interpreter, enum_name: &str, raw: i64) -> Result<Value> {
    if let Ok(cls) = ssl_mod_attr(interp, enum_name)
        && let Ok(val) = call_pos(interp, cls, &[Value::int(raw)])
    {
        return Ok(val);
    }
    Ok(Value::int(raw))
}

fn check_purpose_arg(interp: &mut Interpreter, val: &Value) -> Result<i64> {
    let purpose_cls = ssl_mod_attr(interp, "Purpose")?;
    let is_inst = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[val.clone(), purpose_cls],
    )?;
    if !interp.truthy_value(&is_inst)? {
        return Err(PyError::named(
            "TypeError",
            format!(
                "purpose must be a Purpose enum, not {}",
                value_type_name_str(val)
            ),
        ));
    }
    let idx = PyIndex::try_from_value_with_interp(interp, val, "purpose", "purpose")?;
    idx.expect_i64("purpose", "purpose")
}

fn parse_optional_hostname(val: OptValueRef<'_>) -> Result<Option<String>> {
    match val.as_value() {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => {
            if let Some(s) = v.as_str() {
                Ok(Some(s.to_owned()))
            } else if let ValueKind::Bytes(b) = v.kind() {
                let s = std::str::from_utf8(b)
                    .map_err(|_| PyError::named("ValueError", "invalid server_hostname bytes"))?;
                Ok(Some(s.to_owned()))
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "server_hostname must be str, bytes, or None, not {}",
                        value_type_name_str(v)
                    ),
                ))
            }
        }
    }
}

fn format_peercert(conn: &Connection, verify_mode: i64, binary_form: bool) -> Result<Value> {
    let Some(der) = ssl_engine::peer_leaf_cert_der(conn) else {
        if binary_form {
            return Ok(Value::none());
        }
        return Ok(Value::dict(PyDict::default()));
    };
    if binary_form {
        return Ok(Value::bytes(der));
    }
    if verify_mode == CERT_NONE {
        // CPython 3.12 returns `{}` when peer certificate verification is disabled.
        return Ok(Value::dict(PyDict::default()));
    }
    ssl_engine::parse_cert_der_to_py_dict(&der)
}

fn format_cipher_value(conn: &Connection) -> Value {
    match ssl_engine::cipher_tuple(conn) {
        Some((name, ver, bits)) => Value::tuple(vec![
            Value::string(name),
            Value::string(ver),
            Value::int(bits),
        ]),
        None => Value::none(),
    }
}

fn format_version_value(conn: &Connection) -> Value {
    match ssl_engine::protocol_version_str(conn) {
        Some(ver) => Value::string(ver),
        None => Value::none(),
    }
}

fn format_alpn_value(conn: &Connection) -> Value {
    match ssl_engine::selected_alpn(conn) {
        Some(proto) => Value::string(proto),
        None => Value::none(),
    }
}

fn write_into_py_buffer(buf_val: &Value, data: &[u8]) -> Result<Value> {
    let (dst_rc, offset, capacity) = super::socket::extract_writable_buffer_pub(buf_val)?;
    if data.len() > capacity {
        return Err(PyError::named(
            "ValueError",
            "nbytes is greater than the length of the buffer",
        ));
    }
    let mut dst = dst_rc.borrow_mut();
    let end = offset + data.len();
    if end > dst.len() {
        return Err(PyError::named(
            "BufferError",
            "underlying buffer shrunk during read",
        ));
    }
    dst[offset..end].copy_from_slice(data);
    Ok(Value::int(data.len() as i64))
}

fn get_sock_handle(interp: &mut Interpreter, sock_val: &Value) -> Result<Value> {
    interp.get_attr(sock_val, "_handle").map_err(|_| {
        PyError::named(
            "TypeError",
            format!(
                "expected a socket object, got {}",
                value_type_name_str(sock_val)
            ),
        )
    })
}

struct SslSocketInit<'a> {
    context_val: Value,
    ctx_snapshot: &'a SslContextState,
    handle: Value,
    family: Value,
    sock_type: Value,
    proto: Value,
    server_side: bool,
    server_hostname: Option<String>,
    do_handshake_on_connect: bool,
    suppress_ragged_eofs: bool,
    connected: bool,
}

fn instantiate_ssl_socket(interp: &mut Interpreter, init: SslSocketInit<'_>) -> Result<Value> {
    let cls_val = ssl_mod_attr(interp, "SSLSocket")?;
    let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
        return Err(PyError::runtime("ssl.SSLSocket is not a class".to_string()));
    };
    let mut attrs = InstanceAttrs::new();
    attrs.insert("_handle", init.handle);
    attrs.insert("_family", init.family);
    attrs.insert("_type", init.sock_type);
    attrs.insert("_proto", init.proto);
    attrs.insert("_io_refs", Value::int(0));
    attrs.insert("_closed", Value::bool_(false));

    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs,
    })));

    let conn = if init.connected {
        if init.server_side {
            Some(init.ctx_snapshot.build_server_connection(interp)?)
        } else {
            Some(
                init.ctx_snapshot
                    .build_client_connection(interp, init.server_hostname.as_deref())?,
            )
        }
    } else {
        None
    };

    set_native_state(
        &inst,
        SslSocketState {
            conn,
            context: init.context_val,
            server_side: init.server_side,
            server_hostname: init.server_hostname,
            do_handshake_on_connect: init.do_handshake_on_connect,
            suppress_ragged_eofs: init.suppress_ragged_eofs,
            verify_mode: init.ctx_snapshot.verify_mode,
            handshake_done: false,
            connected: init.connected,
        },
    )?;

    if init.connected && init.do_handshake_on_connect {
        ssl_socket_do_handshake_impl(interp, &inst)?;
    }

    Ok(inst)
}

fn ssl_socket_ensure_conn(interp: &mut Interpreter, self_val: &Value) -> Result<()> {
    let needs_conn =
        with_native_state::<SslSocketState, _>(self_val, "SSLSocket", |st| Ok(st.conn.is_none()))?;
    if !needs_conn {
        return Ok(());
    }
    let (ctx_val, server_side, hostname) =
        with_native_state::<SslSocketState, _>(self_val, "SSLSocket", |st| {
            Ok((
                st.context.clone(),
                st.server_side,
                st.server_hostname.clone(),
            ))
        })?;
    let ctx_snap =
        with_native_state::<SslContextState, _>(&ctx_val, "SSLContext", |c| Ok(c.clone()))?;
    let new_conn = if server_side {
        ctx_snap.build_server_connection(interp)?
    } else {
        ctx_snap.build_client_connection(interp, hostname.as_deref())?
    };
    with_native_state_mut::<SslSocketState, _>(self_val, "SSLSocket", |st| {
        st.conn = Some(new_conn);
        st.verify_mode = ctx_snap.verify_mode;
        Ok(())
    })
}

fn ssl_socket_do_handshake_impl(interp: &mut Interpreter, self_val: &Value) -> Result<()> {
    let handle = get_sock_handle(interp, self_val)?;
    if super::socket::is_closed_handle(&handle) {
        return Err(PyError::named(
            "ValueError",
            "operation on closed SSLSocket",
        ));
    }
    ssl_socket_ensure_conn(interp, self_val)?;
    with_native_state_mut::<SslSocketState, _>(self_val, "SSLSocket", |st| {
        let Some(ref mut conn) = st.conn else {
            return Err(PyError::named("ValueError", "SSLSocket is not connected"));
        };
        ssl_engine::sock_do_handshake(interp, conn, &handle)?;
        st.handshake_done = true;
        st.connected = true;
        Ok(())
    })
}

fn ssl_socket_read_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    len_i64: i64,
    buffer: Option<&Value>,
) -> Result<Value> {
    if len_i64 < 0 {
        return Err(PyError::named("ValueError", "size should not be negative"));
    }
    let handle = get_sock_handle(interp, self_val)?;
    if super::socket::is_closed_handle(&handle) {
        return Err(PyError::named(
            "ValueError",
            "Read on closed or unwrapped SSL socket.",
        ));
    }
    let need_hs =
        with_native_state::<SslSocketState, _>(self_val, "SSLSocket", |st| Ok(!st.handshake_done))?;
    if need_hs {
        ssl_socket_do_handshake_impl(interp, self_val)?;
    }

    let max_len = if let Some(buf_v) = buffer {
        let (_, _, cap) = super::socket::extract_writable_buffer_pub(buf_v)?;
        if len_i64 == 0 {
            cap
        } else {
            let req = len_i64 as usize;
            if req > cap {
                return Err(PyError::named(
                    "ValueError",
                    "nbytes is greater than the length of the buffer",
                ));
            }
            req
        }
    } else {
        (len_i64 as usize).min(16 * 1024 * 1024)
    };

    let data = with_native_state_mut::<SslSocketState, _>(self_val, "SSLSocket", |st| {
        let suppress = st.suppress_ragged_eofs;
        let Some(ref mut conn) = st.conn else {
            return Err(PyError::named(
                "ValueError",
                "Read on closed or unwrapped SSL socket.",
            ));
        };
        ssl_engine::sock_tls_read(interp, conn, &handle, max_len, suppress)
    })?;

    if let Some(buf_v) = buffer {
        write_into_py_buffer(buf_v, &data)
    } else {
        Ok(Value::bytes(data))
    }
}

fn ssl_socket_write_impl(interp: &mut Interpreter, self_val: &Value, data: &[u8]) -> Result<usize> {
    let handle = get_sock_handle(interp, self_val)?;
    if super::socket::is_closed_handle(&handle) {
        return Err(PyError::named(
            "ValueError",
            "Write on closed or unwrapped SSL socket.",
        ));
    }
    let need_hs =
        with_native_state::<SslSocketState, _>(self_val, "SSLSocket", |st| Ok(!st.handshake_done))?;
    if need_hs {
        ssl_socket_do_handshake_impl(interp, self_val)?;
    }
    with_native_state_mut::<SslSocketState, _>(self_val, "SSLSocket", |st| {
        let Some(ref mut conn) = st.conn else {
            return Err(PyError::named(
                "ValueError",
                "Write on closed or unwrapped SSL socket.",
            ));
        };
        ssl_engine::sock_tls_write(interp, conn, &handle, data)
    })
}

fn get_default_verify_paths_impl() -> Value {
    let cafile_env = "SSL_CERT_FILE";
    let capath_env = "SSL_CERT_DIR";

    let env_cafile = std::env::var(cafile_env).ok().filter(|s| !s.is_empty());
    let env_capath = std::env::var(capath_env).ok().filter(|s| !s.is_empty());

    let default_cafile = if cfg!(windows) {
        "C:\\Program Files\\Common Files\\SSL/cert.pem"
    } else {
        "/etc/ssl/cert.pem"
    };
    let default_capath = if cfg!(windows) {
        "C:\\Program Files\\Common Files\\SSL/certs"
    } else {
        "/etc/ssl/certs"
    };

    let cafile_candidate = env_cafile.as_deref().unwrap_or(default_cafile);
    let capath_candidate = env_capath.as_deref().unwrap_or(default_capath);

    let cafile_val = if Path::new(cafile_candidate).is_file() {
        Value::string(cafile_candidate)
    } else {
        Value::none()
    };
    let capath_val = if Path::new(capath_candidate).is_dir() {
        Value::string(capath_candidate)
    } else {
        Value::none()
    };

    DefaultVerifyPaths::make([
        cafile_val,
        capath_val,
        Value::string(cafile_env),
        Value::string(default_cafile),
        Value::string(capath_env),
        Value::string(default_capath),
    ])
}

// ── Native post-load initialization ──────────────────────────────────────────

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let mod_val = Value::py_module(Rc::clone(module));

    // Install genuine Python IntEnum / IntFlag subclasses and global constants.
    VerifyMode::install_with_globals(interp, &mod_val)?;
    TLSVersion::install(interp, &mod_val)?;
    _SSLMethod::install_with_globals(interp, &mod_val)?;
    Options::install_with_globals(interp, &mod_val)?;
    VerifyFlags::install_with_globals(interp, &mod_val)?;
    let purpose_cls = Purpose::install(interp, &mod_val)?;

    // Attach `.oid` attributes to Purpose.SERVER_AUTH and Purpose.CLIENT_AUTH.
    if let Ok(sa) = interp.get_attr(&purpose_cls, "SERVER_AUTH") {
        let _ = interp.assign_attr(sa, "oid", Value::string(PURPOSE_SERVER_AUTH_OID));
    }
    if let Ok(ca) = interp.get_attr(&purpose_cls, "CLIENT_AUTH") {
        let _ = interp.assign_attr(ca, "oid", Value::string(PURPOSE_CLIENT_AUTH_OID));
    }

    // Wire `SSLSocket` to inherit from `socket.socket`.
    let ssl_sock_opt = module.borrow().attrs.get("SSLSocket").cloned();
    if let Ok(sock_mod) = interp.load_module("socket")
        && let Ok(sock_cls) = interp.get_attr(&sock_mod, "socket")
        && let Some(ssl_sock_cls) = ssl_sock_opt
        && let (ValueKind::PyClass(ss_rc), ValueKind::PyClass(base_rc)) =
            (ssl_sock_cls.kind(), sock_cls.kind())
    {
        crate::builtin_modules::macros::link_class_bases(ss_rc, vec![Rc::clone(base_rc)]);
    }

    // Mirror `_create_default_https_context = create_default_context` and
    // `SSLSocket` / `SSLObject` class attributes on `SSLContext`.
    let create_default_fn = module.borrow().attrs.get("create_default_context").cloned();
    let ssl_sock_cls = module.borrow().attrs.get("SSLSocket").cloned();
    let ssl_obj_cls = module.borrow().attrs.get("SSLObject").cloned();
    let ssl_ctx_cls = module.borrow().attrs.get("SSLContext").cloned();
    if let Some(f) = create_default_fn {
        module
            .borrow_mut()
            .insert_attr("_create_default_https_context".to_string(), f);
    }
    if let Some(ctx_v) = ssl_ctx_cls
        && let ValueKind::PyClass(ctx_rc) = ctx_v.kind()
    {
        if let Some(ss) = ssl_sock_cls {
            ctx_rc
                .borrow_mut()
                .attrs
                .insert("sslsocket_class".to_string(), ss);
        }
        if let Some(so) = ssl_obj_cls {
            ctx_rc
                .borrow_mut()
                .attrs
                .insert("sslobject_class".to_string(), so);
        }
    }

    Ok(())
}

// ── Native module definition ─────────────────────────────────────────────────

pyrust_module! {
    constants {
        "DefaultVerifyPaths" => DefaultVerifyPaths::class(),

        "HAS_SNI" => Value::bool_(true),
        "HAS_ALPN" => Value::bool_(true),
        "HAS_ECDH" => Value::bool_(true),
        "HAS_NPN" => Value::bool_(false),
        "HAS_SSLv2" => Value::bool_(false),
        "HAS_SSLv3" => Value::bool_(false),
        "HAS_TLSv1" => Value::bool_(false),
        "HAS_TLSv1_1" => Value::bool_(false),
        "HAS_TLSv1_2" => Value::bool_(true),
        "HAS_TLSv1_3" => Value::bool_(true),
        "HAS_NEVER_CHECK_COMMON_NAME" => Value::bool_(true),
        "HAS_PHA" => Value::bool_(false),
        "HAS_PSK" => Value::bool_(false),

        "OPENSSL_VERSION" => Value::string(OPENSSL_VERSION_STR),
        "OPENSSL_VERSION_INFO" => Value::tuple(vec![
            Value::int(3),
            Value::int(0),
            Value::int(0),
            Value::int(0),
            Value::int(0),
        ]),
        "OPENSSL_VERSION_NUMBER" => Value::int(OPENSSL_VERSION_NUMBER_VAL),

        "SSL_ERROR_SSL" => Value::int(SSL_ERROR_SSL),
        "SSL_ERROR_WANT_READ" => Value::int(SSL_ERROR_WANT_READ),
        "SSL_ERROR_WANT_WRITE" => Value::int(SSL_ERROR_WANT_WRITE),
        "SSL_ERROR_WANT_X509_LOOKUP" => Value::int(SSL_ERROR_WANT_X509_LOOKUP),
        "SSL_ERROR_SYSCALL" => Value::int(SSL_ERROR_SYSCALL),
        "SSL_ERROR_ZERO_RETURN" => Value::int(SSL_ERROR_ZERO_RETURN),
        "SSL_ERROR_WANT_CONNECT" => Value::int(SSL_ERROR_WANT_CONNECT),
        "SSL_ERROR_EOF" => Value::int(SSL_ERROR_EOF),
        "SSL_ERROR_INVALID_ERROR_CODE" => Value::int(SSL_ERROR_INVALID_ERROR_CODE),

        "PEM_HEADER" => Value::string("-----BEGIN CERTIFICATE-----"),
        "PEM_FOOTER" => Value::string("-----END CERTIFICATE-----"),
    }

    // ── Exception hierarchy ──────────────────────────────────────────────────

    /// An error raised by the SSL layer.
    exception SSLError(OSError);
    /// The SSL connection has been closed cleanly.
    exception SSLZeroReturnError(SSLError);
    /// Non-blocking socket needs more data to complete operation.
    exception SSLWantReadError(SSLError);
    /// Non-blocking socket needs write readiness to complete operation.
    exception SSLWantWriteError(SSLError);
    /// System error encountered during SSL operation.
    exception SSLSyscallError(SSLError);
    /// Unexpected EOF in violation of the TLS protocol.
    exception SSLEOFError(SSLError);
    /// Certificate verification failed.
    exception SSLCertVerificationError(SSLError, ValueError);

    // ── MemoryBIO ────────────────────────────────────────────────────────────

    #[native_state(MemoryBioState)]
    class MemoryBIO {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            set_state(
                self_val.0,
                MemoryBioState {
                    inner: Rc::new(RefCell::new(MemoryBioInner::new())),
                },
            )?;
            Ok(Value::none())
        }

        fn read(
            #[state] st: &MemoryBioState,
            #[default((-1_i64).into())] n: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let count = n.expect_i64(FN_NAME, "n")?;
            let limit = if count < 0 { None } else { Some(count as usize) };
            let bytes = st.inner.borrow_mut().read_bytes(limit);
            Ok(Value::bytes(bytes))
        }

        fn write(
            #[state] st: &MemoryBioState,
            b: PyBytesLike<'_>,
        ) -> Result<Value> {
            let mut inner = st.inner.borrow_mut();
            if inner.eof_written {
                drop(inner);
                return Err(ssl_engine::raise_ssl_error(
                    _interp,
                    "SSLError",
                    SSL_ERROR_SSL,
                    "",
                    "cannot write() after write_eof()",
                ));
            }
            let n = inner.write_bytes(b.as_bytes());
            Ok(Value::int(n as i64))
        }

        fn write_eof(#[state] st: &MemoryBioState) -> Result<Value> {
            let _ = _interp;
            st.inner.borrow_mut().eof_written = true;
            Ok(Value::none())
        }

        #[getter]
        fn pending(#[state] st: &MemoryBioState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(st.inner.borrow().pending() as i64))
        }

        #[getter]
        fn eof(#[state] st: &MemoryBioState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.inner.borrow().is_eof()))
        }
    }

    // ── SSLObject ────────────────────────────────────────────────────────────

    #[native_state(SslObjectState)]
    class SSLObject {
        fn __init__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named(
                "TypeError",
                "SSLObject does not have a public constructor. Instances are returned by SSLContext.wrap_bio() method.",
            ))
        }

        fn do_handshake(#[state_mut] st: &mut SslObjectState) -> Result<Value> {
            ssl_engine::bio_do_handshake(_interp, &mut st.conn, &st.incoming, &st.outgoing)?;
            st.handshake_done = true;
            Ok(Value::none())
        }

        fn read(
            #[state_mut] st: &mut SslObjectState,
            #[default(1024_i64.into())] len: PyIndex,
            #[default(OptionalArg::Omitted)] buffer: OptValueRef<'_>,
        ) -> Result<Value> {
            let len_i64 = len.expect_i64(FN_NAME, "len")?;
            if len_i64 < 0 {
                return Err(PyError::named("ValueError", "size should not be negative"));
            }
            if !st.handshake_done {
                ssl_engine::bio_do_handshake(_interp, &mut st.conn, &st.incoming, &st.outgoing)?;
                st.handshake_done = true;
            }
            let buf_opt = buffer.as_value().filter(|v| !v.is_none());
            let max_len = if let Some(buf_v) = buf_opt {
                let (_, _, cap) = super::socket::extract_writable_buffer_pub(buf_v)?;
                if len_i64 == 0 {
                    cap
                } else {
                    let req = len_i64 as usize;
                    if req > cap {
                        return Err(PyError::named(
                            "ValueError",
                            "nbytes is greater than the length of the buffer",
                        ));
                    }
                    req
                }
            } else {
                (len_i64 as usize).min(16 * 1024 * 1024)
            };

            let data = ssl_engine::bio_read(
                _interp,
                &mut st.conn,
                &st.incoming,
                &st.outgoing,
                max_len,
            )?;
            if let Some(buf_v) = buf_opt {
                write_into_py_buffer(buf_v, &data)
            } else {
                Ok(Value::bytes(data))
            }
        }

        fn write(
            #[state_mut] st: &mut SslObjectState,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            if !st.handshake_done {
                ssl_engine::bio_do_handshake(_interp, &mut st.conn, &st.incoming, &st.outgoing)?;
                st.handshake_done = true;
            }
            if st.shutdown_sent {
                return Err(ssl_engine::raise_zero_return(_interp));
            }
            let n = ssl_engine::bio_write(_interp, &mut st.conn, &st.outgoing, data.as_bytes())?;
            Ok(Value::int(n as i64))
        }

        fn unwrap(#[state_mut] st: &mut SslObjectState) -> Result<Value> {
            if !st.shutdown_sent {
                st.conn.send_close_notify();
                st.shutdown_sent = true;
            }
            ssl_engine::flush_tls_to_bio(&mut st.conn, &st.outgoing);
            // Consume any pending close_notify from incoming if already present.
            if st.incoming.borrow().pending() > 0 {
                let _ = ssl_engine::bio_read(_interp, &mut st.conn, &st.incoming, &st.outgoing, 1);
            }
            Ok(Value::none())
        }

        fn pending(#[state] _st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(0))
        }

        fn version(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Ok(Value::none());
            }
            Ok(format_version_value(&st.conn))
        }

        fn cipher(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Ok(Value::none());
            }
            Ok(format_cipher_value(&st.conn))
        }

        fn compression(#[state] _st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn selected_alpn_protocol(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Ok(Value::none());
            }
            Ok(format_alpn_value(&st.conn))
        }

        fn selected_npn_protocol(#[state] _st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn getpeercert(
            #[state] st: &SslObjectState,
            #[default(false.into())] binary_form: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Err(PyError::named("ValueError", "handshake not done yet"));
            }
            format_peercert(&st.conn, st.verify_mode, binary_form.0)
        }

        fn get_unverified_chain(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            let certs = ssl_engine::peer_cert_chain_der(&st.conn);
            if certs.is_empty() {
                return Ok(Value::none());
            }
            Ok(Value::list(certs.into_iter().map(Value::bytes).collect()))
        }

        fn get_verified_chain(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            if st.verify_mode == CERT_NONE {
                return Ok(Value::list(Vec::new()));
            }
            let certs = ssl_engine::peer_cert_chain_der(&st.conn);
            Ok(Value::list(certs.into_iter().map(Value::bytes).collect()))
        }

        #[getter]
        fn server_hostname(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            Ok(match &st.server_hostname {
                Some(h) => Value::string(h),
                None => Value::none(),
            })
        }

        #[getter]
        fn server_side(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.server_side))
        }

        #[getter]
        fn context(#[state] st: &SslObjectState) -> Result<Value> {
            let _ = _interp;
            Ok(st.context.clone())
        }

        #[setter("context")]
        fn set_context(#[state_mut] st: &mut SslObjectState, ctx: PyValue) -> Result<Value> {
            let _ = _interp;
            if !has_native_state::<SslContextState>(&ctx.0) {
                return Err(PyError::named(
                    "TypeError",
                    "context must be an SSLContext instance",
                ));
            }
            st.context = ctx.0;
            Ok(Value::none())
        }
    }

    // ── SSLSocket ────────────────────────────────────────────────────────────

    #[native_state(SslSocketState)]
    class SSLSocket {
        fn __init__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let _ = _interp;
            Err(PyError::named(
                "TypeError",
                "SSLSocket does not have a public constructor. Instances are returned by SSLContext.wrap_socket() method.",
            ))
        }

        fn connect(#[positional_only] self_val: PyValueRef<'_>, addr: PyValue) -> Result<Value> {
            let handle = get_sock_handle(_interp, self_val.0)?;
            let sock_mod = _interp.load_module("socket")?;
            let gaierror = _interp.get_attr(&sock_mod, "gaierror")?;
            super::socket::handle_connect_raw(&handle, &addr.0, &gaierror)?;
            let do_hs = with_native_state_mut::<SslSocketState, _>(self_val.0, "SSLSocket", |st| {
                st.connected = true;
                Ok(st.do_handshake_on_connect)
            })?;
            if do_hs {
                ssl_socket_do_handshake_impl(_interp, self_val.0)?;
            }
            Ok(Value::none())
        }

        fn connect_ex(#[positional_only] self_val: PyValueRef<'_>, addr: PyValue) -> Result<Value> {
            let handle = get_sock_handle(_interp, self_val.0)?;
            let sock_mod = _interp.load_module("socket")?;
            let gaierror = _interp.get_attr(&sock_mod, "gaierror")?;
            let rc = super::socket::handle_connect_ex_raw(&handle, &addr.0, &gaierror)?;
            if rc == 0 {
                let do_hs =
                    with_native_state_mut::<SslSocketState, _>(self_val.0, "SSLSocket", |st| {
                        st.connected = true;
                        Ok(st.do_handshake_on_connect)
                    })?;
                if do_hs {
                    ssl_socket_do_handshake_impl(_interp, self_val.0)?;
                }
            }
            Ok(Value::int(rc))
        }

        fn accept(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = get_sock_handle(_interp, self_val.0)?;
            let sock_mod = _interp.load_module("socket")?;
            let get_dt = _interp.get_attr(&sock_mod, "getdefaulttimeout")?;
            let default_timeout = call_pos(_interp, get_dt, &[])?;
            let (new_handle, peer_addr) =
                super::socket::handle_accept_raw(&handle, &default_timeout)?;

            let family = _interp.get_attr(self_val.0, "_family")?;
            let sock_type = _interp.get_attr(self_val.0, "_type")?;
            let proto = _interp.get_attr(self_val.0, "_proto")?;

            let (ctx_val, do_hs, suppress) =
                with_native_state::<SslSocketState, _>(self_val.0, "SSLSocket", |st| {
                    Ok((
                        st.context.clone(),
                        st.do_handshake_on_connect,
                        st.suppress_ragged_eofs,
                    ))
                })?;
            let ctx_snap =
                with_native_state::<SslContextState, _>(&ctx_val, "SSLContext", |c| Ok(c.clone()))?;

            let peer_ssl = instantiate_ssl_socket(
                _interp,
                SslSocketInit {
                    context_val: ctx_val,
                    ctx_snapshot: &ctx_snap,
                    handle: new_handle,
                    family,
                    sock_type,
                    proto,
                    server_side: true,
                    server_hostname: None,
                    do_handshake_on_connect: do_hs,
                    suppress_ragged_eofs: suppress,
                    connected: true,
                },
            )?;
            Ok(Value::tuple(vec![peer_ssl, peer_addr]))
        }

        fn do_handshake(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(false.into())] _block: PyBool,
        ) -> Result<Value> {
            ssl_socket_do_handshake_impl(_interp, self_val.0)?;
            Ok(Value::none())
        }

        fn read(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1024_i64.into())] len: PyIndex,
            #[default(OptionalArg::Omitted)] buffer: OptValueRef<'_>,
        ) -> Result<Value> {
            let len_i64 = len.expect_i64(FN_NAME, "len")?;
            let buf_opt = buffer.as_value().filter(|v| !v.is_none());
            ssl_socket_read_impl(_interp, self_val.0, len_i64, buf_opt)
        }

        fn write(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
        ) -> Result<Value> {
            let n = ssl_socket_write_impl(_interp, self_val.0, data.as_bytes())?;
            Ok(Value::int(n as i64))
        }

        fn recv(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1024_i64.into())] buflen: PyIndex,
            #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            if fl != 0 {
                return Err(PyError::named(
                    "ValueError",
                    "non-zero flags not allowed in calls to recv() on SSLSocket",
                ));
            }
            let len_i64 = buflen.expect_i64(FN_NAME, "buflen")?;
            ssl_socket_read_impl(_interp, self_val.0, len_i64, None)
        }

        fn recv_into(
            #[positional_only] self_val: PyValueRef<'_>,
            buffer: PyValue,
            #[default(OptionalArg::Omitted)] nbytes: OptValueRef<'_>,
            #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            if fl != 0 {
                return Err(PyError::named(
                    "ValueError",
                    "non-zero flags not allowed in calls to recv_into() on SSLSocket",
                ));
            }
            let req_i64 = match nbytes.as_value() {
                None => 0,
                Some(v) if v.is_none() => 0,
                Some(v) => PyIndex::try_from_value_with_interp(_interp, v, FN_NAME, "nbytes")?
                    .expect_i64(FN_NAME, "nbytes")?,
            };
            ssl_socket_read_impl(_interp, self_val.0, req_i64, Some(&buffer.0))
        }

        fn send(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
            #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            if fl != 0 {
                return Err(PyError::named(
                    "ValueError",
                    "non-zero flags not allowed in calls to send() on SSLSocket",
                ));
            }
            let n = ssl_socket_write_impl(_interp, self_val.0, data.as_bytes())?;
            Ok(Value::int(n as i64))
        }

        fn sendall(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyBytesLike<'_>,
            #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            if fl != 0 {
                return Err(PyError::named(
                    "ValueError",
                    "non-zero flags not allowed in calls to sendall() on SSLSocket",
                ));
            }
            let bytes = data.as_bytes();
            let mut sent = 0usize;
            while sent < bytes.len() {
                let n = ssl_socket_write_impl(_interp, self_val.0, &bytes[sent..])?;
                if n == 0 {
                    return Err(ssl_engine::raise_ssl_eof(
                        _interp,
                        "EOF occurred in violation of protocol",
                    ));
                }
                sent += n;
            }
            Ok(Value::none())
        }

        fn unwrap(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = get_sock_handle(_interp, self_val.0)?;
            with_native_state_mut::<SslSocketState, _>(self_val.0, "SSLSocket", |st| {
                if let Some(ref mut conn) = st.conn {
                    conn.send_close_notify();
                    let _ = ssl_engine::sock_flush_tls(_interp, conn, &handle);
                }
                st.conn = None;
                st.handshake_done = false;
                Ok(())
            })?;

            let detached = super::socket::detach_socket_handle(&handle)?;
            let family = _interp.get_attr(self_val.0, "_family")?;
            let sock_type = _interp.get_attr(self_val.0, "_type")?;
            let proto = _interp.get_attr(self_val.0, "_proto")?;
            let _ = _interp.assign_attr(self_val.0.clone(), "_closed", Value::bool_(true));

            let sock_mod = _interp.load_module("socket")?;
            let sock_cls = _interp.get_attr(&sock_mod, "socket")?;
            let call_args = [
                ExpandedCallArg {
                    name: Some("family".to_string()),
                    value: family,
                },
                ExpandedCallArg {
                    name: Some("type".to_string()),
                    value: sock_type,
                },
                ExpandedCallArg {
                    name: Some("proto".to_string()),
                    value: proto,
                },
                ExpandedCallArg {
                    name: Some("_handle".to_string()),
                    value: detached,
                },
            ];
            _interp.call_function_expanded(sock_cls, &call_args)
        }

        fn shutdown(#[positional_only] self_val: PyValueRef<'_>, how: PyIndex) -> Result<Value> {
            let how_i64 = how.expect_i64(FN_NAME, "how")?;
            let handle = get_sock_handle(_interp, self_val.0)?;
            with_native_state_mut::<SslSocketState, _>(self_val.0, "SSLSocket", |st| {
                if let Some(ref mut conn) = st.conn {
                    conn.send_close_notify();
                    let _ = ssl_engine::sock_flush_tls(_interp, conn, &handle);
                }
                Ok(())
            })?;
            super::socket::handle_shutdown_raw(&handle, how_i64)?;
            Ok(Value::none())
        }

        fn _real_close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = get_sock_handle(_interp, self_val.0)?;
            let _ = with_native_state_mut::<SslSocketState, _>(self_val.0, "SSLSocket", |st| {
                if let Some(ref mut conn) = st.conn
                    && st.handshake_done
                    && !super::socket::is_closed_handle(&handle)
                {
                    conn.send_close_notify();
                    let _ = ssl_engine::sock_flush_tls(_interp, conn, &handle);
                }
                st.conn = None;
                st.handshake_done = false;
                Ok(())
            });
            let _ = super::socket::handle_close_raw(&handle);
            Ok(Value::none())
        }

        fn pending(#[state] _st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::int(0))
        }

        fn version(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Ok(Value::none());
            }
            Ok(st
                .conn
                .as_ref()
                .map(format_version_value)
                .unwrap_or_else(Value::none))
        }

        fn cipher(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Ok(Value::none());
            }
            Ok(st
                .conn
                .as_ref()
                .map(format_cipher_value)
                .unwrap_or_else(Value::none))
        }

        fn compression(#[state] _st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn selected_alpn_protocol(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Ok(Value::none());
            }
            Ok(st
                .conn
                .as_ref()
                .map(format_alpn_value)
                .unwrap_or_else(Value::none))
        }

        fn selected_npn_protocol(#[state] _st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::none())
        }

        fn getpeercert(
            #[state] st: &SslSocketState,
            #[default(false.into())] binary_form: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            if !st.handshake_done {
                return Err(PyError::named("ValueError", "handshake not done yet"));
            }
            let Some(ref conn) = st.conn else {
                return Ok(if binary_form.0 {
                    Value::none()
                } else {
                    Value::dict(PyDict::default())
                });
            };
            format_peercert(conn, st.verify_mode, binary_form.0)
        }

        fn get_unverified_chain(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            let Some(ref conn) = st.conn else {
                return Ok(Value::none());
            };
            let certs = ssl_engine::peer_cert_chain_der(conn);
            if certs.is_empty() {
                return Ok(Value::none());
            }
            Ok(Value::list(certs.into_iter().map(Value::bytes).collect()))
        }

        fn get_verified_chain(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            if st.verify_mode == CERT_NONE {
                return Ok(Value::list(Vec::new()));
            }
            let Some(ref conn) = st.conn else {
                return Ok(Value::list(Vec::new()));
            };
            let certs = ssl_engine::peer_cert_chain_der(conn);
            Ok(Value::list(certs.into_iter().map(Value::bytes).collect()))
        }

        #[getter]
        fn server_hostname(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            Ok(match &st.server_hostname {
                Some(h) => Value::string(h),
                None => Value::none(),
            })
        }

        #[getter]
        fn server_side(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.server_side))
        }

        #[getter]
        fn context(#[state] st: &SslSocketState) -> Result<Value> {
            let _ = _interp;
            Ok(st.context.clone())
        }

        #[setter("context")]
        fn set_context(#[state_mut] st: &mut SslSocketState, ctx: PyValue) -> Result<Value> {
            let _ = _interp;
            if !has_native_state::<SslContextState>(&ctx.0) {
                return Err(PyError::named(
                    "TypeError",
                    "context must be an SSLContext instance",
                ));
            }
            st.context = ctx.0;
            Ok(Value::none())
        }
    }

    // ── SSLContext ───────────────────────────────────────────────────────────

    #[native_state(SslContextState)]
    class SSLContext {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(PROTOCOL_TLS_CLIENT.into())] protocol: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let proto_val = protocol.expect_i64(FN_NAME, "protocol")?;
            let state = SslContextState::new(proto_val)?;
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn protocol(#[state] st: &SslContextState) -> Result<Value> {
            enum_wrap(_interp, "_SSLMethod", st.protocol)
        }

        #[getter]
        fn verify_mode(#[state] st: &SslContextState) -> Result<Value> {
            enum_wrap(_interp, "VerifyMode", st.verify_mode)
        }

        #[setter("verify_mode")]
        fn set_verify_mode(
            #[state_mut] st: &mut SslContextState,
            val: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let v = val.expect_i64(FN_NAME, "verify_mode")?;
            if !matches!(v, CERT_NONE | CERT_OPTIONAL | CERT_REQUIRED) {
                return Err(PyError::named(
                    "ValueError",
                    "invalid value for verify_mode",
                ));
            }
            if v == CERT_NONE && st.check_hostname {
                return Err(PyError::named(
                    "ValueError",
                    "Cannot set verify_mode to CERT_NONE when check_hostname is enabled.",
                ));
            }
            st.verify_mode = v;
            Ok(Value::none())
        }

        #[getter]
        fn check_hostname(#[state] st: &SslContextState) -> Result<Value> {
            let _ = _interp;
            Ok(Value::bool_(st.check_hostname))
        }

        #[setter("check_hostname")]
        fn set_check_hostname(
            #[state_mut] st: &mut SslContextState,
            val: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            if val.0 && st.verify_mode == CERT_NONE {
                st.verify_mode = CERT_REQUIRED;
            }
            st.check_hostname = val.0;
            Ok(Value::none())
        }

        #[getter]
        fn minimum_version(#[state] st: &SslContextState) -> Result<Value> {
            enum_wrap(_interp, "TLSVersion", st.minimum_version)
        }

        #[setter("minimum_version")]
        fn set_minimum_version(
            #[state_mut] st: &mut SslContextState,
            val: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let v = val.expect_i64(FN_NAME, "minimum_version")?;
            if !matches!(
                v,
                TLS_VERSION_MINIMUM_SUPPORTED
                    | TLS_VERSION_SSLV3
                    | TLS_VERSION_TLSV1
                    | TLS_VERSION_TLSV1_1
                    | TLS_VERSION_TLSV1_2
                    | TLS_VERSION_TLSV1_3
                    | TLS_VERSION_MAXIMUM_SUPPORTED
            ) {
                return Err(PyError::named(
                    "ValueError",
                    format!("Unsupported protocol version {v}"),
                ));
            }
            st.minimum_version = v;
            Ok(Value::none())
        }

        #[getter]
        fn maximum_version(#[state] st: &SslContextState) -> Result<Value> {
            enum_wrap(_interp, "TLSVersion", st.maximum_version)
        }

        #[setter("maximum_version")]
        fn set_maximum_version(
            #[state_mut] st: &mut SslContextState,
            val: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            let v = val.expect_i64(FN_NAME, "maximum_version")?;
            if !matches!(
                v,
                TLS_VERSION_MINIMUM_SUPPORTED
                    | TLS_VERSION_SSLV3
                    | TLS_VERSION_TLSV1
                    | TLS_VERSION_TLSV1_1
                    | TLS_VERSION_TLSV1_2
                    | TLS_VERSION_TLSV1_3
                    | TLS_VERSION_MAXIMUM_SUPPORTED
            ) {
                return Err(PyError::named(
                    "ValueError",
                    format!("Unsupported protocol version {v}"),
                ));
            }
            st.maximum_version = v;
            Ok(Value::none())
        }

        #[getter]
        fn options(#[state] st: &SslContextState) -> Result<Value> {
            enum_wrap(_interp, "Options", st.options)
        }

        #[setter("options")]
        fn set_options(
            #[state_mut] st: &mut SslContextState,
            val: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            st.options = val.expect_i64(FN_NAME, "options")?;
            Ok(Value::none())
        }

        #[getter]
        fn verify_flags(#[state] st: &SslContextState) -> Result<Value> {
            enum_wrap(_interp, "VerifyFlags", st.verify_flags)
        }

        #[setter("verify_flags")]
        fn set_verify_flags(
            #[state_mut] st: &mut SslContextState,
            val: PyIndex,
        ) -> Result<Value> {
            let _ = _interp;
            st.verify_flags = val.expect_i64(FN_NAME, "verify_flags")?;
            Ok(Value::none())
        }

        fn load_cert_chain(
            #[state_mut] st: &mut SslContextState,
            certfile: PyPath,
            #[default(None)] keyfile: Option<PyPath>,
            #[default(OptionalArg::Omitted)] _password: OptValueRef<'_>,
        ) -> Result<Value> {
            let (certs, key) = ssl_engine::load_cert_chain_files(
                _interp,
                certfile.as_path(),
                keyfile.as_ref().map(PyPath::as_path),
            )?;
            st.cert_chain = certs;
            st.set_private_key(key);
            Ok(Value::none())
        }

        fn load_verify_locations(
            #[state_mut] st: &mut SslContextState,
            #[default(None)] cafile: Option<PyPath>,
            #[default(None)] capath: Option<PyPath>,
            #[default(OptionalArg::Omitted)] cadata: OptValueRef<'_>,
        ) -> Result<Value> {
            let cadata_val = cadata.as_value().filter(|v| !v.is_none());
            if cafile.is_none() && capath.is_none() && cadata_val.is_none() {
                return Err(PyError::named(
                    "TypeError",
                    "cafile, capath and cadata cannot be all omitted",
                ));
            }
            if let Some(cf) = cafile {
                let certs = ssl_engine::load_ca_certs_from_file(cf.as_path())?;
                if certs.is_empty() {
                    return Err(ssl_engine::raise_ssl_error(
                        _interp,
                        "SSLError",
                        SSL_ERROR_SSL,
                        "NO_CERTIFICATE_OR_CRL_FOUND",
                        "no certificates found in cafile",
                    ));
                }
                st.ca_certs.extend(certs);
            }
            if let Some(cp) = capath {
                let certs = ssl_engine::load_ca_certs_from_dir(cp.as_path())?;
                st.ca_certs.extend(certs);
            }
            if let Some(cd) = cadata_val {
                if let Some(pem_str) = cd.as_str() {
                    if !pem_str.is_ascii() {
                        return Err(PyError::named(
                            "TypeError",
                            "cadata should be an ASCII string or a bytes-like object",
                        ));
                    }
                    let certs = ssl_engine::parse_pem_certs(pem_str.as_bytes())?;
                    if certs.is_empty() {
                        return Err(ssl_engine::raise_ssl_error(
                            _interp,
                            "SSLError",
                            SSL_ERROR_SSL,
                            "NO_CERTIFICATE_OR_CRL_FOUND",
                            "no certificates found in cadata",
                        ));
                    }
                    st.ca_certs.extend(certs);
                } else {
                    let bytes_like =
                        PyBytesLike::try_from_value_with_interp(_interp, cd, FN_NAME, "cadata")?;
                    let raw = bytes_like.as_bytes();
                    if raw.is_empty() {
                        return Err(PyError::named("ValueError", "Empty certificate data"));
                    }
                    if raw.starts_with(b"-----BEGIN") {
                        let certs = ssl_engine::parse_pem_certs(raw)?;
                        st.ca_certs.extend(certs);
                    } else {
                        // Single or concatenated DER certificates.
                        st.ca_certs
                            .push(rustls::pki_types::CertificateDer::from(raw.to_vec()));
                    }
                }
            }
            Ok(Value::none())
        }

        fn load_default_certs(
            #[state_mut] st: &mut SslContextState,
            #[default(OptionalArg::Omitted)] purpose: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(p) = purpose.as_value() {
                let _ = check_purpose_arg(_interp, p)?;
            }
            st.load_default_certs();
            Ok(Value::none())
        }

        fn set_default_verify_paths(#[state_mut] st: &mut SslContextState) -> Result<Value> {
            let _ = _interp;
            st.load_default_certs();
            Ok(Value::none())
        }

        fn set_alpn_protocols(
            #[state_mut] st: &mut SslContextState,
            alpn_protocols: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&alpn_protocols.0)?;
            let mut protos = Vec::with_capacity(items.len());
            for item in items {
                let Some(s) = item.as_str() else {
                    return Err(PyError::named(
                        "TypeError",
                        "alpn_protocols elements must be str",
                    ));
                };
                if !s.is_ascii() || s.is_empty() || s.len() > 255 {
                    return Err(PyError::named(
                        "ValueError",
                        "ALPN protocol must be non-empty ASCII string of length <= 255",
                    ));
                }
                protos.push(s.as_bytes().to_vec());
            }
            st.alpn_protocols = protos;
            Ok(Value::none())
        }

        fn set_ciphers(
            #[state_mut] st: &mut SslContextState,
            cipherlist: PyStr<'_>,
        ) -> Result<Value> {
            let trimmed = cipherlist.0.trim();
            if trimmed.is_empty() {
                return Err(ssl_engine::raise_ssl_error(
                    _interp,
                    "SSLError",
                    SSL_ERROR_SSL,
                    "NO_CIPHER_MATCH",
                    "No cipher can be selected.",
                ));
            }
            st.cipher_list = Some(trimmed.to_string());
            Ok(Value::none())
        }

        fn get_ca_certs(
            #[state] st: &SslContextState,
            #[default(false.into())] binary_form: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            let mut out = Vec::with_capacity(st.ca_certs.len());
            for cert in &st.ca_certs {
                if binary_form.0 {
                    out.push(Value::bytes(cert.as_ref().to_vec()));
                } else if let Ok(dict) = ssl_engine::parse_cert_der_to_py_dict(cert.as_ref()) {
                    out.push(dict);
                }
            }
            Ok(Value::list(out))
        }

        fn cert_store_stats(#[state] st: &SslContextState) -> Result<Value> {
            let _ = _interp;
            let mut x509 = st.ca_certs.len();
            let mut x509_ca = st
                .ca_certs
                .iter()
                .filter(|c| ssl_engine::is_ca_cert_der(c.as_ref()))
                .count();
            if st.use_webpki_roots && x509 == 0 {
                let n = webpki_roots::TLS_SERVER_ROOTS.len();
                x509 = n;
                x509_ca = n;
            }
            let mut d = PyDict::default();
            d.insert(PyKey::str_from("x509"), Value::int(x509 as i64));
            d.insert(PyKey::str_from("crl"), Value::int(0));
            d.insert(PyKey::str_from("x509_ca"), Value::int(x509_ca as i64));
            Ok(Value::dict(d))
        }

        fn wrap_bio(
            #[positional_only] self_val: PyValueRef<'_>,
            incoming: PyValue,
            outgoing: PyValue,
            #[default(false.into())] server_side: PyBool,
            #[default(OptionalArg::Omitted)] server_hostname: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _session: OptValueRef<'_>,
        ) -> Result<Value> {
            if !has_native_state::<MemoryBioState>(&incoming.0) {
                return Err(PyError::named(
                    "TypeError",
                    "incoming must be a MemoryBIO instance",
                ));
            }
            if !has_native_state::<MemoryBioState>(&outgoing.0) {
                return Err(PyError::named(
                    "TypeError",
                    "outgoing must be a MemoryBIO instance",
                ));
            }
            let hostname = parse_optional_hostname(server_hostname)?;
            if server_side.0 && hostname.is_some() {
                return Err(PyError::named(
                    "ValueError",
                    "server_hostname can only be specified in client mode",
                ));
            }
            let inc_rc =
                with_native_state::<MemoryBioState, _>(&incoming.0, "MemoryBIO", |b| {
                    Ok(Rc::clone(&b.inner))
                })?;
            let out_rc =
                with_native_state::<MemoryBioState, _>(&outgoing.0, "MemoryBIO", |b| {
                    Ok(Rc::clone(&b.inner))
                })?;

            let ctx_snap = with_state(self_val.0, |c| Ok(c.clone()))?;
            let conn = if server_side.0 {
                ctx_snap.build_server_connection(_interp)?
            } else {
                ctx_snap.build_client_connection(_interp, hostname.as_deref())?
            };

            let cls_val = ssl_mod_attr(_interp, "SSLObject")?;
            let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
                return Err(PyError::runtime("ssl.SSLObject is not a class".to_string()));
            };
            let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
                class: Rc::clone(cls_rc),
                attrs: InstanceAttrs::new(),
            })));
            set_native_state(
                &inst,
                SslObjectState {
                    conn,
                    incoming: inc_rc,
                    outgoing: out_rc,
                    context: self_val.0.clone(),
                    server_side: server_side.0,
                    server_hostname: hostname,
                    verify_mode: ctx_snap.verify_mode,
                    handshake_done: false,
                    shutdown_sent: false,
                },
            )?;
            Ok(inst)
        }

        fn wrap_socket(
            #[positional_only] self_val: PyValueRef<'_>,
            sock: PyValue,
            #[default(false.into())] server_side: PyBool,
            #[default(true.into())] do_handshake_on_connect: PyBool,
            #[default(true.into())] suppress_ragged_eofs: PyBool,
            #[default(OptionalArg::Omitted)] server_hostname: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _session: OptValueRef<'_>,
        ) -> Result<Value> {
            let hostname = parse_optional_hostname(server_hostname)?;
            if server_side.0 && hostname.is_some() {
                return Err(PyError::named(
                    "ValueError",
                    "server_hostname can only be specified in client mode",
                ));
            }

            let raw_handle = get_sock_handle(_interp, &sock.0)?;
            let connected = super::socket::is_tcp_connected(&raw_handle)?;
            let family = _interp.get_attr(&sock.0, "_family")?;
            let sock_type = _interp.get_attr(&sock.0, "_type")?;
            let proto = _interp.get_attr(&sock.0, "_proto")?;

            // Validate hostname requirement before detaching the socket.
            let ctx_snap = with_state(self_val.0, |c| Ok(c.clone()))?;
            if !server_side.0 && ctx_snap.check_hostname && hostname.is_none() {
                return Err(PyError::named(
                    "ValueError",
                    "check_hostname requires server_hostname",
                ));
            }

            let owned_handle = super::socket::detach_socket_handle(&raw_handle)?;
            let _ = _interp.assign_attr(sock.0.clone(), "_closed", Value::bool_(true));

            instantiate_ssl_socket(
                _interp,
                SslSocketInit {
                    context_val: self_val.0.clone(),
                    ctx_snapshot: &ctx_snap,
                    handle: owned_handle,
                    family,
                    sock_type,
                    proto,
                    server_side: server_side.0,
                    server_hostname: hostname,
                    do_handshake_on_connect: do_handshake_on_connect.0,
                    suppress_ragged_eofs: suppress_ragged_eofs.0,
                    connected,
                },
            )
        }
    }

    // ── Module-level helper functions ────────────────────────────────────────

    fn create_default_context(
        #[default(OptionalArg::Omitted)] purpose: OptValueRef<'_>,
        #[keyword_only]
        #[default(None)]
        cafile: Option<PyPath>,
        #[keyword_only]
        #[default(None)]
        capath: Option<PyPath>,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        cadata: OptValueRef<'_>,
    ) -> Result<Value> {
        let purpose_val = match purpose.as_value() {
            Some(v) => v.clone(),
            None => {
                let p_cls = ssl_mod_attr(_interp, "Purpose")?;
                _interp.get_attr(&p_cls, "SERVER_AUTH")?
            }
        };
        let purpose_nid = check_purpose_arg(_interp, &purpose_val)?;
        let protocol = if purpose_nid == PURPOSE_SERVER_AUTH_NID {
            PROTOCOL_TLS_CLIENT
        } else {
            PROTOCOL_TLS_SERVER
        };

        let ctx_cls = ssl_mod_attr(_interp, "SSLContext")?;
        let ctx = call_pos(_interp, ctx_cls, &[Value::int(protocol)])?;

        let has_ca = cafile.is_some()
            || capath.is_some()
            || cadata.as_value().is_some_and(|v| !v.is_none());
        if has_ca {
            let load_fn = _interp.get_attr(&ctx, "load_verify_locations")?;
            let cafile_val = cafile
                .map(|p| Value::string(p.as_path().to_string_lossy()))
                .unwrap_or_else(Value::none);
            let capath_val = capath
                .map(|p| Value::string(p.as_path().to_string_lossy()))
                .unwrap_or_else(Value::none);
            let cadata_val = cadata.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(_interp, load_fn, &[cafile_val, capath_val, cadata_val])?;
        } else if purpose_nid == PURPOSE_SERVER_AUTH_NID {
            let load_def = _interp.get_attr(&ctx, "load_default_certs")?;
            call_pos(_interp, load_def, &[purpose_val])?;
        }
        Ok(ctx)
    }

    fn _create_unverified_context(
        #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
        #[keyword_only]
        #[default(CERT_NONE.into())]
        cert_reqs: PyIndex,
        #[keyword_only]
        #[default(false.into())]
        check_hostname: PyBool,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        _purpose: OptValueRef<'_>,
        #[keyword_only]
        #[default(None)]
        certfile: Option<PyPath>,
        #[keyword_only]
        #[default(None)]
        keyfile: Option<PyPath>,
        #[keyword_only]
        #[default(None)]
        cafile: Option<PyPath>,
        #[keyword_only]
        #[default(None)]
        capath: Option<PyPath>,
        #[keyword_only]
        #[default(OptionalArg::Omitted)]
        cadata: OptValueRef<'_>,
    ) -> Result<Value> {
        let proto = match protocol.as_value() {
            None => PROTOCOL_TLS_CLIENT,
            Some(v) if v.is_none() => PROTOCOL_TLS_CLIENT,
            Some(v) => PyIndex::try_from_value_with_interp(_interp, v, FN_NAME, "protocol")?
                .expect_i64(FN_NAME, "protocol")?,
        };
        let ctx_cls = ssl_mod_attr(_interp, "SSLContext")?;
        let ctx = call_pos(_interp, ctx_cls, &[Value::int(proto)])?;

        let reqs = cert_reqs.expect_i64(FN_NAME, "cert_reqs")?;
        if !check_hostname.0 {
            _interp.assign_attr(ctx.clone(), "check_hostname", Value::bool_(false))?;
            _interp.assign_attr(ctx.clone(), "verify_mode", Value::int(reqs))?;
        } else {
            _interp.assign_attr(ctx.clone(), "verify_mode", Value::int(reqs))?;
            _interp.assign_attr(ctx.clone(), "check_hostname", Value::bool_(true))?;
        }

        if let Some(cf) = certfile {
            let load_cc = _interp.get_attr(&ctx, "load_cert_chain")?;
            let kf_val = keyfile
                .map(|p| Value::string(p.as_path().to_string_lossy()))
                .unwrap_or_else(Value::none);
            call_pos(
                _interp,
                load_cc,
                &[Value::string(cf.as_path().to_string_lossy()), kf_val],
            )?;
        }
        let has_ca = cafile.is_some()
            || capath.is_some()
            || cadata.as_value().is_some_and(|v| !v.is_none());
        if has_ca {
            let load_vl = _interp.get_attr(&ctx, "load_verify_locations")?;
            let cafile_val = cafile
                .map(|p| Value::string(p.as_path().to_string_lossy()))
                .unwrap_or_else(Value::none);
            let capath_val = capath
                .map(|p| Value::string(p.as_path().to_string_lossy()))
                .unwrap_or_else(Value::none);
            let cadata_val = cadata.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(_interp, load_vl, &[cafile_val, capath_val, cadata_val])?;
        }
        Ok(ctx)
    }

    #[py_name = "DER_cert_to_PEM_cert"]
    fn der_cert_to_pem_cert(der_cert_bytes: PyBytesLike<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(ssl_engine::der_cert_to_pem_cert(
            der_cert_bytes.as_bytes(),
        )))
    }

    #[py_name = "PEM_cert_to_DER_cert"]
    fn pem_cert_to_der_cert(pem_cert_string: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        let der = ssl_engine::pem_cert_to_der_cert(&pem_cert_string.0)?;
        Ok(Value::bytes(der))
    }

    fn get_default_verify_paths() -> Result<Value> {
        let _ = _interp;
        Ok(get_default_verify_paths_impl())
    }

    fn get_protocol_name(protocol_code: PyIndex) -> Result<Value> {
        let _ = _interp;
        let code = protocol_code.expect_i64(FN_NAME, "protocol_code")?;
        let name = match code {
            PROTOCOL_TLS => "PROTOCOL_TLS",
            PROTOCOL_TLSV1 => "PROTOCOL_TLSv1",
            PROTOCOL_TLSV1_1 => "PROTOCOL_TLSv1_1",
            PROTOCOL_TLSV1_2 => "PROTOCOL_TLSv1_2",
            PROTOCOL_TLS_CLIENT => "PROTOCOL_TLS_CLIENT",
            PROTOCOL_TLS_SERVER => "PROTOCOL_TLS_SERVER",
            _ => return Ok(Value::string(format!("<unknown protocol {code}>"))),
        };
        Ok(Value::string(name))
    }
}

//! Pure-Rust TLS & X.509 engine backing PyRust's native `ssl` and `_ssl`
//! modules (issue #3252).
//!
//! Built on `rustls` (with the 100% pure-Rust `rustls-rustcrypto` crypto
//! provider — ZERO OpenSSL C dependency, ZERO C/asm crypto), `rustls-pemfile`,
//! `webpki-roots`, `rustls-native-certs`, and `x509-parser`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::{
    CertificateError, ClientConfig, ClientConnection, Connection, DigitallySignedStruct,
    ProtocolVersion, RootCertStore, ServerConfig, ServerConnection, SignatureScheme,
    SupportedCipherSuite, SupportedProtocolVersion,
};
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::{FromDer, X509Certificate, X509Name};
use x509_parser::time::ASN1Time;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value};

// ── SSL / TLS constants matching CPython 3.12 ────────────────────────────────

pub(crate) const CERT_NONE: i64 = 0;
pub(crate) const CERT_OPTIONAL: i64 = 1;
pub(crate) const CERT_REQUIRED: i64 = 2;

pub(crate) const TLS_VERSION_MINIMUM_SUPPORTED: i64 = -2;
pub(crate) const TLS_VERSION_SSLV3: i64 = 0x300;
pub(crate) const TLS_VERSION_TLSV1: i64 = 0x301;
pub(crate) const TLS_VERSION_TLSV1_1: i64 = 0x302;
pub(crate) const TLS_VERSION_TLSV1_2: i64 = 0x303;
pub(crate) const TLS_VERSION_TLSV1_3: i64 = 0x304;
pub(crate) const TLS_VERSION_MAXIMUM_SUPPORTED: i64 = -1;

pub(crate) const PROTOCOL_TLS: i64 = 2;
pub(crate) const PROTOCOL_SSLV23: i64 = 2;
pub(crate) const PROTOCOL_TLSV1: i64 = 3;
pub(crate) const PROTOCOL_TLSV1_1: i64 = 4;
pub(crate) const PROTOCOL_TLSV1_2: i64 = 5;
pub(crate) const PROTOCOL_TLS_CLIENT: i64 = 16;
pub(crate) const PROTOCOL_TLS_SERVER: i64 = 17;

pub(crate) const OP_LEGACY_SERVER_CONNECT: i64 = 0x0000_0004;
pub(crate) const OP_IGNORE_UNEXPECTED_EOF: i64 = 0x0000_0080;
pub(crate) const OP_NO_TICKET: i64 = 0x0000_4000;
pub(crate) const OP_NO_COMPRESSION: i64 = 0x0002_0000;
pub(crate) const OP_SINGLE_ECDH_USE: i64 = 0x0008_0000;
pub(crate) const OP_SINGLE_DH_USE: i64 = 0x0010_0000;
pub(crate) const OP_ENABLE_MIDDLEBOX_COMPAT: i64 = 0x0010_0000;
pub(crate) const OP_CIPHER_SERVER_PREFERENCE: i64 = 0x0040_0000;
pub(crate) const OP_NO_SSLV2: i64 = 0x0100_0000;
pub(crate) const OP_NO_SSLV3: i64 = 0x0200_0000;
pub(crate) const OP_NO_TLSV1: i64 = 0x0400_0000;
pub(crate) const OP_NO_TLSV1_2: i64 = 0x0800_0000;
pub(crate) const OP_NO_TLSV1_1: i64 = 0x1000_0000;
pub(crate) const OP_NO_TLSV1_3: i64 = 0x2000_0000;
pub(crate) const OP_NO_RENEGOTIATION: i64 = 0x4000_0000;
pub(crate) const OP_ALL: i64 = 0x8000_0BFF;

pub(crate) const VERIFY_DEFAULT: i64 = 0;
pub(crate) const VERIFY_CRL_CHECK_LEAF: i64 = 0x4;
pub(crate) const VERIFY_CRL_CHECK_CHAIN: i64 = 0xC;
pub(crate) const VERIFY_X509_STRICT: i64 = 0x20;
pub(crate) const VERIFY_ALLOW_PROXY_CERTS: i64 = 0x40;
pub(crate) const VERIFY_X509_TRUSTED_FIRST: i64 = 0x8000;
pub(crate) const VERIFY_X509_PARTIAL_CHAIN: i64 = 0x80000;

pub(crate) const PURPOSE_SERVER_AUTH_NID: i64 = 129;
pub(crate) const PURPOSE_CLIENT_AUTH_NID: i64 = 130;
pub(crate) const PURPOSE_SERVER_AUTH_OID: &str = "1.3.6.1.5.5.7.3.1";
pub(crate) const PURPOSE_CLIENT_AUTH_OID: &str = "1.3.6.1.5.5.7.3.2";

pub(crate) const SSL_ERROR_SSL: i64 = 1;
pub(crate) const SSL_ERROR_WANT_READ: i64 = 2;
pub(crate) const SSL_ERROR_WANT_WRITE: i64 = 3;
pub(crate) const SSL_ERROR_WANT_X509_LOOKUP: i64 = 4;
pub(crate) const SSL_ERROR_SYSCALL: i64 = 5;
pub(crate) const SSL_ERROR_ZERO_RETURN: i64 = 6;
pub(crate) const SSL_ERROR_WANT_CONNECT: i64 = 7;
pub(crate) const SSL_ERROR_EOF: i64 = 8;
pub(crate) const SSL_ERROR_INVALID_ERROR_CODE: i64 = 10;

pub(crate) const OPENSSL_VERSION_STR: &str = "rustls 0.23 (pure-Rust TLS, OpenSSL 3.0 compatible)";
pub(crate) const OPENSSL_VERSION_NUMBER_VAL: i64 = 0x3000_0000;

// ── Crypto provider ──────────────────────────────────────────────────────────

pub(crate) fn crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(rustls_rustcrypto::provider())
}

// ── Shared native states ─────────────────────────────────────────────────────

pub(crate) struct MemoryBioInner {
    pub(crate) buf: VecDeque<u8>,
    pub(crate) eof_written: bool,
}

impl MemoryBioInner {
    pub(crate) fn new() -> Self {
        Self {
            buf: VecDeque::new(),
            eof_written: false,
        }
    }

    pub(crate) fn read_bytes(&mut self, n: Option<usize>) -> Vec<u8> {
        let count = match n {
            None => self.buf.len(),
            Some(limit) => limit.min(self.buf.len()),
        };
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(b) = self.buf.pop_front() {
                out.push(b);
            }
        }
        out
    }

    pub(crate) fn write_bytes(&mut self, data: &[u8]) -> usize {
        self.buf.extend(data);
        data.len()
    }

    pub(crate) fn pending(&self) -> usize {
        self.buf.len()
    }

    pub(crate) fn is_eof(&self) -> bool {
        self.eof_written && self.buf.is_empty()
    }
}

pub(crate) struct MemoryBioState {
    pub(crate) inner: Rc<RefCell<MemoryBioInner>>,
}

#[derive(Clone)]
pub(crate) struct SslContextState {
    pub(crate) protocol: i64,
    pub(crate) verify_mode: i64,
    pub(crate) check_hostname: bool,
    pub(crate) minimum_version: i64,
    pub(crate) maximum_version: i64,
    pub(crate) options: i64,
    pub(crate) verify_flags: i64,
    pub(crate) alpn_protocols: Vec<Vec<u8>>,
    pub(crate) cipher_list: Option<String>,
    pub(crate) cert_chain: Vec<CertificateDer<'static>>,
    pub(crate) private_key: Option<Vec<u8>>,
    pub(crate) private_key_kind: Option<PrivateKeyKind>,
    pub(crate) use_webpki_roots: bool,
    pub(crate) ca_certs: Vec<CertificateDer<'static>>,
}

#[derive(Clone, Copy)]
pub(crate) enum PrivateKeyKind {
    Pkcs1,
    Pkcs8,
    Sec1,
}

impl SslContextState {
    pub(crate) fn new(protocol: i64) -> Result<Self> {
        let (verify_mode, check_hostname, min_ver, max_ver) = match protocol {
            PROTOCOL_TLS_CLIENT => (
                CERT_REQUIRED,
                true,
                TLS_VERSION_TLSV1_2,
                TLS_VERSION_MAXIMUM_SUPPORTED,
            ),
            PROTOCOL_TLS_SERVER => (
                CERT_NONE,
                false,
                TLS_VERSION_TLSV1_2,
                TLS_VERSION_MAXIMUM_SUPPORTED,
            ),
            PROTOCOL_TLS => (
                CERT_NONE,
                false,
                TLS_VERSION_TLSV1_2,
                TLS_VERSION_MAXIMUM_SUPPORTED,
            ),
            PROTOCOL_TLSV1_2 => (CERT_NONE, false, TLS_VERSION_TLSV1_2, TLS_VERSION_TLSV1_2),
            PROTOCOL_TLSV1 | PROTOCOL_TLSV1_1 => (
                CERT_NONE,
                false,
                TLS_VERSION_TLSV1_2,
                TLS_VERSION_MAXIMUM_SUPPORTED,
            ),
            _ => {
                return Err(PyError::named("ValueError", "invalid protocol version"));
            }
        };
        Ok(Self {
            protocol,
            verify_mode,
            check_hostname,
            minimum_version: min_ver,
            maximum_version: max_ver,
            options: OP_ALL | OP_NO_SSLV2 | OP_NO_SSLV3 | OP_NO_COMPRESSION,
            verify_flags: VERIFY_X509_STRICT | VERIFY_X509_TRUSTED_FIRST,
            alpn_protocols: Vec::new(),
            cipher_list: None,
            cert_chain: Vec::new(),
            private_key: None,
            private_key_kind: None,
            use_webpki_roots: false,
            ca_certs: Vec::new(),
        })
    }

    pub(crate) fn clone_private_key(&self) -> Option<PrivateKeyDer<'static>> {
        let bytes = self.private_key.as_ref()?.clone();
        match self.private_key_kind? {
            PrivateKeyKind::Pkcs1 => Some(PrivateKeyDer::Pkcs1(bytes.into())),
            PrivateKeyKind::Pkcs8 => Some(PrivateKeyDer::Pkcs8(bytes.into())),
            PrivateKeyKind::Sec1 => Some(PrivateKeyDer::Sec1(bytes.into())),
        }
    }

    pub(crate) fn set_private_key(&mut self, key: PrivateKeyDer<'static>) {
        match key {
            PrivateKeyDer::Pkcs1(k) => {
                self.private_key = Some(k.secret_pkcs1_der().to_vec());
                self.private_key_kind = Some(PrivateKeyKind::Pkcs1);
            }
            PrivateKeyDer::Pkcs8(k) => {
                self.private_key = Some(k.secret_pkcs8_der().to_vec());
                self.private_key_kind = Some(PrivateKeyKind::Pkcs8);
            }
            PrivateKeyDer::Sec1(k) => {
                self.private_key = Some(k.secret_sec1_der().to_vec());
                self.private_key_kind = Some(PrivateKeyKind::Sec1);
            }
            _ => {}
        }
    }

    pub(crate) fn enabled_protocol_versions(
        &self,
        interp: &mut Interpreter,
    ) -> Result<Vec<&'static SupportedProtocolVersion>> {
        let allow_tls13 = (self.options & OP_NO_TLSV1_3) == 0
            && (self.maximum_version == TLS_VERSION_MAXIMUM_SUPPORTED
                || self.maximum_version >= TLS_VERSION_TLSV1_3)
            && (self.minimum_version == TLS_VERSION_MINIMUM_SUPPORTED
                || self.minimum_version <= TLS_VERSION_TLSV1_3);
        let allow_tls12 = (self.options & OP_NO_TLSV1_2) == 0
            && (self.maximum_version == TLS_VERSION_MAXIMUM_SUPPORTED
                || self.maximum_version >= TLS_VERSION_TLSV1_2)
            && (self.minimum_version == TLS_VERSION_MINIMUM_SUPPORTED
                || self.minimum_version <= TLS_VERSION_TLSV1_2);

        let mut versions: Vec<&'static SupportedProtocolVersion> = Vec::with_capacity(2);
        if allow_tls13 {
            versions.push(&rustls::version::TLS13);
        }
        if allow_tls12 {
            versions.push(&rustls::version::TLS12);
        }
        if versions.is_empty() {
            return Err(raise_ssl_error(
                interp,
                "SSLError",
                SSL_ERROR_SSL,
                "NO_PROTOCOLS_AVAILABLE",
                "no protocols available",
            ));
        }
        Ok(versions)
    }

    pub(crate) fn build_root_store(&self) -> RootCertStore {
        let mut store = RootCertStore::empty();
        if self.use_webpki_roots {
            store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
        for cert in &self.ca_certs {
            let _ = store.add(cert.clone());
        }
        store
    }

    pub(crate) fn load_default_certs(&mut self) {
        self.use_webpki_roots = true;
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            if !self.ca_certs.iter().any(|c| c.as_ref() == cert.as_ref()) {
                self.ca_certs.push(cert);
            }
        }
    }

    pub(crate) fn build_client_connection(
        &self,
        interp: &mut Interpreter,
        server_hostname: Option<&str>,
    ) -> Result<Connection> {
        if self.check_hostname && server_hostname.is_none() {
            return Err(PyError::named(
                "ValueError",
                "check_hostname requires server_hostname",
            ));
        }

        let provider = crypto_provider();
        let versions = self.enabled_protocol_versions(interp)?;
        let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&versions)
            .map_err(|e| rustls_err_to_py(interp, &e))?;

        let builder = if self.verify_mode == CERT_NONE {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerifyServerCertVerifier {
                    provider: Arc::clone(&provider),
                }))
        } else {
            let root_store = self.build_root_store();
            if root_store.is_empty() {
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(RejectAllServerCertVerifier {
                        provider: Arc::clone(&provider),
                    }))
            } else {
                let webpki_verifier = WebPkiServerVerifier::builder_with_provider(
                    Arc::new(root_store),
                    Arc::clone(&provider),
                )
                .build()
                .map_err(|e| {
                    raise_ssl_error(
                        interp,
                        "SSLError",
                        SSL_ERROR_SSL,
                        "VERIFIER_BUILD_FAILED",
                        &format!("failed to build certificate verifier: {e}"),
                    )
                })?;
                if self.check_hostname {
                    builder
                        .dangerous()
                        .with_custom_certificate_verifier(webpki_verifier)
                } else {
                    builder
                        .dangerous()
                        .with_custom_certificate_verifier(Arc::new(ChainOnlyServerCertVerifier {
                            inner: webpki_verifier,
                        }))
                }
            }
        };

        let mut config = if !self.cert_chain.is_empty()
            && let Some(key) = self.clone_private_key()
        {
            builder
                .with_client_auth_cert(self.cert_chain.clone(), key)
                .map_err(|e| rustls_err_to_py(interp, &e))?
        } else {
            builder.with_no_client_auth()
        };

        if !self.alpn_protocols.is_empty() {
            config.alpn_protocols = self.alpn_protocols.clone();
        }
        // Enable SNI only when server_hostname was explicitly provided as a DNS name.
        config.enable_sni = server_hostname.is_some();

        let server_name: ServerName<'static> = match server_hostname {
            Some(host) => ServerName::try_from(host.to_string()).map_err(|_| {
                PyError::named("ValueError", format!("invalid server_hostname: {host:?}"))
            })?,
            None => ServerName::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST).into()),
        };

        let conn = ClientConnection::new(Arc::new(config), server_name)
            .map_err(|e| rustls_err_to_py(interp, &e))?;
        Ok(Connection::Client(conn))
    }

    pub(crate) fn build_server_connection(&self, interp: &mut Interpreter) -> Result<Connection> {
        let Some(key) = self.clone_private_key() else {
            return Err(raise_ssl_error(
                interp,
                "SSLError",
                SSL_ERROR_SSL,
                "NO_CERTIFICATE_OR_PRIVATE_KEY",
                "server SSLContext requires load_cert_chain() before wrapping a socket",
            ));
        };
        if self.cert_chain.is_empty() {
            return Err(raise_ssl_error(
                interp,
                "SSLError",
                SSL_ERROR_SSL,
                "NO_CERTIFICATE_OR_PRIVATE_KEY",
                "server SSLContext requires a certificate chain",
            ));
        }

        let provider = crypto_provider();
        let versions = self.enabled_protocol_versions(interp)?;
        let builder = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&versions)
            .map_err(|e| rustls_err_to_py(interp, &e))?;

        let builder = match self.verify_mode {
            CERT_NONE => builder.with_no_client_auth(),
            CERT_OPTIONAL => {
                let roots = Arc::new(self.build_root_store());
                let verifier = WebPkiClientVerifier::builder_with_provider(roots, provider)
                    .allow_unauthenticated()
                    .build()
                    .map_err(|e| {
                        raise_ssl_error(
                            interp,
                            "SSLError",
                            SSL_ERROR_SSL,
                            "VERIFIER_BUILD_FAILED",
                            &format!("failed to build client verifier: {e}"),
                        )
                    })?;
                builder.with_client_cert_verifier(verifier)
            }
            CERT_REQUIRED => {
                let roots = Arc::new(self.build_root_store());
                let verifier = WebPkiClientVerifier::builder_with_provider(roots, provider)
                    .build()
                    .map_err(|e| {
                        raise_ssl_error(
                            interp,
                            "SSLError",
                            SSL_ERROR_SSL,
                            "VERIFIER_BUILD_FAILED",
                            &format!("failed to build client verifier: {e}"),
                        )
                    })?;
                builder.with_client_cert_verifier(verifier)
            }
            _ => builder.with_no_client_auth(),
        };

        let mut config = builder
            .with_single_cert(self.cert_chain.clone(), key)
            .map_err(|e| rustls_err_to_py(interp, &e))?;

        if !self.alpn_protocols.is_empty() {
            config.alpn_protocols = self.alpn_protocols.clone();
        }

        let conn =
            ServerConnection::new(Arc::new(config)).map_err(|e| rustls_err_to_py(interp, &e))?;
        Ok(Connection::Server(conn))
    }
}

// ── Custom rustls ServerCertVerifiers ────────────────────────────────────────

#[derive(Debug)]
struct RejectAllServerCertVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for RejectAllServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Err(rustls::Error::InvalidCertificate(
            CertificateError::UnknownIssuer,
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct NoVerifyServerCertVerifier {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for NoVerifyServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct ChainOnlyServerCertVerifier {
    inner: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for ChainOnlyServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(v) => Ok(v),
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            Err(e) => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

// ── Python SSL exception helpers ─────────────────────────────────────────────

pub(crate) fn raise_ssl_error(
    interp: &mut Interpreter,
    class_name: &str,
    errno_val: i64,
    reason: &str,
    msg: &str,
) -> PyError {
    let full_msg = if reason.is_empty() {
        msg.to_string()
    } else {
        format!("[SSL: {reason}] {msg}")
    };
    let Ok(ssl_mod) = interp.load_module("ssl") else {
        return PyError::named("OSError", full_msg);
    };
    let Ok(cls) = interp.get_attr(&ssl_mod, class_name) else {
        return PyError::named("OSError", full_msg);
    };
    let args = [
        ExpandedCallArg {
            name: None,
            value: Value::int(errno_val),
        },
        ExpandedCallArg {
            name: None,
            value: Value::string(&full_msg),
        },
    ];
    let Ok(inst) = interp.call_function_expanded(cls, &args) else {
        return PyError::named("OSError", full_msg);
    };
    let _ = interp.assign_attr(inst.clone(), "library", Value::string("SSL"));
    let _ = interp.assign_attr(
        inst.clone(),
        "reason",
        if reason.is_empty() {
            Value::none()
        } else {
            Value::string(reason)
        },
    );
    PyError::raised(inst)
}

pub(crate) fn raise_cert_verification_error(
    interp: &mut Interpreter,
    verify_code: i64,
    verify_message: &str,
) -> PyError {
    let full_msg =
        format!("[SSL: CERTIFICATE_VERIFY_FAILED] certificate verify failed: {verify_message}");
    let Ok(ssl_mod) = interp.load_module("ssl") else {
        return PyError::named("OSError", full_msg);
    };
    let Ok(cls) = interp.get_attr(&ssl_mod, "SSLCertVerificationError") else {
        return PyError::named("OSError", full_msg);
    };
    let args = [
        ExpandedCallArg {
            name: None,
            value: Value::int(SSL_ERROR_SSL),
        },
        ExpandedCallArg {
            name: None,
            value: Value::string(&full_msg),
        },
    ];
    let Ok(inst) = interp.call_function_expanded(cls, &args) else {
        return PyError::named("OSError", full_msg);
    };
    let _ = interp.assign_attr(inst.clone(), "library", Value::string("SSL"));
    let _ = interp.assign_attr(
        inst.clone(),
        "reason",
        Value::string("CERTIFICATE_VERIFY_FAILED"),
    );
    let _ = interp.assign_attr(inst.clone(), "verify_code", Value::int(verify_code));
    let _ = interp.assign_attr(
        inst.clone(),
        "verify_message",
        Value::string(verify_message),
    );
    PyError::raised(inst)
}

pub(crate) fn raise_want_read(interp: &mut Interpreter) -> PyError {
    raise_ssl_error(
        interp,
        "SSLWantReadError",
        SSL_ERROR_WANT_READ,
        "",
        "The operation did not complete (read)",
    )
}

pub(crate) fn raise_want_write(interp: &mut Interpreter) -> PyError {
    raise_ssl_error(
        interp,
        "SSLWantWriteError",
        SSL_ERROR_WANT_WRITE,
        "",
        "The operation did not complete (write)",
    )
}

pub(crate) fn raise_ssl_eof(interp: &mut Interpreter, detail: &str) -> PyError {
    raise_ssl_error(
        interp,
        "SSLEOFError",
        SSL_ERROR_EOF,
        "UNEXPECTED_EOF_WHILE_READING",
        detail,
    )
}

pub(crate) fn raise_zero_return(interp: &mut Interpreter) -> PyError {
    raise_ssl_error(
        interp,
        "SSLZeroReturnError",
        SSL_ERROR_ZERO_RETURN,
        "",
        "TLS/SSL connection has been closed (EOF)",
    )
}

pub(crate) fn rustls_err_to_py(interp: &mut Interpreter, err: &rustls::Error) -> PyError {
    match err {
        rustls::Error::InvalidCertificate(cert_err) => {
            let (code, msg) = match cert_err {
                CertificateError::UnknownIssuer => (20, "unable to get local issuer certificate"),
                CertificateError::Expired | CertificateError::ExpiredContext { .. } => {
                    (10, "certificate has expired")
                }
                CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => {
                    (9, "certificate is not yet valid")
                }
                CertificateError::NotValidForName
                | CertificateError::NotValidForNameContext { .. } => {
                    (62, "Hostname mismatch, certificate is not valid for host")
                }
                CertificateError::BadSignature => (7, "certificate signature failure"),
                CertificateError::Revoked => (23, "certificate revoked"),
                CertificateError::InvalidPurpose
                | CertificateError::InvalidPurposeContext { .. } => {
                    (26, "unsupported certificate purpose")
                }
                CertificateError::BadEncoding => (13, "format error in certificate"),
                _ => (18, "certificate verify failed"),
            };
            raise_cert_verification_error(interp, code, msg)
        }
        rustls::Error::NoApplicationProtocol => raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "NO_APPLICATION_PROTOCOL",
            "no application protocol",
        ),
        rustls::Error::AlertReceived(alert) => raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "TLSV1_ALERT",
            &format!("TLS alert received: {alert:?}"),
        ),
        other => raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "SSL_ERROR",
            &other.to_string(),
        ),
    }
}

// ── PEM / DER parsing & formatting ───────────────────────────────────────────

const PEM_HEADER: &str = "-----BEGIN CERTIFICATE-----";
const PEM_FOOTER: &str = "-----END CERTIFICATE-----";

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, &'static str> {
    let mut cleaned: Vec<u8> = Vec::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        cleaned.push(b);
    }
    if !cleaned.len().is_multiple_of(4) {
        return Err("Incorrect padding");
    }
    let mut out = Vec::with_capacity((cleaned.len() / 4) * 3);
    for chunk in cleaned.as_chunks::<4>().0 {
        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                pad += 1;
                vals[i] = 0;
            } else {
                if pad > 0 {
                    return Err("Invalid base64 padding");
                }
                vals[i] = match c {
                    b'A'..=b'Z' => c - b'A',
                    b'a'..=b'z' => c - b'a' + 26,
                    b'0'..=b'9' => c - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return Err("Invalid base64 character"),
                };
            }
        }
        if pad > 2 {
            return Err("Invalid base64 padding");
        }
        out.push((vals[0] << 2) | (vals[1] >> 4));
        if pad < 2 {
            out.push((vals[1] << 4) | (vals[2] >> 2));
        }
        if pad == 0 {
            out.push((vals[2] << 6) | vals[3]);
        }
    }
    Ok(out)
}

pub(crate) fn der_cert_to_pem_cert(der: &[u8]) -> String {
    let b64 = base64_encode(der);
    let mut out = String::with_capacity(b64.len() + 64);
    out.push_str(PEM_HEADER);
    out.push('\n');
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str(PEM_FOOTER);
    out.push('\n');
    out
}

pub(crate) fn pem_cert_to_der_cert(pem: &str) -> Result<Vec<u8>> {
    if !pem.starts_with(PEM_HEADER) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid PEM encoding; must start with {PEM_HEADER}"),
        ));
    }
    let trimmed = pem.trim();
    if !trimmed.ends_with(PEM_FOOTER) {
        return Err(PyError::named(
            "ValueError",
            format!("Invalid PEM encoding; must end with {PEM_FOOTER}"),
        ));
    }
    let body = &trimmed[PEM_HEADER.len()..trimmed.len() - PEM_FOOTER.len()];
    base64_decode(body).map_err(|msg| PyError::named("ValueError", msg))
}

pub(crate) fn parse_pem_certs(bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = Cursor::new(bytes);
    let mut certs = Vec::new();
    for item in rustls_pemfile::certs(&mut reader) {
        let cert = item.map_err(|e| {
            PyError::named(
                "ValueError",
                format!("failed to parse PEM certificate: {e}"),
            )
        })?;
        certs.push(cert);
    }
    Ok(certs)
}

pub(crate) fn parse_pem_private_key(
    interp: &mut Interpreter,
    bytes: &[u8],
) -> Result<PrivateKeyDer<'static>> {
    let mut reader = Cursor::new(bytes);
    match rustls_pemfile::private_key(&mut reader) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "PEM_LIB",
            "no private key found in PEM file",
        )),
        Err(e) => Err(raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "PEM_LIB",
            &format!("failed to parse private key: {e}"),
        )),
    }
}

pub(crate) fn load_cert_chain_files(
    interp: &mut Interpreter,
    certfile: &Path,
    keyfile: Option<&Path>,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let cert_bytes = fs::read(certfile).map_err(|e| PyError::from_io_error(&e, None))?;
    let certs = parse_pem_certs(&cert_bytes)?;
    if certs.is_empty() {
        return Err(raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "PEM_LIB",
            "no certificates found in certfile",
        ));
    }
    let key_bytes = match keyfile {
        Some(kf) => fs::read(kf).map_err(|e| PyError::from_io_error(&e, None))?,
        None => cert_bytes,
    };
    let key = parse_pem_private_key(interp, &key_bytes)?;
    Ok((certs, key))
}

pub(crate) fn load_ca_certs_from_file(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let bytes = fs::read(path).map_err(|e| PyError::from_io_error(&e, None))?;
    let certs = parse_pem_certs(&bytes)?;
    Ok(certs)
}

pub(crate) fn load_ca_certs_from_dir(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let entries = fs::read_dir(path).map_err(|e| PyError::from_io_error(&e, None))?;
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|de| de.path()))
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    let mut certs = Vec::new();
    for p in paths {
        if let Ok(bytes) = fs::read(&p)
            && let Ok(found) = parse_pem_certs(&bytes)
        {
            certs.extend(found);
        }
    }
    Ok(certs)
}

// ── TLS connection metadata helpers ──────────────────────────────────────────

pub(crate) fn protocol_version_str(conn: &Connection) -> Option<&'static str> {
    match conn.protocol_version()? {
        ProtocolVersion::TLSv1_3 => Some("TLSv1.3"),
        ProtocolVersion::TLSv1_2 => Some("TLSv1.2"),
        ProtocolVersion::TLSv1_1 => Some("TLSv1.1"),
        ProtocolVersion::TLSv1_0 => Some("TLSv1"),
        ProtocolVersion::SSLv3 => Some("SSLv3"),
        _ => Some("UNKNOWN"),
    }
}

pub(crate) fn cipher_tuple(conn: &Connection) -> Option<(&'static str, &'static str, i64)> {
    let suite = conn.negotiated_cipher_suite()?;
    let ver = protocol_version_str(conn)?;
    let (name, bits) = match suite {
        SupportedCipherSuite::Tls13(s) => match s.common.suite {
            rustls::CipherSuite::TLS13_AES_256_GCM_SHA384 => ("TLS_AES_256_GCM_SHA384", 256),
            rustls::CipherSuite::TLS13_AES_128_GCM_SHA256 => ("TLS_AES_128_GCM_SHA256", 128),
            rustls::CipherSuite::TLS13_CHACHA20_POLY1305_SHA256 => {
                ("TLS_CHACHA20_POLY1305_SHA256", 256)
            }
            _ => ("TLS_AES_256_GCM_SHA384", 256),
        },
        SupportedCipherSuite::Tls12(s) => match s.common.suite {
            rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 => {
                ("ECDHE-ECDSA-AES256-GCM-SHA384", 256)
            }
            rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 => {
                ("ECDHE-ECDSA-AES128-GCM-SHA256", 128)
            }
            rustls::CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256 => {
                ("ECDHE-ECDSA-CHACHA20-POLY1305", 256)
            }
            rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 => {
                ("ECDHE-RSA-AES256-GCM-SHA384", 256)
            }
            rustls::CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 => {
                ("ECDHE-RSA-AES128-GCM-SHA256", 128)
            }
            rustls::CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256 => {
                ("ECDHE-RSA-CHACHA20-POLY1305", 256)
            }
            _ => ("ECDHE-RSA-AES256-GCM-SHA384", 256),
        },
    };
    Some((name, ver, bits))
}

pub(crate) fn selected_alpn(conn: &Connection) -> Option<String> {
    let proto = conn.alpn_protocol()?;
    Some(String::from_utf8_lossy(proto).into_owned())
}

pub(crate) fn peer_leaf_cert_der(conn: &Connection) -> Option<Vec<u8>> {
    let certs = conn.peer_certificates()?;
    certs.first().map(|c| c.as_ref().to_vec())
}

pub(crate) fn peer_cert_chain_der(conn: &Connection) -> Vec<Vec<u8>> {
    conn.peer_certificates()
        .map(|certs| certs.iter().map(|c| c.as_ref().to_vec()).collect())
        .unwrap_or_default()
}

// ── X.509 DER -> CPython certificate dict ────────────────────────────────────

fn oid_to_python_attr_name(oid: &str) -> String {
    match oid {
        "2.5.4.3" => "commonName".to_string(),
        "2.5.4.5" => "serialNumber".to_string(),
        "2.5.4.6" => "countryName".to_string(),
        "2.5.4.7" => "localityName".to_string(),
        "2.5.4.8" => "stateOrProvinceName".to_string(),
        "2.5.4.9" => "streetAddress".to_string(),
        "2.5.4.10" => "organizationName".to_string(),
        "2.5.4.11" => "organizationalUnitName".to_string(),
        "2.5.4.12" => "title".to_string(),
        "2.5.4.4" => "surname".to_string(),
        "2.5.4.42" => "givenName".to_string(),
        "1.2.840.113549.1.9.1" => "emailAddress".to_string(),
        "0.9.2342.19200300.100.1.25" => "domainComponent".to_string(),
        other => other.to_string(),
    }
}

fn x509_name_to_py(name: &X509Name<'_>) -> Value {
    let mut rdns = Vec::new();
    for rdn in name.iter() {
        let mut attrs = Vec::new();
        for attr in rdn.iter() {
            let oid_str = attr.attr_type().to_id_string();
            let key = oid_to_python_attr_name(&oid_str);
            let val = attr
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|_| String::from_utf8_lossy(attr.attr_value().data).into_owned());
            attrs.push(Value::tuple(vec![Value::string(key), Value::string(val)]));
        }
        rdns.push(Value::tuple(attrs));
    }
    Value::tuple(rdns)
}

fn format_asn1_time(t: ASN1Time) -> String {
    let dt = t.to_datetime();
    let month = match dt.month() as u8 {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "Jan",
    };
    format!(
        "{month} {:2} {:02}:{:02}:{:02} {:04} GMT",
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.year()
    )
}

pub(crate) fn parse_cert_der_to_py_dict(der: &[u8]) -> Result<Value> {
    let (_, cert) = X509Certificate::from_der(der)
        .map_err(|e| PyError::named("ValueError", format!("invalid X.509 certificate: {e}")))?;

    let mut dict = PyDict::default();
    dict.insert(PyKey::str_from("subject"), x509_name_to_py(cert.subject()));
    dict.insert(PyKey::str_from("issuer"), x509_name_to_py(cert.issuer()));
    dict.insert(
        PyKey::str_from("version"),
        Value::int(i64::from(cert.version().0) + 1),
    );
    let serial: String = cert
        .raw_serial()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    dict.insert(PyKey::str_from("serialNumber"), Value::string(serial));
    dict.insert(
        PyKey::str_from("notBefore"),
        Value::string(format_asn1_time(cert.validity().not_before)),
    );
    dict.insert(
        PyKey::str_from("notAfter"),
        Value::string(format_asn1_time(cert.validity().not_after)),
    );

    // Extract Subject Alternative Names if present.
    let mut sans: Vec<Value> = Vec::new();
    for ext in cert.extensions() {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for gn in &san.general_names {
                match gn {
                    GeneralName::DNSName(dns) => {
                        sans.push(Value::tuple(vec![
                            Value::string("DNS"),
                            Value::string(*dns),
                        ]));
                    }
                    GeneralName::IPAddress(bytes) => {
                        let ip_str = match bytes.len() {
                            4 => {
                                let arr: [u8; 4] = [bytes[0], bytes[1], bytes[2], bytes[3]];
                                Ipv4Addr::from(arr).to_string()
                            }
                            16 => {
                                let mut arr = [0u8; 16];
                                arr.copy_from_slice(bytes);
                                Ipv6Addr::from(arr).to_string()
                            }
                            _ => format!("{bytes:?}"),
                        };
                        sans.push(Value::tuple(vec![
                            Value::string("IP Address"),
                            Value::string(ip_str),
                        ]));
                    }
                    GeneralName::RFC822Name(email) => {
                        sans.push(Value::tuple(vec![
                            Value::string("email"),
                            Value::string(*email),
                        ]));
                    }
                    GeneralName::URI(uri) => {
                        sans.push(Value::tuple(vec![
                            Value::string("URI"),
                            Value::string(*uri),
                        ]));
                    }
                    _ => {}
                }
            }
        }
    }
    if !sans.is_empty() {
        dict.insert(PyKey::str_from("subjectAltName"), Value::tuple(sans));
    }

    Ok(Value::dict(dict))
}

pub(crate) fn is_ca_cert_der(der: &[u8]) -> bool {
    let Ok((_, cert)) = X509Certificate::from_der(der) else {
        return false;
    };
    cert.is_ca()
}

// ── MemoryBIO-driven TLS I/O helpers ─────────────────────────────────────────

pub(crate) fn flush_tls_to_bio(conn: &mut Connection, outgoing: &Rc<RefCell<MemoryBioInner>>) {
    let mut out_buf = Vec::new();
    while conn.wants_write() {
        out_buf.clear();
        match conn.write_tls(&mut out_buf) {
            Ok(0) => break,
            Ok(_) => {
                outgoing.borrow_mut().write_bytes(&out_buf);
            }
            Err(_) => break,
        }
    }
}

pub(crate) fn bio_do_handshake(
    interp: &mut Interpreter,
    conn: &mut Connection,
    incoming: &Rc<RefCell<MemoryBioInner>>,
    outgoing: &Rc<RefCell<MemoryBioInner>>,
) -> Result<()> {
    loop {
        flush_tls_to_bio(conn, outgoing);
        if !conn.is_handshaking() {
            return Ok(());
        }
        if conn.wants_read() {
            let chunk = incoming.borrow_mut().read_bytes(None);
            if chunk.is_empty() {
                if incoming.borrow().is_eof() {
                    return Err(raise_ssl_eof(
                        interp,
                        "EOF occurred in violation of protocol",
                    ));
                }
                return Err(raise_want_read(interp));
            }
            let mut cursor = Cursor::new(&chunk);
            while (cursor.position() as usize) < chunk.len() {
                let n = conn.read_tls(&mut cursor).map_err(|e| {
                    raise_ssl_error(
                        interp,
                        "SSLError",
                        SSL_ERROR_SSL,
                        "TLS_READ_ERROR",
                        &e.to_string(),
                    )
                })?;
                if n == 0 {
                    break;
                }
                if let Err(tls_err) = conn.process_new_packets() {
                    // Flush any TLS alert generated before surfacing the error.
                    flush_tls_to_bio(conn, outgoing);
                    return Err(rustls_err_to_py(interp, &tls_err));
                }
            }
            // Any unconsumed trailing bytes stay in incoming for the next call.
            let pos = cursor.position() as usize;
            if pos < chunk.len() {
                let mut inc = incoming.borrow_mut();
                for &b in chunk[pos..].iter().rev() {
                    inc.buf.push_front(b);
                }
            }
        } else if !conn.wants_write() {
            return Ok(());
        }
    }
}

pub(crate) fn bio_read(
    interp: &mut Interpreter,
    conn: &mut Connection,
    incoming: &Rc<RefCell<MemoryBioInner>>,
    outgoing: &Rc<RefCell<MemoryBioInner>>,
    max_len: usize,
) -> Result<Vec<u8>> {
    if max_len == 0 {
        return Ok(Vec::new());
    }
    let mut tmp = vec![0u8; max_len];
    loop {
        match conn.reader().read(&mut tmp) {
            Ok(n) => {
                tmp.truncate(n);
                return Ok(tmp);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                flush_tls_to_bio(conn, outgoing);
                let chunk = incoming.borrow_mut().read_bytes(None);
                if chunk.is_empty() {
                    if incoming.borrow().is_eof() {
                        return Err(raise_ssl_eof(
                            interp,
                            "EOF occurred in violation of protocol",
                        ));
                    }
                    return Err(raise_want_read(interp));
                }
                let mut cursor = Cursor::new(&chunk);
                while (cursor.position() as usize) < chunk.len() {
                    let n = conn.read_tls(&mut cursor).map_err(|err| {
                        raise_ssl_error(
                            interp,
                            "SSLError",
                            SSL_ERROR_SSL,
                            "TLS_READ_ERROR",
                            &err.to_string(),
                        )
                    })?;
                    if n == 0 {
                        break;
                    }
                    match conn.process_new_packets() {
                        Ok(io_state) => {
                            if io_state.plaintext_bytes_to_read() > 0 || io_state.peer_has_closed()
                            {
                                break;
                            }
                        }
                        Err(tls_err) => {
                            flush_tls_to_bio(conn, outgoing);
                            return Err(rustls_err_to_py(interp, &tls_err));
                        }
                    }
                }
                let pos = cursor.position() as usize;
                if pos < chunk.len() {
                    let mut inc = incoming.borrow_mut();
                    for &b in chunk[pos..].iter().rev() {
                        inc.buf.push_front(b);
                    }
                }
                flush_tls_to_bio(conn, outgoing);
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(raise_ssl_eof(
                    interp,
                    "EOF occurred in violation of protocol",
                ));
            }
            Err(e) => {
                return Err(raise_ssl_error(
                    interp,
                    "SSLError",
                    SSL_ERROR_SSL,
                    "TLS_READ_ERROR",
                    &e.to_string(),
                ));
            }
        }
    }
}

pub(crate) fn bio_write(
    interp: &mut Interpreter,
    conn: &mut Connection,
    outgoing: &Rc<RefCell<MemoryBioInner>>,
    data: &[u8],
) -> Result<usize> {
    let n = conn.writer().write(data).map_err(|e| {
        raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "TLS_WRITE_ERROR",
            &e.to_string(),
        )
    })?;
    flush_tls_to_bio(conn, outgoing);
    Ok(n)
}

// ── Socket-backed TLS I/O helpers ────────────────────────────────────────────

fn map_sock_io_err(
    interp: &mut Interpreter,
    handle: &Value,
    err: &io::Error,
    is_read: bool,
) -> PyError {
    if err.kind() == io::ErrorKind::WouldBlock && super::socket::handle_is_nonblocking(handle) {
        return if is_read {
            raise_want_read(interp)
        } else {
            raise_want_write(interp)
        };
    }
    let timeout = super::socket::handle_timeout(handle);
    super::socket::io_error_to_socket_py_pub(err, timeout)
}

pub(crate) fn sock_flush_tls(
    interp: &mut Interpreter,
    conn: &mut Connection,
    handle: &Value,
) -> Result<()> {
    let mut out_buf = Vec::new();
    while conn.wants_write() {
        out_buf.clear();
        let n = conn.write_tls(&mut out_buf).map_err(|e| {
            raise_ssl_error(
                interp,
                "SSLError",
                SSL_ERROR_SSL,
                "TLS_WRITE_ERROR",
                &e.to_string(),
            )
        })?;
        if n == 0 {
            break;
        }
        let mut written = 0usize;
        while written < out_buf.len() {
            match super::socket::handle_write_raw(handle, &out_buf[written..]) {
                Ok(0) => {
                    return Err(raise_ssl_eof(
                        interp,
                        "EOF occurred in violation of protocol",
                    ));
                }
                Ok(w) => written += w,
                Err(e) => return Err(map_sock_io_err(interp, handle, &e, false)),
            }
        }
    }
    Ok(())
}

fn sock_read_tls_step(
    interp: &mut Interpreter,
    conn: &mut Connection,
    handle: &Value,
) -> Result<usize> {
    let mut wire = [0u8; 16384];
    let n = super::socket::handle_read_raw(handle, &mut wire)
        .map_err(|e| map_sock_io_err(interp, handle, &e, true))?;
    if n == 0 {
        return Ok(0);
    }
    let mut cursor = Cursor::new(&wire[..n]);
    while (cursor.position() as usize) < n {
        let consumed = conn.read_tls(&mut cursor).map_err(|e| {
            raise_ssl_error(
                interp,
                "SSLError",
                SSL_ERROR_SSL,
                "TLS_READ_ERROR",
                &e.to_string(),
            )
        })?;
        if consumed == 0 {
            break;
        }
        if let Err(tls_err) = conn.process_new_packets() {
            let _ = sock_flush_tls(interp, conn, handle);
            return Err(rustls_err_to_py(interp, &tls_err));
        }
    }
    Ok(n)
}

pub(crate) fn sock_do_handshake(
    interp: &mut Interpreter,
    conn: &mut Connection,
    handle: &Value,
) -> Result<()> {
    while conn.is_handshaking() {
        sock_flush_tls(interp, conn, handle)?;
        if !conn.is_handshaking() {
            break;
        }
        if conn.wants_read() {
            let n = sock_read_tls_step(interp, conn, handle)?;
            if n == 0 {
                return Err(raise_ssl_eof(
                    interp,
                    "EOF occurred in violation of protocol",
                ));
            }
        }
    }
    sock_flush_tls(interp, conn, handle)?;
    Ok(())
}

pub(crate) fn sock_tls_read(
    interp: &mut Interpreter,
    conn: &mut Connection,
    handle: &Value,
    max_len: usize,
    suppress_ragged_eofs: bool,
) -> Result<Vec<u8>> {
    if max_len == 0 {
        return Ok(Vec::new());
    }
    let mut tmp = vec![0u8; max_len];
    loop {
        match conn.reader().read(&mut tmp) {
            Ok(n) => {
                tmp.truncate(n);
                return Ok(tmp);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                sock_flush_tls(interp, conn, handle)?;
                let n = sock_read_tls_step(interp, conn, handle)?;
                if n == 0 {
                    // Underlying TCP stream hit EOF without close_notify.
                    if suppress_ragged_eofs {
                        return Ok(Vec::new());
                    }
                    return Err(raise_ssl_eof(
                        interp,
                        "EOF occurred in violation of protocol",
                    ));
                }
                sock_flush_tls(interp, conn, handle)?;
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                if suppress_ragged_eofs {
                    return Ok(Vec::new());
                }
                return Err(raise_ssl_eof(
                    interp,
                    "EOF occurred in violation of protocol",
                ));
            }
            Err(e) => {
                return Err(raise_ssl_error(
                    interp,
                    "SSLError",
                    SSL_ERROR_SSL,
                    "TLS_READ_ERROR",
                    &e.to_string(),
                ));
            }
        }
    }
}

pub(crate) fn sock_tls_write(
    interp: &mut Interpreter,
    conn: &mut Connection,
    handle: &Value,
    data: &[u8],
) -> Result<usize> {
    if data.is_empty() {
        return Ok(0);
    }
    let n = conn.writer().write(data).map_err(|e| {
        raise_ssl_error(
            interp,
            "SSLError",
            SSL_ERROR_SSL,
            "TLS_WRITE_ERROR",
            &e.to_string(),
        )
    })?;
    sock_flush_tls(interp, conn, handle)?;
    Ok(n)
}

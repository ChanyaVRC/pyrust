// `socket` module — body for the `socket @inject` entry in
// `pyrust_builtin_modules!` (issue #3205).
//
// Native Rust helpers backed by `std::net` (`TcpListener`, `TcpStream`,
// `UdpSocket`, `ToSocketAddrs`, `Ipv4Addr`, `Ipv6Addr`) plus cross-platform
// OS readiness polling (`libc::poll` on Unix, Winsock2 `select` on Windows)
// shared with `selectors`.
//
// The public Python 3.12 API (`AddressFamily`, `SocketKind`, `AddressInfo`,
// `socket`, `SocketIO`, `create_connection`, `create_server`, `getfqdn`,
// `getaddrinfo`, `getdefaulttimeout`, `setdefaulttimeout`, `error`, `timeout`,
// `gaierror`, `herror`, ...) lives in `socket_py.py` and is injected by
// `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/socket.html>

use std::any::Any;
use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::net::{TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::rc::Rc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

use pyrust_core::{BuiltinTypeOps, PyErrorKind, builtin_ops_is};
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyKey, Value, ValueKind};

/// Python-source definitions for the `socket` module.
const SOCKET_PY_SOURCE: &str = include_str!("socket_py.py");

/// Private native helpers moved into the exec namespace and hidden from
/// `dir(socket)` so the module surface matches CPython 3.12.
const SOCKET_NATIVE_HELPERS: &[&str] = &[
    "_constants_map",
    "_sock_new",
    "_sock_bind",
    "_sock_listen",
    "_sock_accept",
    "_sock_connect",
    "_sock_connect_ex",
    "_sock_send",
    "_sock_sendall",
    "_sock_recv",
    "_sock_recv_into",
    "_sock_sendto",
    "_sock_recvfrom",
    "_sock_shutdown",
    "_sock_close",
    "_sock_fileno",
    "_sock_setsockopt",
    "_sock_getsockopt",
    "_sock_settimeout",
    "_sock_gettimeout",
    "_sock_getsockname",
    "_sock_getpeername",
    "_native_gethostname",
    "_native_gethostbyname",
    "_native_getaddrinfo",
    "_native_inet_aton",
    "_native_inet_ntoa",
    "_native_inet_pton",
    "_native_inet_ntop",
    "_native_htons",
    "_native_htonl",
    "_native_ntohs",
    "_native_ntohl",
];

/// Public attributes exported from `SOCKET_PY_SOURCE` onto the `socket` module.
const SOCKET_PY_EXPORTS: &[&str] = &[
    "__all__",
    "AF_UNSPEC",
    "AF_INET",
    "AF_INET6",
    "SOCK_STREAM",
    "SOCK_DGRAM",
    "SOL_SOCKET",
    "SO_REUSEADDR",
    "SO_KEEPALIVE",
    "SO_BROADCAST",
    "SO_RCVBUF",
    "SO_SNDBUF",
    "SO_ERROR",
    "IPPROTO_IP",
    "IPPROTO_TCP",
    "IPPROTO_UDP",
    "IPPROTO_IPV6",
    "TCP_NODELAY",
    "IPV6_V6ONLY",
    "SHUT_RD",
    "SHUT_WR",
    "SHUT_RDWR",
    "SOMAXCONN",
    "AI_PASSIVE",
    "AI_CANONNAME",
    "AI_NUMERICHOST",
    "AI_NUMERICSERV",
    "MSG_PEEK",
    "has_ipv6",
    "AddressFamily",
    "SocketKind",
    "AddressInfo",
    "error",
    "timeout",
    "gaierror",
    "herror",
    "SocketType",
    "socket",
    "SocketIO",
    "create_connection",
    "create_server",
    "gethostname",
    "getfqdn",
    "gethostbyname",
    "getaddrinfo",
    "getdefaulttimeout",
    "setdefaulttimeout",
    "htons",
    "htonl",
    "ntohs",
    "ntohl",
    "inet_aton",
    "inet_ntoa",
    "inet_pton",
    "inet_ntop",
    "_GLOBAL_DEFAULT_TIMEOUT",
];

// ── Platform constants ───────────────────────────────────────────────────────

#[cfg(unix)]
mod plat {
    pub const AF_UNSPEC: i64 = libc::AF_UNSPEC as i64;
    pub const AF_INET: i64 = libc::AF_INET as i64;
    pub const AF_INET6: i64 = libc::AF_INET6 as i64;
    pub const SOCK_STREAM: i64 = libc::SOCK_STREAM as i64;
    pub const SOCK_DGRAM: i64 = libc::SOCK_DGRAM as i64;
    pub const SOL_SOCKET: i64 = libc::SOL_SOCKET as i64;
    pub const SO_REUSEADDR: i64 = libc::SO_REUSEADDR as i64;
    pub const SO_KEEPALIVE: i64 = libc::SO_KEEPALIVE as i64;
    pub const SO_BROADCAST: i64 = libc::SO_BROADCAST as i64;
    pub const SO_RCVBUF: i64 = libc::SO_RCVBUF as i64;
    pub const SO_SNDBUF: i64 = libc::SO_SNDBUF as i64;
    pub const SO_ERROR: i64 = libc::SO_ERROR as i64;
    pub const IPPROTO_IP: i64 = libc::IPPROTO_IP as i64;
    pub const IPPROTO_TCP: i64 = libc::IPPROTO_TCP as i64;
    pub const IPPROTO_UDP: i64 = libc::IPPROTO_UDP as i64;
    pub const IPPROTO_IPV6: i64 = libc::IPPROTO_IPV6 as i64;
    pub const TCP_NODELAY: i64 = libc::TCP_NODELAY as i64;
    pub const IPV6_V6ONLY: i64 = libc::IPV6_V6ONLY as i64;
    pub const SHUT_RD: i64 = libc::SHUT_RD as i64;
    pub const SHUT_WR: i64 = libc::SHUT_WR as i64;
    pub const SHUT_RDWR: i64 = libc::SHUT_RDWR as i64;
    pub const SOMAXCONN: i64 = libc::SOMAXCONN as i64;
    pub const AI_PASSIVE: i64 = libc::AI_PASSIVE as i64;
    pub const AI_CANONNAME: i64 = libc::AI_CANONNAME as i64;
    pub const AI_NUMERICHOST: i64 = libc::AI_NUMERICHOST as i64;
    pub const AI_NUMERICSERV: i64 = libc::AI_NUMERICSERV as i64;
    pub const MSG_PEEK: i64 = libc::MSG_PEEK as i64;
    pub const EAI_NONAME: i64 = libc::EAI_NONAME as i64;
    pub const EBADF: i64 = libc::EBADF as i64;
    pub const EINVAL: i64 = libc::EINVAL as i64;
    pub const ENOTCONN: i64 = libc::ENOTCONN as i64;
    pub const EDESTADDRREQ: i64 = libc::EDESTADDRREQ as i64;
    pub const EAFNOSUPPORT: i64 = libc::EAFNOSUPPORT as i64;
    pub const EPROTONOSUPPORT: i64 = libc::EPROTONOSUPPORT as i64;
    pub const ESOCKTNOSUPPORT: i64 = libc::ESOCKTNOSUPPORT as i64;
    pub const ECONNREFUSED: i64 = libc::ECONNREFUSED as i64;
    pub const ETIMEDOUT: i64 = libc::ETIMEDOUT as i64;
    pub const EAGAIN: i64 = libc::EAGAIN as i64;
}

#[cfg(windows)]
mod plat {
    pub const AF_UNSPEC: i64 = 0;
    pub const AF_INET: i64 = 2;
    pub const AF_INET6: i64 = 23;
    pub const SOCK_STREAM: i64 = 1;
    pub const SOCK_DGRAM: i64 = 2;
    pub const SOL_SOCKET: i64 = 0xffff;
    pub const SO_REUSEADDR: i64 = 0x0004;
    pub const SO_KEEPALIVE: i64 = 0x0008;
    pub const SO_BROADCAST: i64 = 0x0020;
    pub const SO_RCVBUF: i64 = 0x1002;
    pub const SO_SNDBUF: i64 = 0x1001;
    pub const SO_ERROR: i64 = 0x1007;
    pub const IPPROTO_IP: i64 = 0;
    pub const IPPROTO_TCP: i64 = 6;
    pub const IPPROTO_UDP: i64 = 17;
    pub const IPPROTO_IPV6: i64 = 41;
    pub const TCP_NODELAY: i64 = 1;
    pub const IPV6_V6ONLY: i64 = 27;
    pub const SHUT_RD: i64 = 0;
    pub const SHUT_WR: i64 = 1;
    pub const SHUT_RDWR: i64 = 2;
    pub const SOMAXCONN: i64 = 0x7fffffff;
    pub const AI_PASSIVE: i64 = 1;
    pub const AI_CANONNAME: i64 = 2;
    pub const AI_NUMERICHOST: i64 = 4;
    pub const AI_NUMERICSERV: i64 = 8;
    pub const MSG_PEEK: i64 = 2;
    pub const EAI_NONAME: i64 = 11001;
    pub const EBADF: i64 = 9;
    pub const EINVAL: i64 = 22;
    pub const ENOTCONN: i64 = 10057;
    pub const EDESTADDRREQ: i64 = 10039;
    pub const EAFNOSUPPORT: i64 = 10047;
    pub const EPROTONOSUPPORT: i64 = 10043;
    pub const ESOCKTNOSUPPORT: i64 = 10044;
    pub const ECONNREFUSED: i64 = 10061;
    pub const ETIMEDOUT: i64 = 10060;
    pub const EAGAIN: i64 = 10035;
}

// ── Cross-platform low-level socket FFI ──────────────────────────────────────

#[cfg(windows)]
mod win_ffi {
    #[repr(C)]
    pub struct Timeval {
        pub tv_sec: i32,
        pub tv_usec: i32,
    }

    /// Winsock2 `fd_set` with capacity for up to 1024 handles.
    /// `#[repr(C)]` matches the Windows C ABI alignment of `u_int` + `SOCKET`.
    #[repr(C)]
    pub struct WinFdSet {
        pub fd_count: u32,
        pub fd_array: [usize; 1024],
    }

    impl WinFdSet {
        pub fn new_boxed() -> Box<Self> {
            Box::new(Self {
                fd_count: 0,
                fd_array: [0; 1024],
            })
        }

        pub fn push(&mut self, sock: usize) -> bool {
            let idx = self.fd_count as usize;
            if idx >= self.fd_array.len() {
                return false;
            }
            self.fd_array[idx] = sock;
            self.fd_count += 1;
            true
        }

        pub fn contains(&self, sock: usize) -> bool {
            let count = (self.fd_count as usize).min(self.fd_array.len());
            self.fd_array[..count].contains(&sock)
        }
    }

    #[link(name = "ws2_32")]
    unsafe extern "system" {
        #[link_name = "select"]
        pub fn ws_select(
            nfds: i32,
            readfds: *mut WinFdSet,
            writefds: *mut WinFdSet,
            exceptfds: *mut WinFdSet,
            timeout: *const Timeval,
        ) -> i32;

        #[link_name = "WSAGetLastError"]
        pub fn wsa_get_last_error() -> i32;

        #[link_name = "setsockopt"]
        pub fn ws_setsockopt(
            s: usize,
            level: i32,
            optname: i32,
            optval: *const u8,
            optlen: i32,
        ) -> i32;

        #[link_name = "getsockopt"]
        pub fn ws_getsockopt(
            s: usize,
            level: i32,
            optname: i32,
            optval: *mut u8,
            optlen: *mut i32,
        ) -> i32;

        #[link_name = "gethostname"]
        pub fn ws_gethostname(name: *mut u8, namelen: i32) -> i32;
    }
}

/// Cross-platform OS readiness check on raw socket handles/fds.
///
/// Used both by `selectors.SelectSelector` / `DefaultSelector` and by
/// `socket.accept()` when a finite timeout is active.
pub(crate) fn select_raw_handles(
    read_fds: &[i64],
    write_fds: &[i64],
    timeout: Option<Duration>,
) -> Result<(Vec<i64>, Vec<i64>)> {
    if read_fds.is_empty() && write_fds.is_empty() {
        if let Some(dur) = timeout
            && !dur.is_zero()
        {
            std::thread::sleep(dur);
        }
        return Ok((Vec::new(), Vec::new()));
    }

    #[cfg(unix)]
    {
        let mut unique: Vec<(i64, i16)> = Vec::new();
        for &fd in read_fds {
            if fd < 0 {
                return Err(os_error(plat::EBADF, "Bad file descriptor"));
            }
            if let Some(entry) = unique.iter_mut().find(|(f, _)| *f == fd) {
                entry.1 |= libc::POLLIN;
            } else {
                unique.push((fd, libc::POLLIN));
            }
        }
        for &fd in write_fds {
            if fd < 0 {
                return Err(os_error(plat::EBADF, "Bad file descriptor"));
            }
            if let Some(entry) = unique.iter_mut().find(|(f, _)| *f == fd) {
                entry.1 |= libc::POLLOUT;
            } else {
                unique.push((fd, libc::POLLOUT));
            }
        }

        let mut pfds: Vec<libc::pollfd> = unique
            .iter()
            .map(|&(fd, events)| libc::pollfd {
                fd: fd as libc::c_int,
                events,
                revents: 0,
            })
            .collect();

        let deadline = timeout.map(|d| Instant::now() + d);
        loop {
            for pfd in &mut pfds {
                pfd.revents = 0;
            }
            let timeout_ms: libc::c_int = match (timeout, deadline) {
                (None, _) => -1,
                (Some(d), _) if d.is_zero() => 0,
                (Some(_), Some(dl)) => {
                    let rem = dl.saturating_duration_since(Instant::now());
                    if rem.is_zero() {
                        0
                    } else {
                        let ms = rem.as_millis();
                        if ms == 0 {
                            1
                        } else {
                            ms.min(libc::c_int::MAX as u128) as libc::c_int
                        }
                    }
                }
                _ => 0,
            };

            let rc =
                unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == ErrorKind::Interrupted {
                    if let Some(dl) = deadline
                        && Instant::now() >= dl
                    {
                        return Ok((Vec::new(), Vec::new()));
                    }
                    continue;
                }
                return Err(PyError::from_io_error(&err, None));
            }
            if rc == 0 {
                return Ok((Vec::new(), Vec::new()));
            }
            for pfd in &pfds {
                if (pfd.revents & libc::POLLNVAL) != 0 {
                    return Err(os_error(plat::EBADF, "Bad file descriptor"));
                }
            }
            let mut ready_r = Vec::new();
            let mut ready_w = Vec::new();
            for &fd in read_fds {
                if let Some(pfd) = pfds.iter().find(|p| p.fd as i64 == fd)
                    && (pfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0
                    && !ready_r.contains(&fd)
                {
                    ready_r.push(fd);
                }
            }
            for &fd in write_fds {
                if let Some(pfd) = pfds.iter().find(|p| p.fd as i64 == fd)
                    && (pfd.revents & (libc::POLLOUT | libc::POLLERR | libc::POLLHUP)) != 0
                    && !ready_w.contains(&fd)
                {
                    ready_w.push(fd);
                }
            }
            return Ok((ready_r, ready_w));
        }
    }

    #[cfg(windows)]
    {
        let mut rset = win_ffi::WinFdSet::new_boxed();
        let mut wset = win_ffi::WinFdSet::new_boxed();
        let mut xset = win_ffi::WinFdSet::new_boxed();

        for &fd in read_fds {
            if fd < 0 {
                return Err(os_error(plat::EBADF, "Bad file descriptor"));
            }
            if !rset.push(fd as usize) {
                return Err(PyError::named(
                    "ValueError",
                    "too many file descriptors in select()".to_string(),
                ));
            }
        }
        for &fd in write_fds {
            if fd < 0 {
                return Err(os_error(plat::EBADF, "Bad file descriptor"));
            }
            if !wset.push(fd as usize) || !xset.push(fd as usize) {
                return Err(PyError::named(
                    "ValueError",
                    "too many file descriptors in select()".to_string(),
                ));
            }
        }

        let tv = timeout.map(|d| win_ffi::Timeval {
            tv_sec: d.as_secs().min(i32::MAX as u64) as i32,
            tv_usec: d.subsec_micros() as i32,
        });
        let rptr = if rset.fd_count > 0 {
            &mut *rset as *mut win_ffi::WinFdSet
        } else {
            std::ptr::null_mut()
        };
        let wptr = if wset.fd_count > 0 {
            &mut *wset as *mut win_ffi::WinFdSet
        } else {
            std::ptr::null_mut()
        };
        let xptr = if xset.fd_count > 0 {
            &mut *xset as *mut win_ffi::WinFdSet
        } else {
            std::ptr::null_mut()
        };
        let tvptr = match &tv {
            Some(t) => t as *const win_ffi::Timeval,
            None => std::ptr::null(),
        };

        let rc = unsafe { win_ffi::ws_select(0, rptr, wptr, xptr, tvptr) };
        if rc < 0 {
            let code = unsafe { win_ffi::wsa_get_last_error() };
            let err = std::io::Error::from_raw_os_error(code);
            return Err(PyError::from_io_error(&err, None));
        }
        if rc == 0 {
            return Ok((Vec::new(), Vec::new()));
        }

        let mut ready_r = Vec::new();
        for &fd in read_fds {
            if rset.contains(fd as usize) && !ready_r.contains(&fd) {
                ready_r.push(fd);
            }
        }
        let mut ready_w = Vec::new();
        for &fd in write_fds {
            if (wset.contains(fd as usize) || xset.contains(fd as usize)) && !ready_w.contains(&fd)
            {
                ready_w.push(fd);
            }
        }
        Ok((ready_r, ready_w))
    }
}

/// Cross-platform 3-set OS `select()` on raw socket handles/fds for the
/// `select` stdlib module (`select.select(rlist, wlist, xlist[, timeout])`).
pub(crate) fn select_raw_handles_3(
    read_fds: &[i64],
    write_fds: &[i64],
    except_fds: &[i64],
    timeout: Option<Duration>,
) -> Result<(Vec<i64>, Vec<i64>, Vec<i64>)> {
    #[cfg(unix)]
    {
        let fd_setsize = libc::FD_SETSIZE as i64;
        for &fd in read_fds.iter().chain(write_fds).chain(except_fds) {
            if fd < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("file descriptor cannot be a negative integer ({fd})"),
                ));
            }
            if fd >= fd_setsize {
                return Err(PyError::named(
                    "ValueError",
                    "filedescriptor out of range in select()".to_string(),
                ));
            }
        }

        let deadline = timeout.map(|d| Instant::now() + d);
        loop {
            let mut rset: libc::fd_set = unsafe { std::mem::zeroed() };
            let mut wset: libc::fd_set = unsafe { std::mem::zeroed() };
            let mut xset: libc::fd_set = unsafe { std::mem::zeroed() };
            unsafe {
                libc::FD_ZERO(&mut rset);
                libc::FD_ZERO(&mut wset);
                libc::FD_ZERO(&mut xset);
            }
            let mut max_fd: libc::c_int = -1;
            for &fd in read_fds {
                let cfd = fd as libc::c_int;
                unsafe { libc::FD_SET(cfd, &mut rset) };
                if cfd > max_fd {
                    max_fd = cfd;
                }
            }
            for &fd in write_fds {
                let cfd = fd as libc::c_int;
                unsafe { libc::FD_SET(cfd, &mut wset) };
                if cfd > max_fd {
                    max_fd = cfd;
                }
            }
            for &fd in except_fds {
                let cfd = fd as libc::c_int;
                unsafe { libc::FD_SET(cfd, &mut xset) };
                if cfd > max_fd {
                    max_fd = cfd;
                }
            }

            let mut tv = match (timeout, deadline) {
                (None, _) => None,
                (Some(d), _) if d.is_zero() => Some(libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                }),
                (Some(_), Some(dl)) => {
                    let rem = dl.saturating_duration_since(Instant::now());
                    Some(libc::timeval {
                        tv_sec: rem.as_secs() as libc::time_t,
                        tv_usec: rem.subsec_micros() as libc::suseconds_t,
                    })
                }
                _ => Some(libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                }),
            };
            let tv_ptr = match tv.as_mut() {
                Some(t) => t as *mut libc::timeval,
                None => std::ptr::null_mut(),
            };
            let rptr = if read_fds.is_empty() {
                std::ptr::null_mut()
            } else {
                &mut rset as *mut libc::fd_set
            };
            let wptr = if write_fds.is_empty() {
                std::ptr::null_mut()
            } else {
                &mut wset as *mut libc::fd_set
            };
            let xptr = if except_fds.is_empty() {
                std::ptr::null_mut()
            } else {
                &mut xset as *mut libc::fd_set
            };

            let rc = unsafe { libc::select(max_fd + 1, rptr, wptr, xptr, tv_ptr) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == ErrorKind::Interrupted {
                    if let Some(dl) = deadline
                        && Instant::now() >= dl
                    {
                        return Ok((Vec::new(), Vec::new(), Vec::new()));
                    }
                    continue;
                }
                return Err(PyError::from_io_error(&err, None));
            }
            if rc == 0 {
                return Ok((Vec::new(), Vec::new(), Vec::new()));
            }
            let ready_r = read_fds
                .iter()
                .copied()
                .filter(|&fd| unsafe { libc::FD_ISSET(fd as libc::c_int, &rset) })
                .collect();
            let ready_w = write_fds
                .iter()
                .copied()
                .filter(|&fd| unsafe { libc::FD_ISSET(fd as libc::c_int, &wset) })
                .collect();
            let ready_x = except_fds
                .iter()
                .copied()
                .filter(|&fd| unsafe { libc::FD_ISSET(fd as libc::c_int, &xset) })
                .collect();
            return Ok((ready_r, ready_w, ready_x));
        }
    }

    #[cfg(windows)]
    {
        let mut rset = win_ffi::WinFdSet::new_boxed();
        let mut wset = win_ffi::WinFdSet::new_boxed();
        let mut xset = win_ffi::WinFdSet::new_boxed();

        for &fd in read_fds {
            if fd < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("file descriptor cannot be a negative integer ({fd})"),
                ));
            }
            if !rset.push(fd as usize) {
                return Err(PyError::named(
                    "ValueError",
                    "too many file descriptors in select()".to_string(),
                ));
            }
        }
        for &fd in write_fds {
            if fd < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("file descriptor cannot be a negative integer ({fd})"),
                ));
            }
            if !wset.push(fd as usize) {
                return Err(PyError::named(
                    "ValueError",
                    "too many file descriptors in select()".to_string(),
                ));
            }
        }
        for &fd in except_fds {
            if fd < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("file descriptor cannot be a negative integer ({fd})"),
                ));
            }
            if !xset.push(fd as usize) {
                return Err(PyError::named(
                    "ValueError",
                    "too many file descriptors in select()".to_string(),
                ));
            }
        }

        let tv = timeout.map(|d| win_ffi::Timeval {
            tv_sec: d.as_secs().min(i32::MAX as u64) as i32,
            tv_usec: d.subsec_micros() as i32,
        });
        let rptr = if rset.fd_count > 0 {
            &mut *rset as *mut win_ffi::WinFdSet
        } else {
            std::ptr::null_mut()
        };
        let wptr = if wset.fd_count > 0 {
            &mut *wset as *mut win_ffi::WinFdSet
        } else {
            std::ptr::null_mut()
        };
        let xptr = if xset.fd_count > 0 {
            &mut *xset as *mut win_ffi::WinFdSet
        } else {
            std::ptr::null_mut()
        };
        let tvptr = match &tv {
            Some(t) => t as *const win_ffi::Timeval,
            None => std::ptr::null(),
        };

        let rc = unsafe { win_ffi::ws_select(0, rptr, wptr, xptr, tvptr) };
        if rc < 0 {
            let code = unsafe { win_ffi::wsa_get_last_error() };
            let err = std::io::Error::from_raw_os_error(code);
            return Err(PyError::from_io_error(&err, None));
        }
        if rc == 0 {
            return Ok((Vec::new(), Vec::new(), Vec::new()));
        }

        let ready_r = read_fds
            .iter()
            .copied()
            .filter(|&fd| rset.contains(fd as usize))
            .collect();
        let ready_w = write_fds
            .iter()
            .copied()
            .filter(|&fd| wset.contains(fd as usize))
            .collect();
        let ready_x = except_fds
            .iter()
            .copied()
            .filter(|&fd| xset.contains(fd as usize))
            .collect();
        Ok((ready_r, ready_w, ready_x))
    }
}

fn raw_setsockopt_int(raw: i64, level: i32, optname: i32, val: i32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let rc = unsafe {
            libc::setsockopt(
                raw as libc::c_int,
                level,
                optname,
                (&val as *const i32).cast::<libc::c_void>(),
                std::mem::size_of::<i32>() as libc::socklen_t,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        let rc = unsafe {
            win_ffi::ws_setsockopt(
                raw as usize,
                level,
                optname,
                (&val as *const i32).cast::<u8>(),
                std::mem::size_of::<i32>() as i32,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            let code = unsafe { win_ffi::wsa_get_last_error() };
            Err(std::io::Error::from_raw_os_error(code))
        }
    }
}

fn raw_getsockopt_int(raw: i64, level: i32, optname: i32) -> std::io::Result<i32> {
    #[cfg(unix)]
    {
        let mut val: i32 = 0;
        let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                raw as libc::c_int,
                level,
                optname,
                (&mut val as *mut i32).cast::<libc::c_void>(),
                &mut len,
            )
        };
        if rc == 0 {
            Ok(val)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        let mut val: i32 = 0;
        let mut len = std::mem::size_of::<i32>() as i32;
        let rc = unsafe {
            win_ffi::ws_getsockopt(
                raw as usize,
                level,
                optname,
                (&mut val as *mut i32).cast::<u8>(),
                &mut len,
            )
        };
        if rc == 0 {
            Ok(val)
        } else {
            let code = unsafe { win_ffi::wsa_get_last_error() };
            Err(std::io::Error::from_raw_os_error(code))
        }
    }
}

#[cfg(unix)]
fn tcp_listener_raw(l: &TcpListener) -> i64 {
    use std::os::unix::io::AsRawFd;
    l.as_raw_fd() as i64
}

#[cfg(windows)]
fn tcp_listener_raw(l: &TcpListener) -> i64 {
    use std::os::windows::io::AsRawSocket;
    l.as_raw_socket() as i64
}

#[cfg(unix)]
fn tcp_stream_raw(s: &TcpStream) -> i64 {
    use std::os::unix::io::AsRawFd;
    s.as_raw_fd() as i64
}

#[cfg(windows)]
fn tcp_stream_raw(s: &TcpStream) -> i64 {
    use std::os::windows::io::AsRawSocket;
    s.as_raw_socket() as i64
}

#[cfg(unix)]
fn udp_socket_raw(u: &UdpSocket) -> i64 {
    use std::os::unix::io::AsRawFd;
    u.as_raw_fd() as i64
}

#[cfg(windows)]
fn udp_socket_raw(u: &UdpSocket) -> i64 {
    use std::os::windows::io::AsRawSocket;
    u.as_raw_socket() as i64
}

// ── Error helpers ────────────────────────────────────────────────────────────

fn os_error(errno: i64, strerror: &str) -> PyError {
    PyError::new(PyErrorKind::OsError {
        class_name: "OSError",
        errno,
        strerror: strerror.to_string(),
        filename: None,
        filename2: None,
    })
}

fn closed_socket_error() -> PyError {
    os_error(plat::EBADF, "Bad file descriptor")
}

fn raise_gaierror(gaierror_cls: &Value, errno: i64, msg: &str) -> PyError {
    if let ValueKind::PyClass(cls_rc) = gaierror_cls.kind() {
        return PyError::raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::int(errno), Value::string(msg)],
        ));
    }
    os_error(errno, msg)
}

fn io_error_to_socket_py(err: &std::io::Error, timeout: Option<Duration>) -> PyError {
    match err.kind() {
        ErrorKind::TimedOut => PyError::named("TimeoutError", "timed out".to_string()),
        ErrorKind::WouldBlock => {
            if matches!(timeout, Some(d) if !d.is_zero()) {
                PyError::named("TimeoutError", "timed out".to_string())
            } else {
                PyError::from_io_error(err, None)
            }
        }
        ErrorKind::ConnectionRefused => {
            let raw = err.raw_os_error().map_or(plat::ECONNREFUSED, i64::from);
            PyError::new(PyErrorKind::OsError {
                class_name: "ConnectionRefusedError",
                errno: raw,
                strerror: "Connection refused".to_string(),
                filename: None,
                filename2: None,
            })
        }
        ErrorKind::ConnectionReset => {
            let raw = err.raw_os_error().map_or(104, i64::from);
            PyError::new(PyErrorKind::OsError {
                class_name: "ConnectionResetError",
                errno: raw,
                strerror: "Connection reset by peer".to_string(),
                filename: None,
                filename2: None,
            })
        }
        ErrorKind::ConnectionAborted => {
            let raw = err.raw_os_error().map_or(103, i64::from);
            PyError::new(PyErrorKind::OsError {
                class_name: "ConnectionAbortedError",
                errno: raw,
                strerror: "Software caused connection abort".to_string(),
                filename: None,
                filename2: None,
            })
        }
        ErrorKind::BrokenPipe => {
            let raw = err.raw_os_error().map_or(32, i64::from);
            PyError::new(PyErrorKind::OsError {
                class_name: "BrokenPipeError",
                errno: raw,
                strerror: "Broken pipe".to_string(),
                filename: None,
                filename2: None,
            })
        }
        _ => PyError::from_io_error(err, None),
    }
}

fn io_error_to_errno(err: &std::io::Error) -> i64 {
    if let Some(code) = err.raw_os_error() {
        return i64::from(code);
    }
    match err.kind() {
        ErrorKind::ConnectionRefused => plat::ECONNREFUSED,
        ErrorKind::TimedOut => plat::ETIMEDOUT,
        ErrorKind::WouldBlock => plat::EAGAIN,
        _ => plat::EINVAL,
    }
}

// ── Native socket handle state ───────────────────────────────────────────────

static NEXT_SYNTHETIC_FD: AtomicI64 = AtomicI64::new(100_000);

#[derive(Default)]
struct SocketOptions {
    reuse_addr: i32,
    tcp_nodelay: i32,
    keepalive: i32,
    broadcast: i32,
    v6only: i32,
    rcvbuf: Option<i32>,
    sndbuf: Option<i32>,
}

enum SocketBackend {
    UnboundTcp,
    BoundTcp {
        listener: TcpListener,
        listening: bool,
    },
    ConnectedTcp {
        stream: TcpStream,
    },
    UnboundUdp,
    BoundUdp {
        sock: UdpSocket,
        connected_peer: Option<SocketAddr>,
    },
    Closed,
}

struct SocketHandleState {
    family: i64,
    sock_type: i64,
    proto: i64,
    synthetic_fd: i64,
    /// `None` = blocking; `Some(Duration::ZERO)` = non-blocking; `Some(d)` = timed.
    timeout: Option<Duration>,
    opts: SocketOptions,
    backend: SocketBackend,
}

impl SocketHandleState {
    fn raw_handle(&self) -> Option<i64> {
        match &self.backend {
            SocketBackend::BoundTcp { listener, .. } => Some(tcp_listener_raw(listener)),
            SocketBackend::ConnectedTcp { stream } => Some(tcp_stream_raw(stream)),
            SocketBackend::BoundUdp { sock, .. } => Some(udp_socket_raw(sock)),
            SocketBackend::UnboundTcp | SocketBackend::UnboundUdp | SocketBackend::Closed => None,
        }
    }

    fn fileno(&self) -> i64 {
        match &self.backend {
            SocketBackend::Closed => -1,
            _ => self.raw_handle().unwrap_or(self.synthetic_fd),
        }
    }

    fn apply_timeout_to_backend(&self) -> Result<()> {
        let nonblocking = matches!(self.timeout, Some(d) if d.is_zero());
        let io_timeout = match self.timeout {
            None => None,
            Some(d) if d.is_zero() => None,
            Some(d) => Some(d),
        };
        match &self.backend {
            SocketBackend::BoundTcp { listener, .. } => {
                listener
                    .set_nonblocking(nonblocking)
                    .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
            }
            SocketBackend::ConnectedTcp { stream } => {
                stream
                    .set_nonblocking(nonblocking)
                    .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
                if !nonblocking {
                    stream
                        .set_read_timeout(io_timeout)
                        .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
                    stream
                        .set_write_timeout(io_timeout)
                        .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
                }
            }
            SocketBackend::BoundUdp { sock, .. } => {
                sock.set_nonblocking(nonblocking)
                    .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
                if !nonblocking {
                    sock.set_read_timeout(io_timeout)
                        .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
                    sock.set_write_timeout(io_timeout)
                        .map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
                }
            }
            SocketBackend::UnboundTcp | SocketBackend::UnboundUdp | SocketBackend::Closed => {}
        }
        Ok(())
    }

    fn apply_saved_options_to_backend(&self) {
        if let Some(raw) = self.raw_handle() {
            if self.opts.reuse_addr != 0 {
                let _ = raw_setsockopt_int(
                    raw,
                    plat::SOL_SOCKET as i32,
                    plat::SO_REUSEADDR as i32,
                    self.opts.reuse_addr,
                );
            }
            if self.opts.keepalive != 0 {
                let _ = raw_setsockopt_int(
                    raw,
                    plat::SOL_SOCKET as i32,
                    plat::SO_KEEPALIVE as i32,
                    self.opts.keepalive,
                );
            }
            if self.opts.broadcast != 0 {
                let _ = raw_setsockopt_int(
                    raw,
                    plat::SOL_SOCKET as i32,
                    plat::SO_BROADCAST as i32,
                    self.opts.broadcast,
                );
            }
            if let Some(sz) = self.opts.rcvbuf {
                let _ =
                    raw_setsockopt_int(raw, plat::SOL_SOCKET as i32, plat::SO_RCVBUF as i32, sz);
            }
            if let Some(sz) = self.opts.sndbuf {
                let _ =
                    raw_setsockopt_int(raw, plat::SOL_SOCKET as i32, plat::SO_SNDBUF as i32, sz);
            }
        }
        if let SocketBackend::ConnectedTcp { stream } = &self.backend
            && self.opts.tcp_nodelay != 0
        {
            let _ = stream.set_nodelay(true);
        }
    }

    fn ensure_bound_udp(&mut self) -> Result<()> {
        if matches!(self.backend, SocketBackend::UnboundUdp) {
            let bind_addr: SocketAddr = if self.family == plat::AF_INET6 {
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
            } else {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
            };
            let sock =
                UdpSocket::bind(bind_addr).map_err(|e| io_error_to_socket_py(&e, self.timeout))?;
            self.backend = SocketBackend::BoundUdp {
                sock,
                connected_peer: None,
            };
            self.apply_saved_options_to_backend();
            self.apply_timeout_to_backend()?;
        }
        Ok(())
    }
}

struct SocketHandleOps;
const SOCKET_HANDLE_OPS: &SocketHandleOps = &SocketHandleOps;

impl BuiltinTypeOps for SocketHandleOps {
    fn type_name(&self) -> &'static str {
        "_socket_handle"
    }

    fn is_internal_storage(&self) -> bool {
        true
    }
}

fn wrap_socket_handle(state: SocketHandleState) -> Value {
    let boxed: Box<dyn Any> = Box::new(state);
    Value::builtin_object(SOCKET_HANDLE_OPS, boxed)
}

fn with_socket<R>(val: &Value, f: impl FnOnce(&SocketHandleState) -> Result<R>) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "expected internal socket handle".to_string(),
        ));
    };
    if !builtin_ops_is::<SocketHandleOps>(ops) {
        return Err(PyError::named(
            "TypeError",
            "expected internal socket handle".to_string(),
        ));
    }
    let borrowed = state.borrow();
    let sock = borrowed
        .downcast_ref::<SocketHandleState>()
        .ok_or_else(|| PyError::runtime("corrupt internal socket handle".to_string()))?;
    f(sock)
}

fn with_socket_mut<R>(
    val: &Value,
    f: impl FnOnce(&mut SocketHandleState) -> Result<R>,
) -> Result<R> {
    let ValueKind::BuiltinObject { ops, state } = val.kind() else {
        return Err(PyError::named(
            "TypeError",
            "expected internal socket handle".to_string(),
        ));
    };
    if !builtin_ops_is::<SocketHandleOps>(ops) {
        return Err(PyError::named(
            "TypeError",
            "expected internal socket handle".to_string(),
        ));
    }
    let mut borrowed = state.borrow_mut();
    let sock = borrowed
        .downcast_mut::<SocketHandleState>()
        .ok_or_else(|| PyError::runtime("corrupt internal socket handle".to_string()))?;
    f(sock)
}

// ── Cross-module helpers for native `ssl.SSLSocket` ──────────────────────────

/// Move the live `SocketBackend` (and socket metadata/options/timeout) out of
/// `handle_val` into a fresh `_socket_handle`, leaving `handle_val` closed
/// (matching CPython's `sock.detach()` inside `SSLContext.wrap_socket`).
pub(crate) fn detach_socket_handle(handle_val: &Value) -> Result<Value> {
    with_socket_mut(handle_val, |sock| {
        let backend = std::mem::replace(&mut sock.backend, SocketBackend::Closed);
        let opts = std::mem::take(&mut sock.opts);
        let new_state = SocketHandleState {
            family: sock.family,
            sock_type: sock.sock_type,
            proto: sock.proto,
            synthetic_fd: sock.synthetic_fd,
            timeout: sock.timeout,
            opts,
            backend,
        };
        Ok(wrap_socket_handle(new_state))
    })
}

pub(crate) fn is_tcp_connected(handle_val: &Value) -> Result<bool> {
    with_socket(handle_val, |sock| {
        Ok(matches!(sock.backend, SocketBackend::ConnectedTcp { .. }))
    })
}

pub(crate) fn is_closed_handle(handle_val: &Value) -> bool {
    with_socket(handle_val, |sock| {
        Ok(matches!(sock.backend, SocketBackend::Closed))
    })
    .unwrap_or(true)
}

pub(crate) fn handle_timeout(handle_val: &Value) -> Option<Duration> {
    with_socket(handle_val, |sock| Ok(sock.timeout))
        .ok()
        .flatten()
}

pub(crate) fn handle_is_nonblocking(handle_val: &Value) -> bool {
    matches!(handle_timeout(handle_val), Some(d) if d.is_zero())
}

pub(crate) fn handle_read_raw(
    handle_val: &Value,
    buf: &mut [u8],
) -> std::result::Result<usize, std::io::Error> {
    let mut out_err: Option<std::io::Error> = None;
    let res = with_socket_mut(handle_val, |sock| match &mut sock.backend {
        SocketBackend::ConnectedTcp { stream } => loop {
            match stream.read(buf) {
                Ok(n) => return Ok(Some(n)),
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    out_err = Some(e);
                    return Ok(None);
                }
            }
        },
        SocketBackend::Closed => {
            out_err = Some(std::io::Error::from_raw_os_error(plat::EBADF as i32));
            Ok(None)
        }
        _ => {
            out_err = Some(std::io::Error::from_raw_os_error(plat::ENOTCONN as i32));
            Ok(None)
        }
    });
    match (res, out_err) {
        (Ok(Some(n)), _) => Ok(n),
        (_, Some(e)) => Err(e),
        _ => Err(std::io::Error::from_raw_os_error(plat::EINVAL as i32)),
    }
}

pub(crate) fn handle_write_raw(
    handle_val: &Value,
    buf: &[u8],
) -> std::result::Result<usize, std::io::Error> {
    let mut out_err: Option<std::io::Error> = None;
    let res = with_socket_mut(handle_val, |sock| match &mut sock.backend {
        SocketBackend::ConnectedTcp { stream } => loop {
            match stream.write(buf) {
                Ok(n) => return Ok(Some(n)),
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    out_err = Some(e);
                    return Ok(None);
                }
            }
        },
        SocketBackend::Closed => {
            out_err = Some(std::io::Error::from_raw_os_error(plat::EBADF as i32));
            Ok(None)
        }
        _ => {
            out_err = Some(std::io::Error::from_raw_os_error(plat::ENOTCONN as i32));
            Ok(None)
        }
    });
    match (res, out_err) {
        (Ok(Some(n)), _) => Ok(n),
        (_, Some(e)) => Err(e),
        _ => Err(std::io::Error::from_raw_os_error(plat::EINVAL as i32)),
    }
}

pub(crate) fn handle_connect_raw(
    handle_val: &Value,
    addr: &Value,
    gaierror_cls: &Value,
) -> Result<()> {
    with_socket_mut(handle_val, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        let resolved = resolve_socket_addr(sock.family, addr, false, gaierror_cls)?;
        let timeout = sock.timeout;
        connect_inner(sock, resolved).map_err(|e| io_error_to_socket_py(&e, timeout))
    })
}

pub(crate) fn handle_connect_ex_raw(
    handle_val: &Value,
    addr: &Value,
    gaierror_cls: &Value,
) -> Result<i64> {
    with_socket_mut(handle_val, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        let resolved = resolve_socket_addr(sock.family, addr, false, gaierror_cls)?;
        match connect_inner(sock, resolved) {
            Ok(()) => Ok(0),
            Err(e) => Ok(io_error_to_errno(&e)),
        }
    })
}

pub(crate) fn handle_accept_raw(
    handle_val: &Value,
    default_timeout_val: &Value,
) -> Result<(Value, Value)> {
    let default_timeout = parse_timeout_value(default_timeout_val)?;
    with_socket(handle_val, |sock| {
        let SocketBackend::BoundTcp {
            listener,
            listening,
        } = &sock.backend
        else {
            return if matches!(sock.backend, SocketBackend::Closed) {
                Err(closed_socket_error())
            } else {
                Err(os_error(plat::EINVAL, "Invalid argument"))
            };
        };
        if !*listening {
            return Err(os_error(plat::EINVAL, "Invalid argument"));
        }
        if let Some(dur) = sock.timeout
            && !dur.is_zero()
        {
            let raw = tcp_listener_raw(listener);
            let (ready_r, _) = select_raw_handles(&[raw], &[], Some(dur))?;
            if ready_r.is_empty() {
                return Err(PyError::named("TimeoutError", "timed out".to_string()));
            }
        }
        let (stream, peer_addr) = listener
            .accept()
            .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
        let new_state = SocketHandleState {
            family: sock.family,
            sock_type: sock.sock_type,
            proto: sock.proto,
            synthetic_fd: NEXT_SYNTHETIC_FD.fetch_add(1, Ordering::Relaxed),
            timeout: default_timeout,
            opts: SocketOptions::default(),
            backend: SocketBackend::ConnectedTcp { stream },
        };
        new_state.apply_timeout_to_backend()?;
        Ok((wrap_socket_handle(new_state), sockaddr_to_py(peer_addr)))
    })
}

pub(crate) fn handle_shutdown_raw(handle_val: &Value, how: i64) -> Result<()> {
    let mode = match how {
        x if x == plat::SHUT_RD => Shutdown::Read,
        x if x == plat::SHUT_WR => Shutdown::Write,
        x if x == plat::SHUT_RDWR => Shutdown::Both,
        _ => return Err(os_error(plat::EINVAL, "Invalid argument")),
    };
    with_socket(handle_val, |sock| match &sock.backend {
        SocketBackend::Closed => Err(closed_socket_error()),
        SocketBackend::ConnectedTcp { stream } => stream
            .shutdown(mode)
            .map_err(|e| io_error_to_socket_py(&e, sock.timeout)),
        _ => Err(os_error(
            plat::ENOTCONN,
            "Transport endpoint is not connected",
        )),
    })
}

pub(crate) fn handle_close_raw(handle_val: &Value) -> Result<()> {
    with_socket_mut(handle_val, |sock| {
        sock.backend = SocketBackend::Closed;
        Ok(())
    })
}

pub(crate) fn io_error_to_socket_py_pub(
    err: &std::io::Error,
    timeout: Option<Duration>,
) -> PyError {
    io_error_to_socket_py(err, timeout)
}

pub(crate) fn extract_writable_buffer_pub(val: &Value) -> Result<WritableBufferRange> {
    extract_writable_buffer(val)
}

// ── Address / argument conversion helpers ────────────────────────────────────

fn extract_i64(val: &Value, what: &str) -> Result<i64> {
    match val.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{what} must be an integer, not {}",
                value_type_name_str(val)
            ),
        )),
    }
}

fn extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = val.kind() {
        return Some(rc.as_ref().clone());
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        return Some(snap);
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_bytes_checked(val).ok();
    }
    if let ValueKind::PyInstance(inst_rc) = val.kind() {
        let inst = inst_rc.borrow();
        let is_array = {
            let cls = inst.class.borrow();
            cls.name == "array"
                && cls
                    .attrs
                    .get("__module__")
                    .and_then(|v| v.as_str())
                    .is_some_and(|m| m == "array")
        };
        if is_array
            && let Some(buf) = inst.attrs.get("_buf")
            && let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(buf)
        {
            return Some(snap);
        }
    }
    None
}

type WritableBufferRange = (Rc<RefCell<Vec<u8>>>, usize, usize);

fn extract_writable_buffer(val: &Value) -> Result<WritableBufferRange> {
    if let Some(rc) = pyrust_builtins::bytearray::as_bytearray_rc(val) {
        let len = rc.borrow().len();
        return Ok((rc, 0, len));
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::writable_contiguous_range(val);
    }
    if let ValueKind::PyInstance(inst_rc) = val.kind() {
        let inst = inst_rc.borrow();
        let is_array = {
            let cls = inst.class.borrow();
            cls.name == "array"
                && cls
                    .attrs
                    .get("__module__")
                    .and_then(|v| v.as_str())
                    .is_some_and(|m| m == "array")
        };
        if is_array
            && let Some(buf) = inst.attrs.get("_buf")
            && let Some(rc) = pyrust_builtins::bytearray::as_bytearray_rc(buf)
        {
            let len = rc.borrow().len();
            return Ok((rc, 0, len));
        }
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "recv_into() argument 1 must be read-write bytes-like object, not {}",
            value_type_name_str(val)
        ),
    ))
}

fn sockaddr_to_py(addr: SocketAddr) -> Value {
    match addr {
        SocketAddr::V4(v4) => Value::tuple(vec![
            Value::string(v4.ip().to_string()),
            Value::int(i64::from(v4.port())),
        ]),
        SocketAddr::V6(v6) => Value::tuple(vec![
            Value::string(v6.ip().to_string()),
            Value::int(i64::from(v6.port())),
            Value::int(i64::from(v6.flowinfo())),
            Value::int(i64::from(v6.scope_id())),
        ]),
    }
}

fn parse_port(val: &Value) -> Result<u16> {
    let p = extract_i64(val, "port")?;
    if !(0..=65535).contains(&p) {
        return Err(PyError::named(
            "OverflowError",
            "bind(): port must be 0-65535.".to_string(),
        ));
    }
    Ok(p as u16)
}

fn resolve_socket_addr(
    family: i64,
    addr_val: &Value,
    for_bind: bool,
    gaierror_cls: &Value,
) -> Result<SocketAddr> {
    let ValueKind::Tuple(items) = addr_val.kind() else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "address must be a tuple, not {}",
                value_type_name_str(addr_val)
            ),
        ));
    };

    if family == plat::AF_INET {
        if items.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "AF_INET address must be a pair (host, port), not tuple of length {}",
                    items.len()
                ),
            ));
        }
        let host = match items[0].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "address host must be str, not {}",
                        value_type_name_str(&items[0])
                    ),
                ));
            }
        };
        let port = parse_port(&items[1])?;
        if host.is_empty() {
            let ip = if for_bind {
                Ipv4Addr::UNSPECIFIED
            } else {
                Ipv4Addr::LOCALHOST
            };
            return Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)));
        }
        if host == "<broadcast>" {
            return Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, port)));
        }
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)));
        }
        let addrs = (host.as_str(), port).to_socket_addrs().map_err(|_| {
            raise_gaierror(gaierror_cls, plat::EAI_NONAME, "Name or service not known")
        })?;
        for a in addrs {
            if a.is_ipv4() {
                return Ok(a);
            }
        }
        return Err(raise_gaierror(
            gaierror_cls,
            plat::EAI_NONAME,
            "Name or service not known",
        ));
    }

    if family == plat::AF_INET6 {
        if !(items.len() == 2 || items.len() == 3 || items.len() == 4) {
            return Err(PyError::named(
                "TypeError",
                "AF_INET6 address must be a 2-, 3-, or 4-tuple".to_string(),
            ));
        }
        let host = match items[0].kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "address host must be str, not {}",
                        value_type_name_str(&items[0])
                    ),
                ));
            }
        };
        let port = parse_port(&items[1])?;
        let flowinfo = if items.len() >= 3 {
            extract_i64(&items[2], "flowinfo")? as u32
        } else {
            0
        };
        let scope_id = if items.len() >= 4 {
            extract_i64(&items[3], "scope_id")? as u32
        } else {
            0
        };
        if host.is_empty() {
            let ip = if for_bind {
                Ipv6Addr::UNSPECIFIED
            } else {
                Ipv6Addr::LOCALHOST
            };
            return Ok(SocketAddr::V6(SocketAddrV6::new(
                ip, port, flowinfo, scope_id,
            )));
        }
        if let Ok(ip) = host.parse::<Ipv6Addr>() {
            return Ok(SocketAddr::V6(SocketAddrV6::new(
                ip, port, flowinfo, scope_id,
            )));
        }
        let addrs = (host.as_str(), port).to_socket_addrs().map_err(|_| {
            raise_gaierror(gaierror_cls, plat::EAI_NONAME, "Name or service not known")
        })?;
        for a in addrs {
            if let SocketAddr::V6(v6) = a {
                return Ok(SocketAddr::V6(SocketAddrV6::new(
                    *v6.ip(),
                    v6.port(),
                    flowinfo,
                    scope_id,
                )));
            }
        }
        return Err(raise_gaierror(
            gaierror_cls,
            plat::EAI_NONAME,
            "Name or service not known",
        ));
    }

    Err(os_error(
        plat::EAFNOSUPPORT,
        "Address family not supported by protocol",
    ))
}

fn parse_timeout_value(val: &Value) -> Result<Option<Duration>> {
    match val.kind() {
        ValueKind::None => Ok(None),
        ValueKind::Int(n) => {
            if n < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "Timeout value out of range".to_string(),
                ));
            }
            Ok(Some(Duration::from_secs(n as u64)))
        }
        ValueKind::Bool(b) => Ok(Some(Duration::from_secs(u64::from(b)))),
        ValueKind::Float(f) => {
            if f.is_nan() || f.is_infinite() || f < 0.0 {
                return Err(PyError::named(
                    "ValueError",
                    "Timeout value out of range".to_string(),
                ));
            }
            Ok(Some(Duration::from_secs_f64(f)))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        )),
    }
}

fn timeout_to_py(timeout: Option<Duration>) -> Value {
    match timeout {
        None => Value::none(),
        Some(d) => Value::float(d.as_secs_f64()),
    }
}

/// Parse BSD `inet_aton` dotted-notation IPv4 (1, 2, 3, or 4 parts; decimal,
/// octal `0...`, or hex `0x...`).
fn parse_inet_aton(s: &str) -> Option<[u8; 4]> {
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut vals = [0u64; 4];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return None;
        }
        let val = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            u64::from_str_radix(hex, 16).ok()?
        } else if part.starts_with('0') && part.len() > 1 {
            if !part.bytes().all(|b| matches!(b, b'0'..=b'7')) {
                return None;
            }
            u64::from_str_radix(part, 8).ok()?
        } else {
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse::<u64>().ok()?
        };
        vals[i] = val;
    }
    let ip: u32 = match parts.len() {
        1 => {
            if vals[0] > 0xFFFF_FFFF {
                return None;
            }
            vals[0] as u32
        }
        2 => {
            if vals[0] > 0xFF || vals[1] > 0xFF_FFFF {
                return None;
            }
            ((vals[0] << 24) | vals[1]) as u32
        }
        3 => {
            if vals[0] > 0xFF || vals[1] > 0xFF || vals[2] > 0xFFFF {
                return None;
            }
            ((vals[0] << 24) | (vals[1] << 16) | vals[2]) as u32
        }
        4 => {
            if vals[0] > 0xFF || vals[1] > 0xFF || vals[2] > 0xFF || vals[3] > 0xFF {
                return None;
            }
            ((vals[0] << 24) | (vals[1] << 16) | (vals[2] << 8) | vals[3]) as u32
        }
        _ => return None,
    };
    Some(ip.to_be_bytes())
}

// ── Module injection ─────────────────────────────────────────────────────────

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in SOCKET_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        SOCKET_PY_SOURCE,
        "<frozen socket>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("socket: exec namespace not a dict".into()))?;
    for &name in SOCKET_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind()
                && !matches!(name, "error" | "timeout")
            {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("socket"));
                class.error_name = match name {
                    "gaierror" => Some("socket.gaierror"),
                    "herror" => Some("socket.herror"),
                    _ => None,
                };
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Native helper implementations ────────────────────────────────────────────

fn constants_map_impl() -> Result<Value> {
    let dict = Value::dict(crate::value::PyDict::default());
    let entries: &[(&str, i64)] = &[
        ("AF_UNSPEC", plat::AF_UNSPEC),
        ("AF_INET", plat::AF_INET),
        ("AF_INET6", plat::AF_INET6),
        ("SOCK_STREAM", plat::SOCK_STREAM),
        ("SOCK_DGRAM", plat::SOCK_DGRAM),
        ("SOL_SOCKET", plat::SOL_SOCKET),
        ("SO_REUSEADDR", plat::SO_REUSEADDR),
        ("SO_KEEPALIVE", plat::SO_KEEPALIVE),
        ("SO_BROADCAST", plat::SO_BROADCAST),
        ("SO_RCVBUF", plat::SO_RCVBUF),
        ("SO_SNDBUF", plat::SO_SNDBUF),
        ("SO_ERROR", plat::SO_ERROR),
        ("IPPROTO_IP", plat::IPPROTO_IP),
        ("IPPROTO_TCP", plat::IPPROTO_TCP),
        ("IPPROTO_UDP", plat::IPPROTO_UDP),
        ("IPPROTO_IPV6", plat::IPPROTO_IPV6),
        ("TCP_NODELAY", plat::TCP_NODELAY),
        ("IPV6_V6ONLY", plat::IPV6_V6ONLY),
        ("SHUT_RD", plat::SHUT_RD),
        ("SHUT_WR", plat::SHUT_WR),
        ("SHUT_RDWR", plat::SHUT_RDWR),
        ("SOMAXCONN", plat::SOMAXCONN),
        ("AI_PASSIVE", plat::AI_PASSIVE),
        ("AI_CANONNAME", plat::AI_CANONNAME),
        ("AI_NUMERICHOST", plat::AI_NUMERICHOST),
        ("AI_NUMERICSERV", plat::AI_NUMERICSERV),
        ("MSG_PEEK", plat::MSG_PEEK),
    ];
    for &(k, v) in entries {
        dict.dict_insert(PyKey::str_from(k), Value::int(v))?;
    }
    Ok(dict)
}

fn sock_new_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 4 {
        return Err(PyError::named(
            "TypeError",
            "_sock_new() expects 4 arguments".to_string(),
        ));
    }
    let family = extract_i64(&args[0].value, "family")?;
    let sock_type = extract_i64(&args[1].value, "type")?;
    let proto = extract_i64(&args[2].value, "proto")?;
    let timeout = parse_timeout_value(&args[3].value)?;

    if family != plat::AF_INET && family != plat::AF_INET6 {
        return Err(os_error(
            plat::EAFNOSUPPORT,
            "Address family not supported by protocol",
        ));
    }
    let backend = match sock_type {
        t if t == plat::SOCK_STREAM => SocketBackend::UnboundTcp,
        t if t == plat::SOCK_DGRAM => SocketBackend::UnboundUdp,
        _ => {
            return Err(os_error(plat::ESOCKTNOSUPPORT, "Socket type not supported"));
        }
    };
    if proto != 0
        && !((sock_type == plat::SOCK_STREAM && proto == plat::IPPROTO_TCP)
            || (sock_type == plat::SOCK_DGRAM && proto == plat::IPPROTO_UDP))
    {
        return Err(os_error(plat::EPROTONOSUPPORT, "Protocol not supported"));
    }
    let synthetic_fd = NEXT_SYNTHETIC_FD.fetch_add(1, Ordering::Relaxed);
    Ok(wrap_socket_handle(SocketHandleState {
        family,
        sock_type,
        proto,
        synthetic_fd,
        timeout,
        opts: SocketOptions::default(),
        backend,
    }))
}

fn sock_bind_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_bind() expects 3 arguments".to_string(),
        ));
    }
    let gaierror_cls = &args[2].value;
    with_socket_mut(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        let addr = resolve_socket_addr(sock.family, &args[1].value, true, gaierror_cls)?;
        match &sock.backend {
            SocketBackend::UnboundTcp => {
                let listener =
                    TcpListener::bind(addr).map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
                sock.backend = SocketBackend::BoundTcp {
                    listener,
                    listening: false,
                };
                sock.apply_saved_options_to_backend();
                sock.apply_timeout_to_backend()?;
                Ok(Value::none())
            }
            SocketBackend::UnboundUdp => {
                let udp =
                    UdpSocket::bind(addr).map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
                sock.backend = SocketBackend::BoundUdp {
                    sock: udp,
                    connected_peer: None,
                };
                sock.apply_saved_options_to_backend();
                sock.apply_timeout_to_backend()?;
                Ok(Value::none())
            }
            _ => Err(os_error(plat::EINVAL, "Invalid argument")),
        }
    })
}

fn sock_listen_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            "_sock_listen() expects 2 arguments".to_string(),
        ));
    }
    let _backlog = extract_i64(&args[1].value, "backlog")?;
    with_socket_mut(&args[0].value, |sock| match &mut sock.backend {
        SocketBackend::Closed => Err(closed_socket_error()),
        SocketBackend::BoundTcp { listening, .. } => {
            *listening = true;
            Ok(Value::none())
        }
        SocketBackend::UnboundTcp => {
            let bind_addr: SocketAddr = if sock.family == plat::AF_INET6 {
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
            } else {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
            };
            let listener = TcpListener::bind(bind_addr)
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            sock.backend = SocketBackend::BoundTcp {
                listener,
                listening: true,
            };
            sock.apply_saved_options_to_backend();
            sock.apply_timeout_to_backend()?;
            Ok(Value::none())
        }
        _ => Err(os_error(plat::EINVAL, "Operation not supported on socket")),
    })
}

fn sock_accept_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            "_sock_accept() expects 2 arguments".to_string(),
        ));
    }
    let default_timeout = parse_timeout_value(&args[1].value)?;
    let is_nonblocking = with_socket(&args[0].value, |sock| {
        Ok(sock.timeout.is_some_and(|d| d.is_zero()))
    })
    .unwrap_or(false);
    if !interp.cold.gil_state.is_multi_threaded() || is_nonblocking {
        return with_socket(&args[0].value, |sock| {
            let SocketBackend::BoundTcp {
                listener,
                listening,
            } = &sock.backend
            else {
                return if matches!(sock.backend, SocketBackend::Closed) {
                    Err(closed_socket_error())
                } else {
                    Err(os_error(plat::EINVAL, "Invalid argument"))
                };
            };
            if !*listening {
                return Err(os_error(plat::EINVAL, "Invalid argument"));
            }
            if let Some(dur) = sock.timeout
                && !dur.is_zero()
            {
                let raw = tcp_listener_raw(listener);
                let (ready_r, _) = select_raw_handles(&[raw], &[], Some(dur))?;
                if ready_r.is_empty() {
                    return Err(PyError::named("TimeoutError", "timed out".to_string()));
                }
            }
            let (stream, peer_addr) = listener
                .accept()
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            let new_state = SocketHandleState {
                family: sock.family,
                sock_type: sock.sock_type,
                proto: sock.proto,
                synthetic_fd: NEXT_SYNTHETIC_FD.fetch_add(1, Ordering::Relaxed),
                timeout: default_timeout,
                opts: SocketOptions::default(),
                backend: SocketBackend::ConnectedTcp { stream },
            };
            new_state.apply_timeout_to_backend()?;
            Ok(Value::tuple(vec![
                wrap_socket_handle(new_state),
                sockaddr_to_py(peer_addr),
            ]))
        });
    }
    let (cloned_listener, family, sock_type, proto, timeout) =
        with_socket(&args[0].value, |sock| {
            let SocketBackend::BoundTcp {
                listener,
                listening,
            } = &sock.backend
            else {
                return if matches!(sock.backend, SocketBackend::Closed) {
                    Err(closed_socket_error())
                } else {
                    Err(os_error(plat::EINVAL, "Invalid argument"))
                };
            };
            if !*listening {
                return Err(os_error(plat::EINVAL, "Invalid argument"));
            }
            let cloned = listener
                .try_clone()
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            Ok((
                cloned,
                sock.family,
                sock.sock_type,
                sock.proto,
                sock.timeout,
            ))
        })?;

    let accept_res = interp.release_gil_during(|| {
        if let Some(dur) = timeout
            && !dur.is_zero()
        {
            let raw = tcp_listener_raw(&cloned_listener);
            let (ready_r, _) = select_raw_handles(&[raw], &[], Some(dur))?;
            if ready_r.is_empty() {
                return Err(PyError::named("TimeoutError", "timed out".to_string()));
            }
        }
        cloned_listener
            .accept()
            .map_err(|e| io_error_to_socket_py(&e, timeout))
    });
    let (stream, peer_addr) = accept_res?;
    let new_state = SocketHandleState {
        family,
        sock_type,
        proto,
        synthetic_fd: NEXT_SYNTHETIC_FD.fetch_add(1, Ordering::Relaxed),
        timeout: default_timeout,
        opts: SocketOptions::default(),
        backend: SocketBackend::ConnectedTcp { stream },
    };
    new_state.apply_timeout_to_backend()?;
    Ok(Value::tuple(vec![
        wrap_socket_handle(new_state),
        sockaddr_to_py(peer_addr),
    ]))
}

fn connect_inner(
    sock: &mut SocketHandleState,
    addr: SocketAddr,
) -> std::result::Result<(), std::io::Error> {
    match &mut sock.backend {
        SocketBackend::UnboundTcp => {
            let stream = match sock.timeout {
                None => TcpStream::connect(addr)?,
                Some(d) if d.is_zero() => {
                    TcpStream::connect_timeout(&addr, Duration::from_millis(50))?
                }
                Some(d) => TcpStream::connect_timeout(&addr, d)?,
            };
            sock.backend = SocketBackend::ConnectedTcp { stream };
            sock.apply_saved_options_to_backend();
            let _ = sock.apply_timeout_to_backend();
            Ok(())
        }
        SocketBackend::BoundTcp {
            listening: false, ..
        } => {
            // `bind()` followed by `connect()` (e.g. `create_connection(..., source_address=...)`).
            sock.backend = SocketBackend::UnboundTcp;
            let stream = match sock.timeout {
                None => TcpStream::connect(addr)?,
                Some(d) if d.is_zero() => {
                    TcpStream::connect_timeout(&addr, Duration::from_millis(50))?
                }
                Some(d) => TcpStream::connect_timeout(&addr, d)?,
            };
            sock.backend = SocketBackend::ConnectedTcp { stream };
            sock.apply_saved_options_to_backend();
            let _ = sock.apply_timeout_to_backend();
            Ok(())
        }
        SocketBackend::UnboundUdp => {
            let bind_addr: SocketAddr = if sock.family == plat::AF_INET6 {
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
            } else {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
            };
            let udp = UdpSocket::bind(bind_addr)?;
            udp.connect(addr)?;
            sock.backend = SocketBackend::BoundUdp {
                sock: udp,
                connected_peer: Some(addr),
            };
            sock.apply_saved_options_to_backend();
            let _ = sock.apply_timeout_to_backend();
            Ok(())
        }
        SocketBackend::BoundUdp {
            sock: udp,
            connected_peer,
        } => {
            udp.connect(addr)?;
            *connected_peer = Some(addr);
            Ok(())
        }
        _ => Err(std::io::Error::from_raw_os_error(plat::EINVAL as i32)),
    }
}

fn sock_connect_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_connect() expects 3 arguments".to_string(),
        ));
    }
    let gaierror_cls = &args[2].value;
    with_socket_mut(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        let addr = resolve_socket_addr(sock.family, &args[1].value, false, gaierror_cls)?;
        let timeout = sock.timeout;
        connect_inner(sock, addr).map_err(|e| io_error_to_socket_py(&e, timeout))?;
        Ok(Value::none())
    })
}

fn sock_connect_ex_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_connect_ex() expects 3 arguments".to_string(),
        ));
    }
    let gaierror_cls = &args[2].value;
    with_socket_mut(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        let addr = resolve_socket_addr(sock.family, &args[1].value, false, gaierror_cls)?;
        match connect_inner(sock, addr) {
            Ok(()) => Ok(Value::int(0)),
            Err(e) => Ok(Value::int(io_error_to_errno(&e))),
        }
    })
}

fn sock_send_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_send() expects 3 arguments".to_string(),
        ));
    }
    let data = extract_bytes_like(&args[1].value).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(&args[1].value)
            ),
        )
    })?;
    let _flags = extract_i64(&args[2].value, "flags")?;
    with_socket_mut(&args[0].value, |sock| {
        let timeout = sock.timeout;
        match &mut sock.backend {
            SocketBackend::Closed => Err(closed_socket_error()),
            SocketBackend::ConnectedTcp { stream } => {
                let n = stream
                    .write(&data)
                    .map_err(|e| io_error_to_socket_py(&e, timeout))?;
                Ok(Value::int(n as i64))
            }
            SocketBackend::BoundUdp {
                sock: udp,
                connected_peer: Some(_),
            } => {
                let n = udp
                    .send(&data)
                    .map_err(|e| io_error_to_socket_py(&e, timeout))?;
                Ok(Value::int(n as i64))
            }
            SocketBackend::BoundUdp {
                connected_peer: None,
                ..
            }
            | SocketBackend::UnboundUdp => {
                Err(os_error(plat::EDESTADDRREQ, "Destination address required"))
            }
            SocketBackend::UnboundTcp | SocketBackend::BoundTcp { .. } => Err(os_error(
                plat::ENOTCONN,
                "Transport endpoint is not connected",
            )),
        }
    })
}

fn sock_sendall_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_sendall() expects 3 arguments".to_string(),
        ));
    }
    let data = extract_bytes_like(&args[1].value).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(&args[1].value)
            ),
        )
    })?;
    let _flags = extract_i64(&args[2].value, "flags")?;
    with_socket_mut(&args[0].value, |sock| {
        let timeout = sock.timeout;
        match &mut sock.backend {
            SocketBackend::Closed => Err(closed_socket_error()),
            SocketBackend::ConnectedTcp { stream } => {
                let mut written = 0;
                while written < data.len() {
                    match stream.write(&data[written..]) {
                        Ok(0) => {
                            return Err(PyError::new(PyErrorKind::OsError {
                                class_name: "BrokenPipeError",
                                errno: 32,
                                strerror: "Broken pipe".to_string(),
                                filename: None,
                                filename2: None,
                            }));
                        }
                        Ok(n) => written += n,
                        Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                    }
                }
                Ok(Value::none())
            }
            SocketBackend::BoundUdp {
                sock: udp,
                connected_peer: Some(_),
            } => {
                let mut written = 0;
                while written < data.len() {
                    match udp.send(&data[written..]) {
                        Ok(0) => break,
                        Ok(n) => written += n,
                        Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                    }
                }
                Ok(Value::none())
            }
            SocketBackend::BoundUdp {
                connected_peer: None,
                ..
            }
            | SocketBackend::UnboundUdp => {
                Err(os_error(plat::EDESTADDRREQ, "Destination address required"))
            }
            SocketBackend::UnboundTcp | SocketBackend::BoundTcp { .. } => Err(os_error(
                plat::ENOTCONN,
                "Transport endpoint is not connected",
            )),
        }
    })
}

fn sock_recv_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_recv() expects 3 arguments".to_string(),
        ));
    }
    let bufsize = extract_i64(&args[1].value, "bufsize")?;
    if bufsize < 0 {
        return Err(PyError::named(
            "ValueError",
            "negative buffersize in recv".to_string(),
        ));
    }
    let flags = extract_i64(&args[2].value, "flags")?;
    let peek = (flags & plat::MSG_PEEK) != 0;
    let size = (bufsize as usize).min(16 * 1024 * 1024);

    let is_nonblocking = with_socket(&args[0].value, |sock| {
        Ok(sock.timeout.is_some_and(|d| d.is_zero()))
    })
    .unwrap_or(false);
    if !interp.cold.gil_state.is_multi_threaded() || is_nonblocking {
        return with_socket_mut(&args[0].value, |sock| {
            let timeout = sock.timeout;
            match &mut sock.backend {
                SocketBackend::Closed => Err(closed_socket_error()),
                SocketBackend::ConnectedTcp { stream } => {
                    let mut buf = vec![0u8; size];
                    let n = loop {
                        let res = if peek {
                            stream.peek(&mut buf)
                        } else {
                            stream.read(&mut buf)
                        };
                        match res {
                            Ok(n) => break n,
                            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                            Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                        }
                    };
                    buf.truncate(n);
                    Ok(Value::bytes(buf))
                }
                SocketBackend::BoundUdp { sock: udp, .. } => {
                    let mut buf = vec![0u8; size];
                    let n = loop {
                        let res = if peek {
                            udp.peek(&mut buf)
                        } else {
                            udp.recv(&mut buf)
                        };
                        match res {
                            Ok(n) => break n,
                            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                            Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                        }
                    };
                    buf.truncate(n);
                    Ok(Value::bytes(buf))
                }
                _ => Err(os_error(
                    plat::ENOTCONN,
                    "Transport endpoint is not connected",
                )),
            }
        });
    }

    enum RecvHandle {
        Tcp(TcpStream),
        Udp(UdpSocket),
    }

    let (handle, timeout) = with_socket(&args[0].value, |sock| {
        let timeout = sock.timeout;
        match &sock.backend {
            SocketBackend::Closed => Err(closed_socket_error()),
            SocketBackend::ConnectedTcp { stream } => Ok((
                RecvHandle::Tcp(
                    stream
                        .try_clone()
                        .map_err(|e| io_error_to_socket_py(&e, timeout))?,
                ),
                timeout,
            )),
            SocketBackend::BoundUdp { sock: udp, .. } => Ok((
                RecvHandle::Udp(
                    udp.try_clone()
                        .map_err(|e| io_error_to_socket_py(&e, timeout))?,
                ),
                timeout,
            )),
            _ => Err(os_error(
                plat::ENOTCONN,
                "Transport endpoint is not connected",
            )),
        }
    })?;

    let data = interp.release_gil_during(|| match handle {
        RecvHandle::Tcp(mut stream) => {
            let mut buf = vec![0u8; size];
            let n = loop {
                let res = if peek {
                    stream.peek(&mut buf)
                } else {
                    stream.read(&mut buf)
                };
                match res {
                    Ok(n) => break n,
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                }
            };
            buf.truncate(n);
            Ok(buf)
        }
        RecvHandle::Udp(udp) => {
            let mut buf = vec![0u8; size];
            let n = loop {
                let res = if peek {
                    udp.peek(&mut buf)
                } else {
                    udp.recv(&mut buf)
                };
                match res {
                    Ok(n) => break n,
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                }
            };
            buf.truncate(n);
            Ok(buf)
        }
    })?;
    Ok(Value::bytes(data))
}

fn sock_recv_into_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 4 {
        return Err(PyError::named(
            "TypeError",
            "_sock_recv_into() expects 4 arguments".to_string(),
        ));
    }
    let (data_rc, offset, capacity) = extract_writable_buffer(&args[1].value)?;
    let nbytes_raw = if matches!(args[2].value.kind(), ValueKind::None) {
        0
    } else {
        extract_i64(&args[2].value, "nbytes")?
    };
    if nbytes_raw < 0 {
        return Err(PyError::named(
            "ValueError",
            "negative buffersize in recv_into".to_string(),
        ));
    }
    let nbytes = if nbytes_raw == 0 {
        capacity
    } else {
        let req = nbytes_raw as usize;
        if req > capacity {
            return Err(PyError::named(
                "ValueError",
                "nbytes is greater than the length of the buffer".to_string(),
            ));
        }
        req
    };
    let flags = extract_i64(&args[3].value, "flags")?;
    let peek = (flags & plat::MSG_PEEK) != 0;

    let read_bytes = with_socket_mut(&args[0].value, |sock| {
        let timeout = sock.timeout;
        let mut tmp = vec![0u8; nbytes];
        let n = match &mut sock.backend {
            SocketBackend::Closed => return Err(closed_socket_error()),
            SocketBackend::ConnectedTcp { stream } => loop {
                let res = if peek {
                    stream.peek(&mut tmp)
                } else {
                    stream.read(&mut tmp)
                };
                match res {
                    Ok(n) => break n,
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                }
            },
            SocketBackend::BoundUdp { sock: udp, .. } => loop {
                let res = if peek {
                    udp.peek(&mut tmp)
                } else {
                    udp.recv(&mut tmp)
                };
                match res {
                    Ok(n) => break n,
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                }
            },
            _ => {
                return Err(os_error(
                    plat::ENOTCONN,
                    "Transport endpoint is not connected",
                ));
            }
        };
        tmp.truncate(n);
        Ok(tmp)
    })?;

    let mut dst = data_rc.borrow_mut();
    let end = offset + read_bytes.len();
    if end > dst.len() {
        return Err(PyError::named(
            "BufferError",
            "underlying buffer shrunk during recv_into".to_string(),
        ));
    }
    dst[offset..end].copy_from_slice(&read_bytes);
    Ok(Value::int(read_bytes.len() as i64))
}

fn sock_sendto_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 5 {
        return Err(PyError::named(
            "TypeError",
            "_sock_sendto() expects 5 arguments".to_string(),
        ));
    }
    let data = extract_bytes_like(&args[1].value).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(&args[1].value)
            ),
        )
    })?;
    let _flags = extract_i64(&args[2].value, "flags")?;
    let gaierror_cls = &args[4].value;

    with_socket_mut(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        let target = resolve_socket_addr(sock.family, &args[3].value, false, gaierror_cls)?;
        sock.ensure_bound_udp()?;
        let timeout = sock.timeout;
        match &mut sock.backend {
            SocketBackend::BoundUdp { sock: udp, .. } => {
                let n = udp
                    .send_to(&data, target)
                    .map_err(|e| io_error_to_socket_py(&e, timeout))?;
                Ok(Value::int(n as i64))
            }
            SocketBackend::ConnectedTcp { stream } => {
                let n = stream
                    .write(&data)
                    .map_err(|e| io_error_to_socket_py(&e, timeout))?;
                Ok(Value::int(n as i64))
            }
            _ => Err(os_error(plat::EINVAL, "Invalid argument")),
        }
    })
}

fn sock_recvfrom_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_recvfrom() expects 3 arguments".to_string(),
        ));
    }
    let bufsize = extract_i64(&args[1].value, "bufsize")?;
    if bufsize < 0 {
        return Err(PyError::named(
            "ValueError",
            "negative buffersize in recvfrom".to_string(),
        ));
    }
    let flags = extract_i64(&args[2].value, "flags")?;
    let peek = (flags & plat::MSG_PEEK) != 0;
    let size = (bufsize as usize).min(16 * 1024 * 1024);

    with_socket_mut(&args[0].value, |sock| {
        let timeout = sock.timeout;
        match &mut sock.backend {
            SocketBackend::Closed => Err(closed_socket_error()),
            SocketBackend::BoundUdp { sock: udp, .. } => {
                let mut buf = vec![0u8; size];
                let (n, peer) = loop {
                    let res = if peek {
                        udp.peek_from(&mut buf)
                    } else {
                        udp.recv_from(&mut buf)
                    };
                    match res {
                        Ok(v) => break v,
                        Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                    }
                };
                buf.truncate(n);
                Ok(Value::tuple(vec![Value::bytes(buf), sockaddr_to_py(peer)]))
            }
            SocketBackend::ConnectedTcp { stream } => {
                let mut buf = vec![0u8; size];
                let n = loop {
                    let res = if peek {
                        stream.peek(&mut buf)
                    } else {
                        stream.read(&mut buf)
                    };
                    match res {
                        Ok(n) => break n,
                        Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                        Err(e) => return Err(io_error_to_socket_py(&e, timeout)),
                    }
                };
                buf.truncate(n);
                let peer_val = stream
                    .peer_addr()
                    .map_or_else(|_| Value::none(), sockaddr_to_py);
                Ok(Value::tuple(vec![Value::bytes(buf), peer_val]))
            }
            _ => Err(os_error(
                plat::ENOTCONN,
                "Transport endpoint is not connected",
            )),
        }
    })
}

fn sock_shutdown_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            "_sock_shutdown() expects 2 arguments".to_string(),
        ));
    }
    let how = extract_i64(&args[1].value, "how")?;
    let mode = match how {
        x if x == plat::SHUT_RD => Shutdown::Read,
        x if x == plat::SHUT_WR => Shutdown::Write,
        x if x == plat::SHUT_RDWR => Shutdown::Both,
        _ => return Err(os_error(plat::EINVAL, "Invalid argument")),
    };
    with_socket(&args[0].value, |sock| match &sock.backend {
        SocketBackend::Closed => Err(closed_socket_error()),
        SocketBackend::ConnectedTcp { stream } => {
            stream
                .shutdown(mode)
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            Ok(Value::none())
        }
        _ => Err(os_error(
            plat::ENOTCONN,
            "Transport endpoint is not connected",
        )),
    })
}

fn sock_close_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "_sock_close() expects 1 argument".to_string(),
        ));
    }
    with_socket_mut(&args[0].value, |sock| {
        sock.backend = SocketBackend::Closed;
        Ok(Value::none())
    })
}

fn sock_fileno_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "_sock_fileno() expects 1 argument".to_string(),
        ));
    }
    with_socket(&args[0].value, |sock| Ok(Value::int(sock.fileno())))
}

fn sock_setsockopt_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 4 {
        return Err(PyError::named(
            "TypeError",
            "_sock_setsockopt() expects 4 arguments".to_string(),
        ));
    }
    let level = extract_i64(&args[1].value, "level")?;
    let optname = extract_i64(&args[2].value, "optname")?;
    let optval = extract_i64(&args[3].value, "value")? as i32;

    with_socket_mut(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        if level == plat::SOL_SOCKET {
            match optname {
                x if x == plat::SO_REUSEADDR => sock.opts.reuse_addr = optval,
                x if x == plat::SO_KEEPALIVE => sock.opts.keepalive = optval,
                x if x == plat::SO_BROADCAST => sock.opts.broadcast = optval,
                x if x == plat::SO_RCVBUF => sock.opts.rcvbuf = Some(optval),
                x if x == plat::SO_SNDBUF => sock.opts.sndbuf = Some(optval),
                _ => {}
            }
            if let Some(raw) = sock.raw_handle() {
                raw_setsockopt_int(raw, level as i32, optname as i32, optval)
                    .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            }
            return Ok(Value::none());
        }
        if level == plat::IPPROTO_TCP && optname == plat::TCP_NODELAY {
            sock.opts.tcp_nodelay = optval;
            if let SocketBackend::ConnectedTcp { stream } = &sock.backend {
                stream
                    .set_nodelay(optval != 0)
                    .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            }
            return Ok(Value::none());
        }
        if level == plat::IPPROTO_IPV6 && optname == plat::IPV6_V6ONLY {
            sock.opts.v6only = optval;
            if let Some(raw) = sock.raw_handle() {
                let _ = raw_setsockopt_int(raw, level as i32, optname as i32, optval);
            }
            return Ok(Value::none());
        }
        if let Some(raw) = sock.raw_handle() {
            raw_setsockopt_int(raw, level as i32, optname as i32, optval)
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
        }
        Ok(Value::none())
    })
}

fn sock_getsockopt_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            "_sock_getsockopt() expects 3 arguments".to_string(),
        ));
    }
    let level = extract_i64(&args[1].value, "level")?;
    let optname = extract_i64(&args[2].value, "optname")?;

    with_socket(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        if level == plat::IPPROTO_TCP && optname == plat::TCP_NODELAY {
            if let SocketBackend::ConnectedTcp { stream } = &sock.backend {
                let nodelay = stream
                    .nodelay()
                    .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
                return Ok(Value::int(i64::from(nodelay)));
            }
            return Ok(Value::int(i64::from(sock.opts.tcp_nodelay != 0)));
        }
        if let Some(raw) = sock.raw_handle() {
            let v = raw_getsockopt_int(raw, level as i32, optname as i32)
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            return Ok(Value::int(i64::from(v)));
        }
        if level == plat::SOL_SOCKET {
            let val = match optname {
                x if x == plat::SO_REUSEADDR => sock.opts.reuse_addr,
                x if x == plat::SO_KEEPALIVE => sock.opts.keepalive,
                x if x == plat::SO_BROADCAST => sock.opts.broadcast,
                x if x == plat::SO_RCVBUF => sock.opts.rcvbuf.unwrap_or(65536),
                x if x == plat::SO_SNDBUF => sock.opts.sndbuf.unwrap_or(65536),
                x if x == plat::SO_ERROR => 0,
                _ => 0,
            };
            return Ok(Value::int(i64::from(val)));
        }
        if level == plat::IPPROTO_IPV6 && optname == plat::IPV6_V6ONLY {
            return Ok(Value::int(i64::from(sock.opts.v6only)));
        }
        Ok(Value::int(0))
    })
}

fn sock_settimeout_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            "_sock_settimeout() expects 2 arguments".to_string(),
        ));
    }
    let timeout = parse_timeout_value(&args[1].value)?;
    with_socket_mut(&args[0].value, |sock| {
        if matches!(sock.backend, SocketBackend::Closed) {
            return Err(closed_socket_error());
        }
        sock.timeout = timeout;
        sock.apply_timeout_to_backend()?;
        Ok(Value::none())
    })
}

fn sock_gettimeout_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "_sock_gettimeout() expects 1 argument".to_string(),
        ));
    }
    with_socket(&args[0].value, |sock| Ok(timeout_to_py(sock.timeout)))
}

fn sock_getsockname_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "_sock_getsockname() expects 1 argument".to_string(),
        ));
    }
    with_socket(&args[0].value, |sock| match &sock.backend {
        SocketBackend::Closed => Err(closed_socket_error()),
        SocketBackend::BoundTcp { listener, .. } => {
            let addr = listener
                .local_addr()
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            Ok(sockaddr_to_py(addr))
        }
        SocketBackend::ConnectedTcp { stream } => {
            let addr = stream
                .local_addr()
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            Ok(sockaddr_to_py(addr))
        }
        SocketBackend::BoundUdp { sock: udp, .. } => {
            let addr = udp
                .local_addr()
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            Ok(sockaddr_to_py(addr))
        }
        SocketBackend::UnboundTcp | SocketBackend::UnboundUdp => {
            let addr = if sock.family == plat::AF_INET6 {
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0))
            } else {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
            };
            Ok(sockaddr_to_py(addr))
        }
    })
}

fn sock_getpeername_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "_sock_getpeername() expects 1 argument".to_string(),
        ));
    }
    with_socket(&args[0].value, |sock| match &sock.backend {
        SocketBackend::Closed => Err(closed_socket_error()),
        SocketBackend::ConnectedTcp { stream } => {
            let addr = stream
                .peer_addr()
                .map_err(|e| io_error_to_socket_py(&e, sock.timeout))?;
            Ok(sockaddr_to_py(addr))
        }
        SocketBackend::BoundUdp {
            connected_peer: Some(peer),
            ..
        } => Ok(sockaddr_to_py(*peer)),
        _ => Err(os_error(
            plat::ENOTCONN,
            "Transport endpoint is not connected",
        )),
    })
}

// ── Hostname, DNS, byte-order, and address conversion helpers ────────────────

fn native_gethostname_impl() -> Result<Value> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len()) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(PyError::from_io_error(&err, None));
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Ok(Value::string(String::from_utf8_lossy(&buf[..end]).as_ref()))
    }
    #[cfg(windows)]
    {
        // Ensure Winsock is initialized if needed by reading COMPUTERNAME fallback.
        let mut buf = [0u8; 256];
        let rc = unsafe { win_ffi::ws_gethostname(buf.as_mut_ptr(), buf.len() as i32) };
        if rc == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            return Ok(Value::string(String::from_utf8_lossy(&buf[..end]).as_ref()));
        }
        if let Ok(name) = std::env::var("COMPUTERNAME") {
            return Ok(Value::string(name));
        }
        Ok(Value::string("localhost"))
    }
}

fn native_gethostbyname_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            "_native_gethostbyname() expects 2 arguments".to_string(),
        ));
    }
    let host = match args[0].value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "gethostbyname() argument 1 must be str, not {}",
                    value_type_name_str(&args[0].value)
                ),
            ));
        }
    };
    let gaierror_cls = &args[1].value;
    if host.is_empty() {
        return Ok(Value::string("0.0.0.0"));
    }
    if host == "<broadcast>" {
        return Ok(Value::string("255.255.255.255"));
    }
    if let Some(octets) = parse_inet_aton(&host) {
        return Ok(Value::string(Ipv4Addr::from(octets).to_string()));
    }
    let addrs = (host.as_str(), 0u16)
        .to_socket_addrs()
        .map_err(|_| raise_gaierror(gaierror_cls, plat::EAI_NONAME, "Name or service not known"))?;
    for a in addrs {
        if let SocketAddr::V4(v4) = a {
            return Ok(Value::string(v4.ip().to_string()));
        }
    }
    Err(raise_gaierror(
        gaierror_cls,
        plat::EAI_NONAME,
        "Name or service not known",
    ))
}

fn native_getaddrinfo_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 7 {
        return Err(PyError::named(
            "TypeError",
            "_native_getaddrinfo() expects 7 arguments".to_string(),
        ));
    }
    let host_opt = match args[0].value.kind() {
        ValueKind::None => None,
        ValueKind::Str(s) => Some(s.to_string()),
        ValueKind::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "getaddrinfo() argument 1 must be string or None".to_string(),
            ));
        }
    };
    let gaierror_cls = &args[6].value;
    let port: u16 = match args[1].value.kind() {
        ValueKind::None => 0,
        ValueKind::Int(p) => {
            if !(0..=65535).contains(&p) {
                return Err(raise_gaierror(
                    gaierror_cls,
                    plat::EAI_NONAME,
                    "Servname not supported for ai_socktype",
                ));
            }
            p as u16
        }
        ValueKind::Str(s) => {
            if s.is_empty() {
                0
            } else if let Ok(p) = s.parse::<u16>() {
                p
            } else {
                match s {
                    "http" => 80,
                    "https" => 443,
                    "ftp" => 21,
                    "ssh" => 22,
                    "smtp" => 25,
                    "dns" | "domain" => 53,
                    _ => {
                        return Err(raise_gaierror(
                            gaierror_cls,
                            plat::EAI_NONAME,
                            "Servname not supported for ai_socktype",
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                "Int or String expected".to_string(),
            ));
        }
    };

    let family = extract_i64(&args[2].value, "family")?;
    let sock_type = extract_i64(&args[3].value, "type")?;
    let proto = extract_i64(&args[4].value, "proto")?;
    let flags = extract_i64(&args[5].value, "flags")?;

    if family != plat::AF_UNSPEC && family != plat::AF_INET && family != plat::AF_INET6 {
        return Err(raise_gaierror(
            gaierror_cls,
            plat::EAFNOSUPPORT,
            "ai_family not supported",
        ));
    }

    let passive = (flags & plat::AI_PASSIVE) != 0;
    let numeric_host = (flags & plat::AI_NUMERICHOST) != 0;
    let want_canon = (flags & plat::AI_CANONNAME) != 0;

    let mut resolved_addrs: Vec<SocketAddr> = Vec::new();
    let canon_name: String = match host_opt.as_deref() {
        None | Some("") => {
            if family == plat::AF_UNSPEC || family == plat::AF_INET {
                let ip = if passive {
                    Ipv4Addr::UNSPECIFIED
                } else {
                    Ipv4Addr::LOCALHOST
                };
                resolved_addrs.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
            }
            if family == plat::AF_UNSPEC || family == plat::AF_INET6 {
                let ip = if passive {
                    Ipv6Addr::UNSPECIFIED
                } else {
                    Ipv6Addr::LOCALHOST
                };
                resolved_addrs.push(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, 0)));
            }
            String::new()
        }
        Some(host) => {
            if let Ok(ip) = host.parse::<IpAddr>() {
                match (ip, family) {
                    (IpAddr::V4(v4), f) if f == plat::AF_UNSPEC || f == plat::AF_INET => {
                        resolved_addrs.push(SocketAddr::V4(SocketAddrV4::new(v4, port)));
                    }
                    (IpAddr::V6(v6), f) if f == plat::AF_UNSPEC || f == plat::AF_INET6 => {
                        resolved_addrs.push(SocketAddr::V6(SocketAddrV6::new(v6, port, 0, 0)));
                    }
                    _ => {
                        return Err(raise_gaierror(
                            gaierror_cls,
                            plat::EAI_NONAME,
                            "Address family for hostname not supported",
                        ));
                    }
                }
                if want_canon {
                    host.to_string()
                } else {
                    String::new()
                }
            } else {
                if numeric_host {
                    return Err(raise_gaierror(
                        gaierror_cls,
                        plat::EAI_NONAME,
                        "Name or service not known",
                    ));
                }
                let iter = (host, port).to_socket_addrs().map_err(|_| {
                    raise_gaierror(gaierror_cls, plat::EAI_NONAME, "Name or service not known")
                })?;
                for addr in iter {
                    let ok = match addr {
                        SocketAddr::V4(_) => family == plat::AF_UNSPEC || family == plat::AF_INET,
                        SocketAddr::V6(_) => family == plat::AF_UNSPEC || family == plat::AF_INET6,
                    };
                    if ok && !resolved_addrs.contains(&addr) {
                        resolved_addrs.push(addr);
                    }
                }
                if resolved_addrs.is_empty() {
                    return Err(raise_gaierror(
                        gaierror_cls,
                        plat::EAI_NONAME,
                        "Name or service not known",
                    ));
                }
                if want_canon {
                    host.to_string()
                } else {
                    String::new()
                }
            }
        }
    };

    let kinds: Vec<(i64, i64)> = match (sock_type, proto) {
        (0, 0) => vec![
            (plat::SOCK_STREAM, plat::IPPROTO_TCP),
            (plat::SOCK_DGRAM, plat::IPPROTO_UDP),
        ],
        (t, 0) if t == plat::SOCK_STREAM => vec![(plat::SOCK_STREAM, 0)],
        (t, 0) if t == plat::SOCK_DGRAM => vec![(plat::SOCK_DGRAM, 0)],
        (0, p) if p == plat::IPPROTO_TCP => vec![(plat::SOCK_STREAM, plat::IPPROTO_TCP)],
        (0, p) if p == plat::IPPROTO_UDP => vec![(plat::SOCK_DGRAM, plat::IPPROTO_UDP)],
        (t, p) => vec![(t, p)],
    };

    let mut out = Vec::new();
    for (idx, addr) in resolved_addrs.into_iter().enumerate() {
        let fam = match addr {
            SocketAddr::V4(_) => plat::AF_INET,
            SocketAddr::V6(_) => plat::AF_INET6,
        };
        let sa_val = sockaddr_to_py(addr);
        for (k_idx, &(kind, p)) in kinds.iter().enumerate() {
            let cname = if idx == 0 && k_idx == 0 {
                canon_name.as_str()
            } else {
                ""
            };
            out.push(Value::tuple(vec![
                Value::int(fam),
                Value::int(kind),
                Value::int(p),
                Value::string(cname),
                sa_val.clone(),
            ]));
        }
    }
    Ok(Value::list(out))
}

fn native_inet_aton_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "inet_aton() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let s = match args[0].value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "inet_aton() argument 1 must be str, not {}",
                    value_type_name_str(&args[0].value)
                ),
            ));
        }
    };
    let octets = parse_inet_aton(&s).ok_or_else(|| {
        PyError::named(
            "OSError",
            "illegal IP address string passed to inet_aton".to_string(),
        )
    })?;
    Ok(Value::bytes(octets.to_vec()))
}

fn native_inet_ntoa_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "inet_ntoa() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let raw = extract_bytes_like(&args[0].value).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(&args[0].value)
            ),
        )
    })?;
    if raw.len() != 4 {
        return Err(PyError::named(
            "OSError",
            "packed IP wrong length for inet_ntoa".to_string(),
        ));
    }
    let ip = Ipv4Addr::new(raw[0], raw[1], raw[2], raw[3]);
    Ok(Value::string(ip.to_string()))
}

fn native_inet_pton_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "inet_pton() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let family = extract_i64(&args[0].value, "address_family")?;
    let s = match args[1].value.kind() {
        ValueKind::Str(s) => s.to_string(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "inet_pton() argument 2 must be str, not {}",
                    value_type_name_str(&args[1].value)
                ),
            ));
        }
    };
    if family == plat::AF_INET {
        let ip = s.parse::<Ipv4Addr>().map_err(|_| {
            PyError::named(
                "OSError",
                "illegal IP address string passed to inet_pton".to_string(),
            )
        })?;
        return Ok(Value::bytes(ip.octets().to_vec()));
    }
    if family == plat::AF_INET6 {
        let ip = s.parse::<Ipv6Addr>().map_err(|_| {
            PyError::named(
                "OSError",
                "illegal IP address string passed to inet_pton".to_string(),
            )
        })?;
        return Ok(Value::bytes(ip.octets().to_vec()));
    }
    Err(PyError::named(
        "OSError",
        format!("unknown address family {family}"),
    ))
}

fn native_inet_ntop_impl(args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "inet_ntop() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let family = extract_i64(&args[0].value, "address_family")?;
    let raw = extract_bytes_like(&args[1].value).ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "a bytes-like object is required, not '{}'",
                value_type_name_str(&args[1].value)
            ),
        )
    })?;
    if family == plat::AF_INET {
        if raw.len() != 4 {
            return Err(PyError::named(
                "ValueError",
                "invalid length of packed IP address string".to_string(),
            ));
        }
        let ip = Ipv4Addr::new(raw[0], raw[1], raw[2], raw[3]);
        return Ok(Value::string(ip.to_string()));
    }
    if family == plat::AF_INET6 {
        if raw.len() != 16 {
            return Err(PyError::named(
                "ValueError",
                "invalid length of packed IP address string".to_string(),
            ));
        }
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&raw);
        let ip = Ipv6Addr::from(octets);
        return Ok(Value::string(ip.to_string()));
    }
    Err(PyError::named(
        "ValueError",
        format!("unknown address family {family}"),
    ))
}

fn extract_u16_checked(fn_name: &str, args: &[ExpandedCallArg]) -> Result<u16> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let val = extract_i64(&args[0].value, fn_name)?;
    if val < 0 {
        return Err(PyError::named(
            "OverflowError",
            format!("{fn_name}: can't convert negative Python int to C 16-bit unsigned integer"),
        ));
    }
    if val > u16::MAX as i64 {
        return Err(PyError::named(
            "OverflowError",
            format!("{fn_name}: Python int too large to convert to C 16-bit unsigned integer"),
        ));
    }
    Ok(val as u16)
}

fn extract_u32_checked(fn_name: &str, args: &[ExpandedCallArg]) -> Result<u32> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let val = extract_i64(&args[0].value, fn_name)?;
    if val < 0 {
        return Err(PyError::named(
            "OverflowError",
            "can't convert negative value to unsigned int".to_string(),
        ));
    }
    if val > u32::MAX as i64 {
        return Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C unsigned long".to_string(),
        ));
    }
    Ok(val as u32)
}

pyrust_module! {
    fn _constants_map(args) -> Result<Value> {
        let _ = (_interp, args);
        constants_map_impl()
    }

    fn _sock_new(args) -> Result<Value> {
        let _ = _interp;
        sock_new_impl(args)
    }

    fn _sock_bind(args) -> Result<Value> {
        let _ = _interp;
        sock_bind_impl(args)
    }

    fn _sock_listen(args) -> Result<Value> {
        let _ = _interp;
        sock_listen_impl(args)
    }

    fn _sock_accept(args) -> Result<Value> {
        sock_accept_impl(_interp, args)
    }

    fn _sock_connect(args) -> Result<Value> {
        let _ = _interp;
        sock_connect_impl(args)
    }

    fn _sock_connect_ex(args) -> Result<Value> {
        let _ = _interp;
        sock_connect_ex_impl(args)
    }

    fn _sock_send(args) -> Result<Value> {
        let _ = _interp;
        sock_send_impl(args)
    }

    fn _sock_sendall(args) -> Result<Value> {
        let _ = _interp;
        sock_sendall_impl(args)
    }

    fn _sock_recv(args) -> Result<Value> {
        sock_recv_impl(_interp, args)
    }

    fn _sock_recv_into(args) -> Result<Value> {
        let _ = _interp;
        sock_recv_into_impl(args)
    }

    fn _sock_sendto(args) -> Result<Value> {
        let _ = _interp;
        sock_sendto_impl(args)
    }

    fn _sock_recvfrom(args) -> Result<Value> {
        let _ = _interp;
        sock_recvfrom_impl(args)
    }

    fn _sock_shutdown(args) -> Result<Value> {
        let _ = _interp;
        sock_shutdown_impl(args)
    }

    fn _sock_close(args) -> Result<Value> {
        let _ = _interp;
        sock_close_impl(args)
    }

    fn _sock_fileno(args) -> Result<Value> {
        let _ = _interp;
        sock_fileno_impl(args)
    }

    fn _sock_setsockopt(args) -> Result<Value> {
        let _ = _interp;
        sock_setsockopt_impl(args)
    }

    fn _sock_getsockopt(args) -> Result<Value> {
        let _ = _interp;
        sock_getsockopt_impl(args)
    }

    fn _sock_settimeout(args) -> Result<Value> {
        let _ = _interp;
        sock_settimeout_impl(args)
    }

    fn _sock_gettimeout(args) -> Result<Value> {
        let _ = _interp;
        sock_gettimeout_impl(args)
    }

    fn _sock_getsockname(args) -> Result<Value> {
        let _ = _interp;
        sock_getsockname_impl(args)
    }

    fn _sock_getpeername(args) -> Result<Value> {
        let _ = _interp;
        sock_getpeername_impl(args)
    }

    fn _native_gethostname(args) -> Result<Value> {
        let _ = (_interp, args);
        native_gethostname_impl()
    }

    fn _native_gethostbyname(args) -> Result<Value> {
        let _ = _interp;
        native_gethostbyname_impl(args)
    }

    fn _native_getaddrinfo(args) -> Result<Value> {
        let _ = _interp;
        native_getaddrinfo_impl(args)
    }

    fn _native_inet_aton(args) -> Result<Value> {
        let _ = _interp;
        native_inet_aton_impl(args)
    }

    fn _native_inet_ntoa(args) -> Result<Value> {
        let _ = _interp;
        native_inet_ntoa_impl(args)
    }

    fn _native_inet_pton(args) -> Result<Value> {
        let _ = _interp;
        native_inet_pton_impl(args)
    }

    fn _native_inet_ntop(args) -> Result<Value> {
        let _ = _interp;
        native_inet_ntop_impl(args)
    }

    fn _native_htons(args) -> Result<Value> {
        let _ = _interp;
        let v = extract_u16_checked("htons", args)?;
        Ok(Value::int(i64::from(v.to_be())))
    }

    fn _native_ntohs(args) -> Result<Value> {
        let _ = _interp;
        let v = extract_u16_checked("ntohs", args)?;
        Ok(Value::int(i64::from(u16::from_be(v))))
    }

    fn _native_htonl(args) -> Result<Value> {
        let _ = _interp;
        let v = extract_u32_checked("htonl", args)?;
        Ok(Value::int(i64::from(v.to_be())))
    }

    fn _native_ntohl(args) -> Result<Value> {
        let _ = _interp;
        let v = extract_u32_checked("ntohl", args)?;
        Ok(Value::int(i64::from(u32::from_be(v))))
    }
}

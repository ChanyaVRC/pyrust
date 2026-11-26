// `select` module — 100% native Rust implementation (issue #3253).
//
// Provides:
// - `error` (alias for `OSError`)
// - `select(rlist, wlist, xlist[, timeout])` backed by `libc::select` on Unix
//   and Winsock2 `select` on Windows (reusing `socket::select_raw_handles_3`)
// - `poll()` and `POLL*` constants on Unix (`#[cfg(unix)]`)
// - `epoll()` and `EPOLL*` constants on Linux (`#[cfg(target_os = "linux")]`)
// - `kqueue()` / `kevent` and `KQ_*` constants on macOS (`#[cfg(target_os = "macos")]`)
//
// Reference: <https://docs.python.org/3/library/select.html>

use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
#[cfg(unix)]
use crate::interpreter::builtin_args::{PyIndex, PyValueRef};
use crate::interpreter::builtin_args::{PyNumber, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

/// Extract a non-negative integer file descriptor from an `int` or an object
/// implementing `.fileno()`.
fn extract_fileno(interp: &mut Interpreter, obj: &Value) -> Result<i64> {
    let fd = match obj.kind() {
        ValueKind::Int(n) => n,
        ValueKind::Bool(b) => i64::from(b),
        ValueKind::BigInt(_) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            ));
        }
        _ => {
            let fileno_method = interp.get_attr(obj, "fileno").map_err(|_| {
                PyError::named(
                    "TypeError",
                    format!(
                        "argument must be an int, or have a fileno() method, not '{}'",
                        value_type_name_str(obj)
                    ),
                )
            })?;
            let fd_val = interp.call_function_expanded(fileno_method, &[])?;
            match fd_val.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => i64::from(b),
                ValueKind::BigInt(_) => {
                    return Err(PyError::named(
                        "OverflowError",
                        "Python int too large to convert to C int".to_string(),
                    ));
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "fileno() returned a non-integer (type {})",
                            value_type_name_str(&fd_val)
                        ),
                    ));
                }
            }
        }
    };
    if fd < 0 {
        return Err(PyError::named(
            "ValueError",
            format!("file descriptor cannot be a negative integer ({fd})"),
        ));
    }
    Ok(fd)
}

#[cfg(unix)]
fn extract_c_int_fd(interp: &mut Interpreter, obj: &Value) -> Result<libc::c_int> {
    let fd = extract_fileno(interp, obj)?;
    libc::c_int::try_from(fd).map_err(|_| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        )
    })
}

fn parse_select_timeout(timeout: Option<PyNumber>) -> Result<Option<Duration>> {
    let Some(num) = timeout else {
        return Ok(None);
    };
    let secs = num.as_f64();
    if secs.is_nan() {
        return Err(PyError::named(
            "ValueError",
            "timeout must not be NaN".to_string(),
        ));
    }
    if secs < 0.0 {
        return Err(PyError::named(
            "ValueError",
            "timeout must be non-negative".to_string(),
        ));
    }
    if secs.is_infinite() {
        return Ok(None);
    }
    Ok(Some(Duration::from_secs_f64(secs)))
}

// ── Unix `poll` state ────────────────────────────────────────────────────────

#[cfg(unix)]
struct PollState {
    fds: IndexMap<libc::c_int, libc::c_short>,
}

// ── Linux `epoll` state ──────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
struct EpollState {
    epfd: Option<libc::c_int>,
}

#[cfg(target_os = "linux")]
impl EpollState {
    fn open_fd(&self) -> Result<libc::c_int> {
        self.epfd
            .ok_or_else(|| PyError::named("ValueError", "I/O operation on closed epoll object"))
    }

    fn close_internal(&mut self) -> Result<()> {
        if let Some(fd) = self.epfd.take() {
            let rc = unsafe { libc::close(fd) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for EpollState {
    fn drop(&mut self) {
        if let Some(fd) = self.epfd.take() {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

// ── macOS `kqueue` / `kevent` state ──────────────────────────────────────────

#[cfg(target_os = "macos")]
struct KqueueState {
    kqfd: Option<libc::c_int>,
}

#[cfg(target_os = "macos")]
impl KqueueState {
    fn open_fd(&self) -> Result<libc::c_int> {
        self.kqfd
            .ok_or_else(|| PyError::named("ValueError", "I/O operation on closed kqueue object"))
    }

    fn close_internal(&mut self) -> Result<()> {
        if let Some(fd) = self.kqfd.take() {
            let rc = unsafe { libc::close(fd) };
            if rc < 0 {
                return Err(PyError::from_io_error(
                    &std::io::Error::last_os_error(),
                    None,
                ));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for KqueueState {
    fn drop(&mut self) {
        if let Some(fd) = self.kqfd.take() {
            unsafe {
                libc::close(fd);
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, PartialEq, Eq)]
struct KeventState {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: isize,
    udata: usize,
}

#[cfg(target_os = "macos")]
impl KeventState {
    fn to_libc(self) -> libc::kevent {
        libc::kevent {
            ident: self.ident,
            filter: self.filter,
            flags: self.flags,
            fflags: self.fflags,
            data: self.data,
            udata: self.udata as *mut libc::c_void,
        }
    }

    fn from_libc(ev: libc::kevent) -> Self {
        Self {
            ident: ev.ident,
            filter: ev.filter,
            flags: ev.flags,
            fflags: ev.fflags,
            data: ev.data,
            udata: ev.udata as usize,
        }
    }
}

pyrust_module! {
    constants {
        "error" => Value::py_class(
            crate::interpreter::lookup_exc_class("OSError").expect("OSError class")
        ),

        #[cfg(unix)]
        "POLLIN" => Value::int(libc::POLLIN as i64),
        #[cfg(unix)]
        "POLLPRI" => Value::int(libc::POLLPRI as i64),
        #[cfg(unix)]
        "POLLOUT" => Value::int(libc::POLLOUT as i64),
        #[cfg(unix)]
        "POLLERR" => Value::int(libc::POLLERR as i64),
        #[cfg(unix)]
        "POLLHUP" => Value::int(libc::POLLHUP as i64),
        #[cfg(unix)]
        "POLLNVAL" => Value::int(libc::POLLNVAL as i64),
        #[cfg(unix)]
        "POLLRDNORM" => Value::int(libc::POLLRDNORM as i64),
        #[cfg(unix)]
        "POLLRDBAND" => Value::int(libc::POLLRDBAND as i64),
        #[cfg(unix)]
        "POLLWRNORM" => Value::int(libc::POLLWRNORM as i64),
        #[cfg(unix)]
        "POLLWRBAND" => Value::int(libc::POLLWRBAND as i64),

        #[cfg(target_os = "linux")]
        "EPOLLIN" => Value::int(libc::EPOLLIN as i64),
        #[cfg(target_os = "linux")]
        "EPOLLOUT" => Value::int(libc::EPOLLOUT as i64),
        #[cfg(target_os = "linux")]
        "EPOLLPRI" => Value::int(libc::EPOLLPRI as i64),
        #[cfg(target_os = "linux")]
        "EPOLLERR" => Value::int(libc::EPOLLERR as i64),
        #[cfg(target_os = "linux")]
        "EPOLLHUP" => Value::int(libc::EPOLLHUP as i64),
        #[cfg(target_os = "linux")]
        "EPOLLET" => Value::int((libc::EPOLLET as u32) as i64),
        #[cfg(target_os = "linux")]
        "EPOLLONESHOT" => Value::int((libc::EPOLLONESHOT as u32) as i64),
        #[cfg(target_os = "linux")]
        "EPOLLRDNORM" => Value::int(libc::EPOLLRDNORM as i64),
        #[cfg(target_os = "linux")]
        "EPOLLRDBAND" => Value::int(libc::EPOLLRDBAND as i64),
        #[cfg(target_os = "linux")]
        "EPOLLWRNORM" => Value::int(libc::EPOLLWRNORM as i64),
        #[cfg(target_os = "linux")]
        "EPOLLWRBAND" => Value::int(libc::EPOLLWRBAND as i64),
        #[cfg(target_os = "linux")]
        "EPOLLMSG" => Value::int(libc::EPOLLMSG as i64),
        #[cfg(target_os = "linux")]
        "EPOLLRDHUP" => Value::int(libc::EPOLLRDHUP as i64),
        #[cfg(target_os = "linux")]
        "EPOLLEXCLUSIVE" => Value::int((libc::EPOLLEXCLUSIVE as u32) as i64),
        #[cfg(target_os = "linux")]
        "EPOLL_CLOEXEC" => Value::int(libc::EPOLL_CLOEXEC as i64),

        #[cfg(target_os = "macos")]
        "KQ_FILTER_READ" => Value::int(libc::EVFILT_READ as i64),
        #[cfg(target_os = "macos")]
        "KQ_FILTER_WRITE" => Value::int(libc::EVFILT_WRITE as i64),
        #[cfg(target_os = "macos")]
        "KQ_FILTER_AIO" => Value::int(libc::EVFILT_AIO as i64),
        #[cfg(target_os = "macos")]
        "KQ_FILTER_VNODE" => Value::int(libc::EVFILT_VNODE as i64),
        #[cfg(target_os = "macos")]
        "KQ_FILTER_PROC" => Value::int(libc::EVFILT_PROC as i64),
        #[cfg(target_os = "macos")]
        "KQ_FILTER_SIGNAL" => Value::int(libc::EVFILT_SIGNAL as i64),
        #[cfg(target_os = "macos")]
        "KQ_FILTER_TIMER" => Value::int(libc::EVFILT_TIMER as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_ADD" => Value::int(libc::EV_ADD as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_DELETE" => Value::int(libc::EV_DELETE as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_ENABLE" => Value::int(libc::EV_ENABLE as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_DISABLE" => Value::int(libc::EV_DISABLE as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_ONESHOT" => Value::int(libc::EV_ONESHOT as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_CLEAR" => Value::int(libc::EV_CLEAR as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_EOF" => Value::int(libc::EV_EOF as i64),
        #[cfg(target_os = "macos")]
        "KQ_EV_ERROR" => Value::int(libc::EV_ERROR as i64),
    }

    /// Wait until one or more file descriptors are ready for I/O.
    fn select(
        #[positional_only] rlist: PyValue,
        #[positional_only] wlist: PyValue,
        #[positional_only] xlist: PyValue,
        #[positional_only]
        #[default(None)]
        timeout: Option<PyNumber>,
    ) -> Result<Value> {
        let timeout_dur = parse_select_timeout(timeout)?;

        let r_items = _interp.collect_iterable(&rlist.0)?;
        let w_items = _interp.collect_iterable(&wlist.0)?;
        let x_items = _interp.collect_iterable(&xlist.0)?;

        let mut r_pairs: Vec<(i64, Value)> = Vec::with_capacity(r_items.len());
        for item in r_items {
            let fd = extract_fileno(_interp, &item)?;
            r_pairs.push((fd, item));
        }
        let mut w_pairs: Vec<(i64, Value)> = Vec::with_capacity(w_items.len());
        for item in w_items {
            let fd = extract_fileno(_interp, &item)?;
            w_pairs.push((fd, item));
        }
        let mut x_pairs: Vec<(i64, Value)> = Vec::with_capacity(x_items.len());
        for item in x_items {
            let fd = extract_fileno(_interp, &item)?;
            x_pairs.push((fd, item));
        }

        let r_fds: Vec<i64> = r_pairs.iter().map(|(fd, _)| *fd).collect();
        let w_fds: Vec<i64> = w_pairs.iter().map(|(fd, _)| *fd).collect();
        let x_fds: Vec<i64> = x_pairs.iter().map(|(fd, _)| *fd).collect();

        // Wait with the GIL released, as CPython does around every blocking
        // `select` / `poll` / `epoll_wait` / `kevent` call in this module.
        let (ready_r_fds, ready_w_fds, ready_x_fds) = _interp.release_gil_during(|| {
            super::socket::select_raw_handles_3(&r_fds, &w_fds, &x_fds, timeout_dur)
        })?;

        let ready_r: Vec<Value> = r_pairs
            .into_iter()
            .filter(|(fd, _)| ready_r_fds.contains(fd))
            .map(|(_, obj)| obj)
            .collect();
        let ready_w: Vec<Value> = w_pairs
            .into_iter()
            .filter(|(fd, _)| ready_w_fds.contains(fd))
            .map(|(_, obj)| obj)
            .collect();
        let ready_x: Vec<Value> = x_pairs
            .into_iter()
            .filter(|(fd, _)| ready_x_fds.contains(fd))
            .map(|(_, obj)| obj)
            .collect();

        Ok(Value::tuple(vec![
            Value::list(ready_r),
            Value::list(ready_w),
            Value::list(ready_x),
        ]))
    }

    #[cfg(unix)]
    #[native_state(PollState)]
    class poll {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            set_state(
                self_val.0,
                PollState {
                    fds: IndexMap::new(),
                },
            )?;
            Ok(Value::none())
        }

        fn register(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] fd: PyValue,
            #[positional_only]
            #[default(None)]
            eventmask: Option<PyIndex>,
        ) -> Result<Value> {
            let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
            let mask: libc::c_short = match eventmask {
                Some(idx) => {
                    let v = idx.expect_i64(FN_NAME, "eventmask")?;
                    if !(0..=u16::MAX as i64).contains(&v) {
                        return Err(PyError::named(
                            "OverflowError",
                            "unsigned short integer is out of range",
                        ));
                    }
                    v as u16 as libc::c_short
                }
                None => libc::POLLIN | libc::POLLPRI | libc::POLLOUT,
            };
            with_state_mut(self_val.0, |st| {
                st.fds.insert(raw_fd, mask);
                Ok(Value::none())
            })
        }

        fn modify(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] fd: PyValue,
            #[positional_only] eventmask: PyIndex,
        ) -> Result<Value> {
            let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
            let v = eventmask.expect_i64(FN_NAME, "eventmask")?;
            if !(0..=u16::MAX as i64).contains(&v) {
                return Err(PyError::named(
                    "OverflowError",
                    "unsigned short integer is out of range",
                ));
            }
            let mask = v as u16 as libc::c_short;
            with_state_mut(self_val.0, |st| {
                if let Some(slot) = st.fds.get_mut(&raw_fd) {
                    *slot = mask;
                    Ok(Value::none())
                } else {
                    let err = std::io::Error::from_raw_os_error(libc::ENOENT);
                    Err(PyError::from_io_error(&err, None))
                }
            })
        }

        fn unregister(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] fd: PyValue,
        ) -> Result<Value> {
            let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
            with_state_mut(self_val.0, |st| {
                if st.fds.shift_remove(&raw_fd).is_some() {
                    Ok(Value::none())
                } else {
                    Err(PyError::key_error(Value::int(raw_fd as i64)))
                }
            })
        }

        fn poll(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only]
            #[default(None)]
            timeout: Option<PyNumber>,
        ) -> Result<Value> {
            let timeout_ms: Option<f64> = match timeout {
                None => None,
                Some(num) => {
                    let ms = num.as_f64();
                    if ms.is_nan() {
                        return Err(PyError::named(
                            "ValueError",
                            "timeout must not be NaN",
                        ));
                    }
                    if ms < 0.0 || ms.is_infinite() {
                        None
                    } else {
                        Some(ms)
                    }
                }
            };

            // Snapshot the registrations so no state borrow outlives the GIL
            // release below (another thread may `register()` meanwhile).
            let mut pfds: Vec<libc::pollfd> = with_state(self_val.0, |st| {
                Ok(st
                    .fds
                    .iter()
                    .map(|(&fd, &events)| libc::pollfd {
                        fd,
                        events,
                        revents: 0,
                    })
                    .collect())
            })?;

            let deadline = timeout_ms.map(|ms| Instant::now() + Duration::from_secs_f64(ms / 1000.0));
            _interp
                .release_gil_during(|| loop {
                    for pfd in &mut pfds {
                        pfd.revents = 0;
                    }
                    let wait_ms: libc::c_int = match (timeout_ms, deadline) {
                        (None, _) => -1,
                        (Some(0.0), _) => 0,
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
                    let rc = unsafe {
                        libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, wait_ms)
                    };
                    if rc >= 0 {
                        return Ok(());
                    }
                    let err = std::io::Error::last_os_error();
                    if err.kind() != std::io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                    if let Some(dl) = deadline
                        && Instant::now() >= dl
                    {
                        for pfd in &mut pfds {
                            pfd.revents = 0;
                        }
                        return Ok(());
                    }
                })
                .map_err(|err| PyError::from_io_error(&err, None))?;
            let out = pfds
                .iter()
                .filter(|pfd| pfd.revents != 0)
                .map(|pfd| {
                    Value::tuple(vec![
                        Value::int(pfd.fd as i64),
                        Value::int((pfd.revents as u16) as i64),
                    ])
                })
                .collect();
            Ok(Value::list(out))
        }
    }

    #[cfg(target_os = "linux")]
    #[native_state(EpollState)]
    class epoll {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default((-1_i64).into())] sizehint: PyIndex,
            #[default(0_i64.into())] flags: PyIndex,
        ) -> Result<Value> {
            let sh = sizehint.expect_i64(FN_NAME, "sizehint")?;
            if sh != -1 && sh <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    "negative sizehint",
                ));
            }
            let fl = flags.expect_i64(FN_NAME, "flags")?;
            if fl != 0 && fl != libc::EPOLL_CLOEXEC as i64 {
                let err = std::io::Error::from_raw_os_error(libc::EINVAL);
                return Err(PyError::from_io_error(&err, None));
            }
            let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
            if epfd < 0 {
                return Err(PyError::from_io_error(&std::io::Error::last_os_error(), None));
            }
            set_state(self_val.0, EpollState { epfd: Some(epfd) })?;
            Ok(Value::none())
        }

        #[classmethod]
        fn fromfd(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] fd: PyIndex,
        ) -> Result<Value> {
            let raw = fd.expect_i64(FN_NAME, "fd")?;
            let cfd = libc::c_int::try_from(raw).map_err(|_| {
                PyError::named(
                    "OverflowError",
                    "Python int too large to convert to C int",
                )
            })?;
            let ValueKind::PyClass(class_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "fromfd() requires a class"));
            };
            let inst = Value::py_instance(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::PyInstance {
                    class: std::rc::Rc::clone(class_rc),
                    attrs: crate::value::InstanceAttrs::new(),
                },
            )));
            set_state(&inst, EpollState { epfd: Some(cfd) })?;
            Ok(inst)
        }

        fn close(#[state_mut] st: &mut EpollState) -> Result<Value> {
            st.close_internal()?;
            Ok(Value::none())
        }

        #[getter]
        fn closed(#[state] st: &EpollState) -> Result<Value> {
            Ok(Value::bool_(st.epfd.is_none()))
        }

        fn fileno(#[state] st: &EpollState) -> Result<Value> {
            Ok(Value::int(st.open_fd()? as i64))
        }

        fn register(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
            #[default(None)] eventmask: Option<PyIndex>,
        ) -> Result<Value> {
            let epfd = with_state(self_val.0, |st| st.open_fd())?;
            let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
            let events: u32 = match eventmask {
                Some(idx) => idx.expect_i64(FN_NAME, "eventmask")? as u32,
                None => (libc::EPOLLIN | libc::EPOLLPRI | libc::EPOLLOUT) as u32,
            };
            let mut ev = libc::epoll_event {
                events,
                u64: raw_fd as u64,
            };
            let rc = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, raw_fd, &mut ev) };
            if rc < 0 {
                return Err(PyError::from_io_error(&std::io::Error::last_os_error(), None));
            }
            Ok(Value::none())
        }

        fn modify(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
            eventmask: PyIndex,
        ) -> Result<Value> {
            let epfd = with_state(self_val.0, |st| st.open_fd())?;
            let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
            let events = eventmask.expect_i64(FN_NAME, "eventmask")? as u32;
            let mut ev = libc::epoll_event {
                events,
                u64: raw_fd as u64,
            };
            let rc = unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_MOD, raw_fd, &mut ev) };
            if rc < 0 {
                return Err(PyError::from_io_error(&std::io::Error::last_os_error(), None));
            }
            Ok(Value::none())
        }

        fn unregister(
            #[positional_only] self_val: PyValueRef<'_>,
            fd: PyValue,
        ) -> Result<Value> {
            let epfd = with_state(self_val.0, |st| st.open_fd())?;
            let raw_fd = extract_c_int_fd(_interp, &fd.0)?;
            let rc = unsafe {
                libc::epoll_ctl(epfd, libc::EPOLL_CTL_DEL, raw_fd, std::ptr::null_mut())
            };
            if rc < 0 {
                return Err(PyError::from_io_error(&std::io::Error::last_os_error(), None));
            }
            Ok(Value::none())
        }

        fn poll(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(None)] timeout: Option<PyNumber>,
            #[default((-1_i64).into())] maxevents: PyIndex,
        ) -> Result<Value> {
            let epfd = with_state(self_val.0, |st| st.open_fd())?;
            let max_ev_i64 = maxevents.expect_i64(FN_NAME, "maxevents")?;
            let max_ev: usize = if max_ev_i64 == -1 {
                64
            } else if max_ev_i64 <= 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("maxevents must be greater than 0, got {max_ev_i64}"),
                ));
            } else {
                (max_ev_i64 as usize).min(65536)
            };

            let timeout_secs: Option<f64> = match timeout {
                None => None,
                Some(num) => {
                    let s = num.as_f64();
                    if s.is_nan() {
                        return Err(PyError::named("ValueError", "timeout must not be NaN"));
                    }
                    if s < 0.0 || s.is_infinite() {
                        None
                    } else {
                        Some(s)
                    }
                }
            };

            let mut events = vec![libc::epoll_event { events: 0, u64: 0 }; max_ev];
            let deadline = timeout_secs.map(|s| Instant::now() + Duration::from_secs_f64(s));
            let ready = _interp
                .release_gil_during(|| loop {
                    let wait_ms: libc::c_int = match (timeout_secs, deadline) {
                        (None, _) => -1,
                        (Some(0.0), _) => 0,
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
                    let rc = unsafe {
                        libc::epoll_wait(epfd, events.as_mut_ptr(), max_ev as libc::c_int, wait_ms)
                    };
                    if rc >= 0 {
                        return Ok(rc as usize);
                    }
                    let err = std::io::Error::last_os_error();
                    if err.kind() != std::io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                    if let Some(dl) = deadline
                        && Instant::now() >= dl
                    {
                        return Ok(0);
                    }
                })
                .map_err(|err| PyError::from_io_error(&err, None))?;
            let out = events[..ready]
                .iter()
                .map(|ev| {
                    Value::tuple(vec![
                        Value::int(ev.u64 as i64),
                        Value::int(ev.events as i64),
                    ])
                })
                .collect();
            Ok(Value::list(out))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| st.open_fd())?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| st.close_internal())?;
            Ok(Value::none())
        }
    }

    #[cfg(target_os = "macos")]
    #[native_state(KeventState)]
    class kevent {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            ident: PyValue,
            #[default((libc::EVFILT_READ as i64).into())] filter: PyIndex,
            #[default((libc::EV_ADD as i64).into())] flags: PyIndex,
            #[default(0_i64.into())] fflags: PyIndex,
            #[default(0_i64.into())] data: PyIndex,
            #[default(0_i64.into())] udata: PyIndex,
        ) -> Result<Value> {
            let ident_val = extract_fileno(_interp, &ident.0)? as usize;
            let state = KeventState {
                ident: ident_val,
                filter: filter.expect_i64(FN_NAME, "filter")? as i16,
                flags: flags.expect_i64(FN_NAME, "flags")? as u16,
                fflags: fflags.expect_i64(FN_NAME, "fflags")? as u32,
                data: data.expect_i64(FN_NAME, "data")? as isize,
                udata: udata.expect_i64(FN_NAME, "udata")? as usize,
            };
            set_state(self_val.0, state)?;
            Ok(Value::none())
        }

        #[getter]
        fn ident(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::int(st.ident as i64))
        }

        #[setter("ident")]
        fn set_ident(#[state_mut] st: &mut KeventState, v: PyIndex) -> Result<Value> {
            st.ident = v.expect_i64(FN_NAME, "ident")? as usize;
            Ok(Value::none())
        }

        #[getter]
        fn filter(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::int(st.filter as i64))
        }

        #[setter("filter")]
        fn set_filter(#[state_mut] st: &mut KeventState, v: PyIndex) -> Result<Value> {
            st.filter = v.expect_i64(FN_NAME, "filter")? as i16;
            Ok(Value::none())
        }

        #[getter]
        fn flags(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::int(st.flags as i64))
        }

        #[setter("flags")]
        fn set_flags(#[state_mut] st: &mut KeventState, v: PyIndex) -> Result<Value> {
            st.flags = v.expect_i64(FN_NAME, "flags")? as u16;
            Ok(Value::none())
        }

        #[getter]
        fn fflags(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::int(st.fflags as i64))
        }

        #[setter("fflags")]
        fn set_fflags(#[state_mut] st: &mut KeventState, v: PyIndex) -> Result<Value> {
            st.fflags = v.expect_i64(FN_NAME, "fflags")? as u32;
            Ok(Value::none())
        }

        #[getter]
        fn data(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::int(st.data as i64))
        }

        #[setter("data")]
        fn set_data(#[state_mut] st: &mut KeventState, v: PyIndex) -> Result<Value> {
            st.data = v.expect_i64(FN_NAME, "data")? as isize;
            Ok(Value::none())
        }

        #[getter]
        fn udata(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::int(st.udata as i64))
        }

        #[setter("udata")]
        fn set_udata(#[state_mut] st: &mut KeventState, v: PyIndex) -> Result<Value> {
            st.udata = v.expect_i64(FN_NAME, "udata")? as usize;
            Ok(Value::none())
        }

        fn __repr__(#[state] st: &KeventState) -> Result<Value> {
            Ok(Value::string(format!(
                "<select.kevent ident={} filter={} flags=0x{:x} fflags=0x{:x} data=0x{:x} udata=0x{:x}>",
                st.ident, st.filter, st.flags, st.fflags, st.data, st.udata
            )))
        }
    }

    #[cfg(target_os = "macos")]
    #[native_state(KqueueState)]
    class kqueue {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let kqfd = unsafe { libc::kqueue() };
            if kqfd < 0 {
                return Err(PyError::from_io_error(&std::io::Error::last_os_error(), None));
            }
            unsafe {
                libc::fcntl(kqfd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
            set_state(self_val.0, KqueueState { kqfd: Some(kqfd) })?;
            Ok(Value::none())
        }

        #[classmethod]
        fn fromfd(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] fd: PyIndex,
        ) -> Result<Value> {
            let raw = fd.expect_i64(FN_NAME, "fd")?;
            let cfd = libc::c_int::try_from(raw).map_err(|_| {
                PyError::named(
                    "OverflowError",
                    "Python int too large to convert to C int",
                )
            })?;
            let ValueKind::PyClass(class_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "fromfd() requires a class"));
            };
            let inst = Value::py_instance(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::PyInstance {
                    class: std::rc::Rc::clone(class_rc),
                    attrs: crate::value::InstanceAttrs::new(),
                },
            )));
            set_state(&inst, KqueueState { kqfd: Some(cfd) })?;
            Ok(inst)
        }

        fn close(#[state_mut] st: &mut KqueueState) -> Result<Value> {
            st.close_internal()?;
            Ok(Value::none())
        }

        #[getter]
        fn closed(#[state] st: &KqueueState) -> Result<Value> {
            Ok(Value::bool_(st.kqfd.is_none()))
        }

        fn fileno(#[state] st: &KqueueState) -> Result<Value> {
            Ok(Value::int(st.open_fd()? as i64))
        }

        fn control(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] changelist: PyValue,
            #[positional_only] max_events: PyIndex,
            #[positional_only]
            #[default(None)]
            timeout: Option<PyNumber>,
        ) -> Result<Value> {
            let kqfd = with_state(self_val.0, |st| st.open_fd())?;
            let max_ev_i64 = max_events.expect_i64(FN_NAME, "max_events")?;
            if max_ev_i64 < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("Length of eventlist must be 0 or positive, got {max_ev_i64}"),
                ));
            }
            let max_ev = (max_ev_i64 as usize).min(65536);

            let mut changes: Vec<libc::kevent> = Vec::new();
            if !matches!(changelist.0.kind(), ValueKind::None) {
                let items = _interp.collect_iterable(&changelist.0)?;
                for item in items {
                    let ev = crate::builtin_modules::macros::native_state::with_native_state::<
                        KeventState,
                        _,
                    >(&item, "kevent", |kev| Ok(kev.to_libc()))
                    .map_err(|_| {
                        PyError::named(
                            "TypeError",
                            "changelist must be an iterable of select.kevent objects",
                        )
                    })?;
                    changes.push(ev);
                }
            }

            let timeout_dur = parse_select_timeout(timeout)?;
            let mut out_events: Vec<libc::kevent> = vec![unsafe { std::mem::zeroed() }; max_ev];
            let deadline = timeout_dur.map(|d| Instant::now() + d);

            let rc = _interp
                .release_gil_during(|| loop {
                    let ts = match (timeout_dur, deadline) {
                        (None, _) => None,
                        (Some(d), _) if d.is_zero() => Some(libc::timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        }),
                        (Some(_), Some(dl)) => {
                            let rem = dl.saturating_duration_since(Instant::now());
                            Some(libc::timespec {
                                tv_sec: rem.as_secs() as libc::time_t,
                                tv_nsec: rem.subsec_nanos() as libc::c_long,
                            })
                        }
                        _ => Some(libc::timespec {
                            tv_sec: 0,
                            tv_nsec: 0,
                        }),
                    };
                    let ts_ptr = match &ts {
                        Some(t) => t as *const libc::timespec,
                        None => std::ptr::null(),
                    };
                    let n = unsafe {
                        libc::kevent(
                            kqfd,
                            if changes.is_empty() {
                                std::ptr::null()
                            } else {
                                changes.as_ptr()
                            },
                            changes.len() as libc::c_int,
                            if out_events.is_empty() {
                                std::ptr::null_mut()
                            } else {
                                out_events.as_mut_ptr()
                            },
                            out_events.len() as libc::c_int,
                            ts_ptr,
                        )
                    };
                    if n < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.kind() == std::io::ErrorKind::Interrupted {
                            if let Some(dl) = deadline
                                && Instant::now() >= dl
                            {
                                break Ok(0);
                            }
                            continue;
                        }
                        break Err(err);
                    }
                    break Ok(n);
                })
                .map_err(|err| PyError::from_io_error(&err, None))?;

            let select_mod = _interp.load_module("select")?;
            let kevent_cls = _interp.get_attr(&select_mod, "kevent")?;
            let ValueKind::PyClass(class_rc) = kevent_cls.kind() else {
                return Err(PyError::runtime("select.kevent is not a class".into()));
            };
            let mut results = Vec::with_capacity(rc as usize);
            for ev in &out_events[..rc as usize] {
                let inst = Value::py_instance(std::rc::Rc::new(std::cell::RefCell::new(
                    crate::value::PyInstance {
                        class: std::rc::Rc::clone(class_rc),
                        attrs: crate::value::InstanceAttrs::new(),
                    },
                )));
                crate::builtin_modules::macros::native_state::set_native_state(
                    &inst,
                    KeventState::from_libc(*ev),
                )?;
                results.push(inst);
            }
            Ok(Value::list(results))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| st.open_fd())?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| st.close_internal())?;
            Ok(Value::none())
        }
    }
}

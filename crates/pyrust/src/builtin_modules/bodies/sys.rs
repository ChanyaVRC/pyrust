// `sys` registration facade — included into `pub mod sys { … }` declared by
// `pyrust_builtin_modules!` in `builtin_modules/mod.rs`.
//
// Python-visible constants and callables stay declared here while private
// submodules implement focused responsibilities:
//
// - `runtime_limits`: recursion, int-str-digits, switch interval, coroutine tracking;
// - `introspection`: exit, exc_info/exception, _getframe, intern, getsizeof, encodings;
// - `hooks`: profile, trace, asyncgen hooks, audit hooks, display/except/unraisable/breakpoint;
// - `version_info`: `sys.version_info` and `sys.flags` classes and dunder dispatch;
// - `platform_info`: platform constants, struct-sequence builders, and module-name sets;
// - `namespace`: the startup namespace table and its deferred heavy slots (issue #3293).
//
// Reference: <https://docs.python.org/3/library/sys.html>

use std::cmp::Ordering;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;
use pyrust_derive::pyrust_module;

#[path = "sys/hooks.rs"]
mod hooks;
#[path = "sys/introspection.rs"]
mod introspection;
#[path = "sys/monitoring.rs"]
mod monitoring;
#[path = "sys/namespace.rs"]
mod namespace;
#[path = "sys/platform_info.rs"]
mod platform_info;
#[path = "sys/runtime_limits.rs"]
mod runtime_limits;
#[path = "sys/version_info.rs"]
mod version_info;

pub(crate) use exports::REGS;
pub(crate) use namespace::module;
pub(crate) use platform_info::{detect_pyvenv_startup, parse_pyvenv_cfg};
pub(crate) use version_info::deferred_flags;

#[cfg(test)]
pub(crate) use namespace::deferred_slot_builds;

const PYRUST_COPYRIGHT: &str = "Copyright (c) 2001-2023 Python Software Foundation.\nAll Rights Reserved.\n\nPyRust 0.2 (Python 3.12 compatible runtime).";

/// The `sys` callables and their registry entries.
///
/// `pyrust_module!` also emits a generic `module()` builder here; `sys` is
/// assembled by `namespace::module` instead, which walks these exports once and
/// leaves the heavy attributes deferred.
#[allow(dead_code)]
mod exports {
    use super::*;

    pyrust_module! {
        fn exit(args) -> Result<Value> {
            introspection::exit(_interp, args, FN_NAME)
        }

        fn getrecursionlimit(args) -> Result<Value> {
            runtime_limits::getrecursionlimit(_interp, args, FN_NAME)
        }

        fn setrecursionlimit(args) -> Result<Value> {
            runtime_limits::setrecursionlimit(_interp, args, FN_NAME)
        }

        fn get_int_max_str_digits(args) -> Result<Value> {
            runtime_limits::get_int_max_str_digits(_interp, args, FN_NAME)
        }

        fn set_int_max_str_digits(args) -> Result<Value> {
            runtime_limits::set_int_max_str_digits(_interp, args, FN_NAME)
        }

        fn exc_info(args) -> Result<Value> {
            introspection::exc_info(_interp, args, FN_NAME)
        }

        fn _getframe(args) -> Result<Value> {
            introspection::getframe(_interp, args, FN_NAME)
        }

        fn exception(args) -> Result<Value> {
            introspection::exception(_interp, args, FN_NAME)
        }

        fn intern(args) -> Result<Value> {
            introspection::intern(args, FN_NAME)
        }

        fn getsizeof(args) -> Result<Value> {
            introspection::getsizeof(args, FN_NAME)
        }

        fn getdefaultencoding(args) -> Result<Value> {
            introspection::getdefaultencoding(args, FN_NAME)
        }

        fn is_finalizing(args) -> Result<Value> {
            introspection::is_finalizing(args, FN_NAME)
        }

        fn getfilesystemencoding(args) -> Result<Value> {
            introspection::getfilesystemencoding(args, FN_NAME)
        }

        fn getfilesystemencodeerrors(args) -> Result<Value> {
            introspection::getfilesystemencodeerrors(args, FN_NAME)
        }

        fn getswitchinterval(args) -> Result<Value> {
            runtime_limits::getswitchinterval(_interp, args, FN_NAME)
        }

        fn setswitchinterval(args) -> Result<Value> {
            runtime_limits::setswitchinterval(_interp, args, FN_NAME)
        }

        fn get_coroutine_origin_tracking_depth(args) -> Result<Value> {
            runtime_limits::get_coroutine_origin_tracking_depth(_interp, args, FN_NAME)
        }

        fn set_coroutine_origin_tracking_depth(args) -> Result<Value> {
            runtime_limits::set_coroutine_origin_tracking_depth(_interp, args)
        }

        fn getprofile(args) -> Result<Value> {
            hooks::getprofile(_interp, args, FN_NAME)
        }

        fn setprofile(args) -> Result<Value> {
            hooks::setprofile(_interp, args, FN_NAME)
        }

        fn gettrace(args) -> Result<Value> {
            hooks::gettrace(_interp, args, FN_NAME)
        }

        fn settrace(args) -> Result<Value> {
            hooks::settrace(_interp, args, FN_NAME)
        }

        fn get_asyncgen_hooks(args) -> Result<Value> {
            hooks::get_asyncgen_hooks(_interp, args, FN_NAME)
        }

        fn set_asyncgen_hooks(args) -> Result<Value> {
            hooks::set_asyncgen_hooks(_interp, args)
        }

        fn addaudithook(args) -> Result<Value> {
            hooks::addaudithook(_interp, args)
        }

        fn audit(args) -> Result<Value> {
            hooks::audit(_interp, args, FN_NAME)
        }

        fn displayhook(args) -> Result<Value> {
            hooks::displayhook(_interp, args, FN_NAME)
        }

        fn excepthook(args) -> Result<Value> {
            hooks::excepthook(_interp, args, FN_NAME)
        }

        fn unraisablehook(args) -> Result<Value> {
            hooks::unraisablehook(_interp, args)
        }

        fn breakpointhook(args) -> Result<Value> {
            hooks::breakpointhook(_interp, args)
        }

        fn call_tracing(args) -> Result<Value> {
            hooks::call_tracing(_interp, args, FN_NAME)
        }

        fn getallocatedblocks(args) -> Result<Value> {
            introspection::getallocatedblocks(args, FN_NAME)
        }

        fn _clear_type_cache(args) -> Result<Value> {
            introspection::clear_type_cache(args, FN_NAME)
        }

        fn getrefcount(args) -> Result<Value> {
            introspection::getrefcount(args, FN_NAME)
        }

        fn _getframemodulename(args) -> Result<Value> {
            introspection::getframemodulename(_interp, args, FN_NAME)
        }

        fn _current_frames(args) -> Result<Value> {
            introspection::current_frames(_interp, args, FN_NAME)
        }

        fn _current_exceptions(args) -> Result<Value> {
            introspection::current_exceptions(_interp, args, FN_NAME)
        }

        #[cfg(windows)]
        fn getwindowsversion(args) -> Result<Value> {
            runtime_limits::require_no_args(args, FN_NAME, "getwindowsversion")?;
            Ok(platform_info::make_windows_version())
        }

        // ── sys.monitoring (PEP 669) callables ───────────────────────────────────

        #[py_name = "monitoring.use_tool_id"]
        fn monitoring_use_tool_id(args) -> Result<Value> {
            monitoring::use_tool_id(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.free_tool_id"]
        fn monitoring_free_tool_id(args) -> Result<Value> {
            monitoring::free_tool_id(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.clear_tool_id"]
        fn monitoring_clear_tool_id(args) -> Result<Value> {
            monitoring::clear_tool_id(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.get_tool"]
        fn monitoring_get_tool(args) -> Result<Value> {
            monitoring::get_tool(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.get_events"]
        fn monitoring_get_events(args) -> Result<Value> {
            monitoring::get_events(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.set_events"]
        fn monitoring_set_events(args) -> Result<Value> {
            monitoring::set_events(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.get_local_events"]
        fn monitoring_get_local_events(args) -> Result<Value> {
            monitoring::get_local_events(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.set_local_events"]
        fn monitoring_set_local_events(args) -> Result<Value> {
            monitoring::set_local_events(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.register_callback"]
        fn monitoring_register_callback(args) -> Result<Value> {
            monitoring::register_callback(_interp, args, FN_NAME)
        }

        #[py_name = "monitoring.restart_events"]
        fn monitoring_restart_events(args) -> Result<Value> {
            monitoring::restart_events(args, FN_NAME)
        }

        #[py_name = "monitoring._all_events"]
        fn monitoring_all_events(args) -> Result<Value> {
            monitoring::all_events(args, FN_NAME)
        }

        // ── version_info rich-comparison and sequence methods ────────────────────

        #[py_name = "version_info.__ge__"]
        fn version_info_ge(args) -> Result<Value> {
            version_info::version_info_cmp(args, |ord| ord != Ordering::Less)
        }

        #[py_name = "version_info.__gt__"]
        fn version_info_gt(args) -> Result<Value> {
            version_info::version_info_cmp(args, |ord| ord == Ordering::Greater)
        }

        #[py_name = "version_info.__le__"]
        fn version_info_le(args) -> Result<Value> {
            version_info::version_info_cmp(args, |ord| ord != Ordering::Greater)
        }

        #[py_name = "version_info.__lt__"]
        fn version_info_lt(args) -> Result<Value> {
            version_info::version_info_cmp(args, |ord| ord == Ordering::Less)
        }

        #[py_name = "version_info.__eq__"]
        fn version_info_eq(args) -> Result<Value> {
            version_info::version_info_cmp(args, |ord| ord == Ordering::Equal)
        }

        #[py_name = "version_info.__ne__"]
        fn version_info_ne(args) -> Result<Value> {
            version_info::version_info_cmp(args, |ord| ord != Ordering::Equal)
        }

        #[py_name = "version_info.__repr__"]
        fn version_info_repr(args) -> Result<Value> {
            version_info::version_info_repr(args)
        }

        #[py_name = "version_info.__getitem__"]
        fn version_info_getitem(args) -> Result<Value> {
            version_info::version_info_getitem(args)
        }

        #[py_name = "version_info.__len__"]
        fn version_info_len(args) -> Result<Value> {
            version_info::version_info_len(args)
        }
    }
}

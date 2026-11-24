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
// - `platform_info`: platform constants, struct-sequence builders, and module-name sets.
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
#[path = "sys/platform_info.rs"]
mod platform_info;
#[path = "sys/runtime_limits.rs"]
mod runtime_limits;
#[path = "sys/version_info.rs"]
mod version_info;

pyrust_module! {
    constants {
        "version"              => Value::string("PyRust 0.2"),
        "argv"                 => Value::list(Vec::new()),
        "maxsize"              => Value::int(i64::MAX),
        "platform"             => Value::string(platform_info::sys_platform()),
        "version_info"         => version_info::make_version_info(),
        "stdout"               => pyrust_builtins::file::make_stdout(),
        "stderr"               => pyrust_builtins::file::make_stderr(),
        "stdin"                => pyrust_builtins::file::make_stdin(),
        "path"                 => Value::list(vec![Value::string("")]),
        // Placeholder replaced by `prepare_builtin_module` with the importing
        // Interpreter's authoritative registry before import-cache insertion.
        "modules"              => Value::dict(crate::value::PyDict::default()),
        "executable"           => Value::string(""),
        "byteorder"            => Value::string(platform_info::sys_byteorder()),
        "prefix"               => Value::string(""),
        "exec_prefix"          => Value::string(""),
        "base_prefix"          => Value::string(""),
        "base_exec_prefix"     => Value::string(""),
        // 3.12.0 final → 0x030c00f0
        "hexversion"           => Value::int(0x030c_00f0),
        "copyright"            => Value::string(""),
        "flags"                => version_info::make_flags(),
        "maxunicode"           => Value::int(1_114_111),
        "abiflags"             => Value::string(""),
        "float_info"           => platform_info::make_float_info(),
        "int_info"             => platform_info::make_int_info(),
        "hash_info"            => platform_info::make_hash_info(),
        "implementation"       => platform_info::make_implementation(),
        "builtin_module_names" => platform_info::make_builtin_module_names(),
        "api_version"          => Value::int(1013),
        "dont_write_bytecode"  => Value::bool_(false),
        "float_repr_style"     => Value::string("short"),
        "platlibdir"           => Value::string(platform_info::sys_platlibdir()),
        "pycache_prefix"       => Value::none(),
        "orig_argv"            => Value::list(Vec::new()),
        "warnoptions"          => Value::list(Vec::new()),
        "meta_path"            => Value::list(Vec::new()),
        "path_hooks"           => Value::list(Vec::new()),
        "path_importer_cache"  => Value::dict(crate::value::PyDict::default()),
        "_xoptions"            => Value::dict(crate::value::PyDict::default()),
        "stdlib_module_names"  => platform_info::make_stdlib_module_names(),
        "thread_info"          => platform_info::make_thread_info(),
        "__stdout__"           => Value::none(),
        "__stderr__"           => Value::none(),
        "__stdin__"            => Value::none(),
        "__displayhook__"      => Value::none(),
        "__excepthook__"       => Value::none(),
        "__unraisablehook__"   => Value::none(),
        "__breakpointhook__"   => Value::none(),
    }

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
        hooks::breakpointhook(args)
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

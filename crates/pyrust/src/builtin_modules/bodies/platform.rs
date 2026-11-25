// `platform` module — body for the `platform @inject` entry in
// `pyrust_builtin_modules!` (issue #3209).
//
// Native OS probes (`_host_uname`, `_host_win32_ver`, `_host_processor`,
// `_host_libc_ver`, `_host_probe_executable`, `_host_compiler`) back the
// CPython 3.12 `platform` API injected from `platform_py.py`.
//
// Reference: <https://docs.python.org/3.12/library/platform.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, reject_keyword_args_expanded};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the `platform` module.
const PLATFORM_PY_SOURCE: &str = include_str!("platform_py.py");

/// Private native helpers moved into the exec namespace and removed from the
/// public module dict so `dir(platform)` stays clean.
const PLATFORM_PRIVATE_HELPERS: &[&str] = &[
    "_host_uname",
    "_host_win32_ver",
    "_host_processor",
    "_host_libc_ver",
    "_host_probe_executable",
    "_host_compiler",
];

/// Public attributes exported onto the `platform` module.
const PLATFORM_PY_EXPORTS: &[&str] = &[
    "__all__",
    "uname_result",
    "uname",
    "system",
    "node",
    "release",
    "version",
    "machine",
    "processor",
    "architecture",
    "platform",
    "system_alias",
    "win32_ver",
    "libc_ver",
    "python_implementation",
    "python_version",
    "python_version_tuple",
    "python_branch",
    "python_revision",
    "python_build",
    "python_compiler",
];

/// Execute `PLATFORM_PY_SOURCE` once and publish its exports onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in PLATFORM_PRIVATE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(
        PLATFORM_PY_SOURCE,
        "<frozen platform>",
        Some(ns.clone()),
        None,
    )?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("platform: exec namespace not a dict".into()))?;
    for &name in PLATFORM_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("platform"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    drop(dict);
    Ok(Some(ns))
}

#[cfg(windows)]
mod win32_info {
    #[repr(C)]
    struct OsVersionInfoExW {
        dw_os_version_info_size: u32,
        dw_major_version: u32,
        dw_minor_version: u32,
        dw_build_number: u32,
        dw_platform_id: u32,
        sz_csd_version: [u16; 128],
        w_service_pack_major: u16,
        w_service_pack_minor: u16,
        w_suite_mask: u16,
        w_product_type: u8,
        w_reserved: u8,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(lp_version_information: *mut OsVersionInfoExW) -> i32;
    }

    const CLIENT_RELEASES: &[((u32, u32, u32), &str)] = &[
        ((10, 1, 0), "post11"),
        ((10, 0, 22000), "11"),
        ((6, 4, 0), "10"),
        ((6, 3, 0), "8.1"),
        ((6, 2, 0), "8"),
        ((6, 1, 0), "7"),
        ((6, 0, 0), "Vista"),
        ((5, 2, 3790), "XP64"),
        ((5, 2, 0), "XPMedia"),
        ((5, 1, 0), "XP"),
        ((5, 0, 0), "2000"),
    ];

    const SERVER_RELEASES: &[((u32, u32, u32), &str)] = &[
        ((10, 1, 0), "post2025Server"),
        ((10, 0, 26100), "2025Server"),
        ((10, 0, 20348), "2022Server"),
        ((10, 0, 17763), "2019Server"),
        ((6, 4, 0), "2016Server"),
        ((6, 3, 0), "2012ServerR2"),
        ((6, 2, 0), "2012Server"),
        ((6, 1, 0), "2008ServerR2"),
        ((6, 0, 0), "2008Server"),
        ((5, 2, 0), "2003Server"),
        ((5, 0, 0), "2000Server"),
    ];

    pub(super) fn query_win32_ver() -> (String, String, String, String) {
        let mut info = OsVersionInfoExW {
            dw_os_version_info_size: std::mem::size_of::<OsVersionInfoExW>() as u32,
            dw_major_version: 0,
            dw_minor_version: 0,
            dw_build_number: 0,
            dw_platform_id: 0,
            sz_csd_version: [0; 128],
            w_service_pack_major: 0,
            w_service_pack_minor: 0,
            w_suite_mask: 0,
            w_product_type: 0,
            w_reserved: 0,
        };
        // SAFETY: `info` is properly sized with `dw_os_version_info_size` set to
        // `size_of::<OsVersionInfoExW>()` as required by `RtlGetVersion`.
        let status = unsafe { RtlGetVersion(&mut info) };
        if status != 0 {
            return (String::new(), String::new(), String::new(), String::new());
        }
        let tuple = (
            info.dw_major_version,
            info.dw_minor_version,
            info.dw_build_number,
        );
        let version = format!("{}.{}.{}", tuple.0, tuple.1, tuple.2);
        let is_client = info.w_product_type == 1;
        let table = if is_client {
            CLIENT_RELEASES
        } else {
            SERVER_RELEASES
        };
        let release = table
            .iter()
            .find(|&&(v, _)| v <= tuple)
            .map(|&(_, r)| r.to_string())
            .unwrap_or_default();
        let csd = format!("SP{}", info.w_service_pack_major);
        (release, version, csd, "Multiprocessor Free".to_string())
    }

    pub(super) fn query_machine() -> String {
        if let Ok(val) = std::env::var("PROCESSOR_ARCHITEW6432")
            && !val.is_empty()
        {
            return val;
        }
        if let Ok(val) = std::env::var("PROCESSOR_ARCHITECTURE")
            && !val.is_empty()
        {
            return val;
        }
        match std::env::consts::ARCH {
            "x86_64" => "AMD64".to_string(),
            "aarch64" => "ARM64".to_string(),
            "x86" => "x86".to_string(),
            other => other.to_string(),
        }
    }
}

fn host_uname_strings() -> (String, String, String, String, String) {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        // SAFETY: `libc::utsname` is a plain C struct of fixed byte arrays and
        // `libc::uname` populates all five fields on success.
        let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::uname(&mut uts) };
        if rc == 0 {
            let to_str = |buf: &[libc::c_char]| -> String {
                // SAFETY: `uname` NUL-terminates each field on success.
                unsafe { CStr::from_ptr(buf.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            };
            return (
                to_str(&uts.sysname),
                to_str(&uts.nodename),
                to_str(&uts.release),
                to_str(&uts.version),
                to_str(&uts.machine),
            );
        }
        (
            std::env::consts::OS.to_string(),
            String::new(),
            String::new(),
            String::new(),
            std::env::consts::ARCH.to_string(),
        )
    }

    #[cfg(windows)]
    {
        let node = std::env::var("COMPUTERNAME").unwrap_or_default();
        let (release, version, _, _) = win32_info::query_win32_ver();
        let machine = win32_info::query_machine();
        ("Windows".to_string(), node, release, version, machine)
    }

    #[cfg(not(any(unix, windows)))]
    {
        (
            std::env::consts::OS.to_string(),
            String::new(),
            String::new(),
            String::new(),
            std::env::consts::ARCH.to_string(),
        )
    }
}

fn host_win32_ver_strings() -> (String, String, String, String) {
    #[cfg(windows)]
    {
        win32_info::query_win32_ver()
    }
    #[cfg(not(windows))]
    {
        (String::new(), String::new(), String::new(), String::new())
    }
}

fn host_processor_string() -> String {
    #[cfg(windows)]
    {
        std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| win32_info::query_machine())
    }
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("uname").arg("-p").output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !text.is_empty() && text != "unknown" {
                return text;
            }
        }
        String::new()
    }
    #[cfg(not(any(unix, windows)))]
    {
        String::new()
    }
}

fn host_libc_ver_strings() -> (String, String) {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use std::ffi::CStr;
        // SAFETY: `gnu_get_libc_version` returns a pointer to a static NUL-terminated
        // version string owned by glibc.
        let ptr = unsafe { libc::gnu_get_libc_version() };
        if !ptr.is_null() {
            let ver = unsafe { CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned();
            return ("glibc".to_string(), ver);
        }
        (String::new(), String::new())
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    {
        (String::new(), String::new())
    }
}

fn host_probe_executable_header(path: &str) -> (String, String) {
    use std::io::Read;
    if path.is_empty() {
        return (String::new(), String::new());
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return (String::new(), String::new());
    };
    let mut header = [0u8; 64];
    let Ok(n) = file.read(&mut header) else {
        return (String::new(), String::new());
    };
    if n >= 5 && &header[..4] == b"\x7fELF" {
        let bits = match header[4] {
            1 => "32bit",
            2 => "64bit",
            _ => "",
        };
        return (bits.to_string(), "ELF".to_string());
    }
    (String::new(), String::new())
}

fn host_compiler_string() -> String {
    format!("Rust ({}/{})", std::env::consts::OS, std::env::consts::ARCH)
}

pyrust_module! {
    fn _host_uname(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        let (system, node, release, version, machine) = host_uname_strings();
        Ok(Value::tuple(vec![
            Value::string(system),
            Value::string(node),
            Value::string(release),
            Value::string(version),
            Value::string(machine),
        ]))
    }

    fn _host_win32_ver(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        let (release, version, csd, ptype) = host_win32_ver_strings();
        Ok(Value::tuple(vec![
            Value::string(release),
            Value::string(version),
            Value::string(csd),
            Value::string(ptype),
        ]))
    }

    fn _host_processor(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::string(host_processor_string()))
    }

    fn _host_libc_ver(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        let (lib, ver) = host_libc_ver_strings();
        Ok(Value::tuple(vec![Value::string(lib), Value::string(ver)]))
    }

    fn _host_probe_executable(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        let path = match args.first().map(|a| a.value.kind()) {
            Some(ValueKind::Str(s)) => s,
            _ => "",
        };
        let (bits, linkage) = host_probe_executable_header(path);
        Ok(Value::tuple(vec![
            Value::string(bits),
            Value::string(linkage),
        ]))
    }

    fn _host_compiler(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        Ok(Value::string(host_compiler_string()))
    }
}

// `ctypes.wintypes` module — 100% native-Rust Windows C type & structure definitions (issue #3265).
//
// Importable on all platforms (matching CPython 3.12's `Lib/ctypes/wintypes.py`).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::ctypes_rt::types::{
    ByteOrder, C_LONG_SIZE, CTypeInfo, PTR_SIZE, PrimKind, core_classes, register_ctype,
};
use crate::builtin_modules::ctypes_rt::{
    finalize_struct_fields, make_array_type, make_pointer_type,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyIndex;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, PyModule, Value, ValueKind};

const MAX_PATH: i64 = 260;

pyrust_module! {
    constants {
        "MAX_PATH" => Value::int(MAX_PATH),
    }

    #[py_name = "RGB"]
    fn rgb(red: PyIndex, green: PyIndex, blue: PyIndex) -> Result<Value> {
        let r = red.expect_i64(FN_NAME, "red")?;
        let g = green.expect_i64(FN_NAME, "green")?;
        let b = blue.expect_i64(FN_NAME, "blue")?;
        Ok(Value::int(r + (g << 8) + (b << 16)))
    }
}

fn get_cls(val: &Value, name: &str) -> Result<Rc<RefCell<PyClass>>> {
    match val.kind() {
        ValueKind::PyClass(c) => Ok(Rc::clone(c)),
        _ => Err(PyError::runtime(format!(
            "internal: ctypes.{name} is not a class"
        ))),
    }
}

fn make_wintypes_struct(
    interp: &mut Interpreter,
    name: &str,
    fields: &[(&str, &Rc<RefCell<PyClass>>)],
) -> Result<Rc<RefCell<PyClass>>> {
    let core = core_classes()?;
    let mut attrs = IndexMap::new();
    attrs.insert("__module__".to_string(), Value::string("ctypes.wintypes"));
    let mut cls = PyClass::new(name, name, Some(Rc::clone(&core.struct_base)), attrs);
    cls.metatype = Some(Rc::clone(&core.struct_meta));
    let cls_rc = Rc::new(RefCell::new(cls));
    core.struct_base
        .borrow()
        .subclasses
        .borrow_mut()
        .push(Rc::downgrade(&cls_rc));

    let fields_list = Value::list(
        fields
            .iter()
            .map(|(fname, fcls)| {
                Value::tuple(vec![
                    Value::string(*fname),
                    Value::py_class(Rc::clone(fcls)),
                ])
            })
            .collect(),
    );
    finalize_struct_fields(interp, &cls_rc, &fields_list)?;
    Ok(cls_rc)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ctypes_mod = interp.load_module("ctypes")?;
    let core = core_classes()?;

    let c_ubyte = get_cls(&interp.get_attr(&ctypes_mod, "c_ubyte")?, "c_ubyte")?;
    let c_ushort = get_cls(&interp.get_attr(&ctypes_mod, "c_ushort")?, "c_ushort")?;
    let c_ulong = get_cls(&interp.get_attr(&ctypes_mod, "c_ulong")?, "c_ulong")?;
    let c_char = get_cls(&interp.get_attr(&ctypes_mod, "c_char")?, "c_char")?;
    let c_wchar = get_cls(&interp.get_attr(&ctypes_mod, "c_wchar")?, "c_wchar")?;
    let c_uint = get_cls(&interp.get_attr(&ctypes_mod, "c_uint")?, "c_uint")?;
    let c_int = get_cls(&interp.get_attr(&ctypes_mod, "c_int")?, "c_int")?;
    let c_double = get_cls(&interp.get_attr(&ctypes_mod, "c_double")?, "c_double")?;
    let c_float = get_cls(&interp.get_attr(&ctypes_mod, "c_float")?, "c_float")?;
    let c_long = get_cls(&interp.get_attr(&ctypes_mod, "c_long")?, "c_long")?;
    let c_short = get_cls(&interp.get_attr(&ctypes_mod, "c_short")?, "c_short")?;
    let c_longlong = get_cls(&interp.get_attr(&ctypes_mod, "c_longlong")?, "c_longlong")?;
    let c_ulonglong = get_cls(&interp.get_attr(&ctypes_mod, "c_ulonglong")?, "c_ulonglong")?;
    let c_char_p = get_cls(&interp.get_attr(&ctypes_mod, "c_char_p")?, "c_char_p")?;
    let c_wchar_p = get_cls(&interp.get_attr(&ctypes_mod, "c_wchar_p")?, "c_wchar_p")?;
    let c_void_p = get_cls(&interp.get_attr(&ctypes_mod, "c_void_p")?, "c_void_p")?;

    // `VARIANT_BOOL` subclass of `_SimpleCData` (`_type_ = "v"`)
    let variant_bool = {
        let mut attrs = IndexMap::new();
        attrs.insert("__module__".to_string(), Value::string("ctypes.wintypes"));
        attrs.insert("_type_".to_string(), Value::string("v"));
        let mut cls = PyClass::new(
            "VARIANT_BOOL",
            "VARIANT_BOOL",
            Some(Rc::clone(&core.simple_base)),
            attrs,
        );
        cls.metatype = Some(Rc::clone(&core.simple_meta));
        let cls_rc = Rc::new(RefCell::new(cls));
        core.simple_base
            .borrow()
            .subclasses
            .borrow_mut()
            .push(Rc::downgrade(&cls_rc));
        register_ctype(
            &cls_rc,
            CTypeInfo::Simple {
                kind: PrimKind::VariantBool,
                byte_order: ByteOrder::Native,
            },
        );
        cls_rc
    };

    // Scalar aliases matching CPython 3.12 `Lib/ctypes/wintypes.py`
    let byte = Rc::clone(&c_ubyte);
    let word = Rc::clone(&c_ushort);
    let dword = Rc::clone(&c_ulong);
    let boolean = Rc::clone(&byte);
    let bool_t = Rc::clone(&c_long);
    let ulong = Rc::clone(&c_ulong);
    let long = Rc::clone(&c_long);
    let ushort = Rc::clone(&c_ushort);
    let short = Rc::clone(&c_short);
    let large_integer = Rc::clone(&c_longlong);
    let ularge_integer = Rc::clone(&c_ulonglong);
    let (wparam, lparam) = if C_LONG_SIZE == PTR_SIZE {
        (Rc::clone(&c_ulong), Rc::clone(&c_long))
    } else {
        (Rc::clone(&c_ulonglong), Rc::clone(&c_longlong))
    };
    let handle = Rc::clone(&c_void_p);

    // Structures
    let rect = make_wintypes_struct(
        interp,
        "RECT",
        &[
            ("left", &long),
            ("top", &long),
            ("right", &long),
            ("bottom", &long),
        ],
    )?;
    let small_rect = make_wintypes_struct(
        interp,
        "_SMALL_RECT",
        &[
            ("Left", &short),
            ("Top", &short),
            ("Right", &short),
            ("Bottom", &short),
        ],
    )?;
    let coord = make_wintypes_struct(interp, "_COORD", &[("X", &short), ("Y", &short)])?;
    let point = make_wintypes_struct(interp, "POINT", &[("x", &long), ("y", &long)])?;
    let size = make_wintypes_struct(interp, "SIZE", &[("cx", &long), ("cy", &long)])?;
    let filetime = make_wintypes_struct(
        interp,
        "FILETIME",
        &[("dwLowDateTime", &dword), ("dwHighDateTime", &dword)],
    )?;
    let msg = make_wintypes_struct(
        interp,
        "MSG",
        &[
            ("hWnd", &handle),
            ("message", &c_uint),
            ("wParam", &wparam),
            ("lParam", &lparam),
            ("time", &dword),
            ("pt", &point),
        ],
    )?;

    let char_max_path = make_array_type(interp, &c_char, MAX_PATH as usize)?;
    let char_14 = make_array_type(interp, &c_char, 14)?;
    let wchar_max_path = make_array_type(interp, &c_wchar, MAX_PATH as usize)?;
    let wchar_14 = make_array_type(interp, &c_wchar, 14)?;

    let find_data_a = make_wintypes_struct(
        interp,
        "WIN32_FIND_DATAA",
        &[
            ("dwFileAttributes", &dword),
            ("ftCreationTime", &filetime),
            ("ftLastAccessTime", &filetime),
            ("ftLastWriteTime", &filetime),
            ("nFileSizeHigh", &dword),
            ("nFileSizeLow", &dword),
            ("dwReserved0", &dword),
            ("dwReserved1", &dword),
            ("cFileName", &char_max_path),
            ("cAlternateFileName", &char_14),
        ],
    )?;
    let find_data_w = make_wintypes_struct(
        interp,
        "WIN32_FIND_DATAW",
        &[
            ("dwFileAttributes", &dword),
            ("ftCreationTime", &filetime),
            ("ftLastAccessTime", &filetime),
            ("ftLastWriteTime", &filetime),
            ("nFileSizeHigh", &dword),
            ("nFileSizeLow", &dword),
            ("dwReserved0", &dword),
            ("dwReserved1", &dword),
            ("cFileName", &wchar_max_path),
            ("cAlternateFileName", &wchar_14),
        ],
    )?;

    // Pointer types
    let pbool = make_pointer_type(interp, Some(&bool_t))?;
    let pboolean = make_pointer_type(interp, Some(&boolean))?;
    let pbyte = make_pointer_type(interp, Some(&byte))?;
    let pchar = make_pointer_type(interp, Some(&c_char))?;
    let pdword = make_pointer_type(interp, Some(&dword))?;
    let pfiletime = make_pointer_type(interp, Some(&filetime))?;
    let pfloat = make_pointer_type(interp, Some(&c_float))?;
    let phandle = make_pointer_type(interp, Some(&handle))?;
    let pint = make_pointer_type(interp, Some(&c_int))?;
    let plarge_integer = make_pointer_type(interp, Some(&large_integer))?;
    let plong = make_pointer_type(interp, Some(&long))?;
    let pmsg = make_pointer_type(interp, Some(&msg))?;
    let ppoint = make_pointer_type(interp, Some(&point))?;
    let prect = make_pointer_type(interp, Some(&rect))?;
    let pshort = make_pointer_type(interp, Some(&short))?;
    let psize = make_pointer_type(interp, Some(&size))?;
    let psmall_rect = make_pointer_type(interp, Some(&small_rect))?;
    let puint = make_pointer_type(interp, Some(&c_uint))?;
    let pularge_integer = make_pointer_type(interp, Some(&ularge_integer))?;
    let pulong = make_pointer_type(interp, Some(&ulong))?;
    let pushort = make_pointer_type(interp, Some(&ushort))?;
    let pwchar = make_pointer_type(interp, Some(&c_wchar))?;
    let pfind_data_a = make_pointer_type(interp, Some(&find_data_a))?;
    let pfind_data_w = make_pointer_type(interp, Some(&find_data_w))?;
    let pword = make_pointer_type(interp, Some(&word))?;

    let mut m = module.borrow_mut();
    m.insert_attr("ctypes".to_string(), ctypes_mod);

    let type_bindings: &[(&str, &Rc<RefCell<PyClass>>)] = &[
        ("BYTE", &byte),
        ("WORD", &word),
        ("DWORD", &dword),
        ("CHAR", &c_char),
        ("WCHAR", &c_wchar),
        ("UINT", &c_uint),
        ("INT", &c_int),
        ("DOUBLE", &c_double),
        ("FLOAT", &c_float),
        ("BOOLEAN", &boolean),
        ("BOOL", &bool_t),
        ("VARIANT_BOOL", &variant_bool),
        ("ULONG", &ulong),
        ("LONG", &long),
        ("USHORT", &ushort),
        ("SHORT", &short),
        ("_LARGE_INTEGER", &large_integer),
        ("LARGE_INTEGER", &large_integer),
        ("_ULARGE_INTEGER", &ularge_integer),
        ("ULARGE_INTEGER", &ularge_integer),
        ("LPCOLESTR", &c_wchar_p),
        ("LPOLESTR", &c_wchar_p),
        ("OLESTR", &c_wchar_p),
        ("LPCWSTR", &c_wchar_p),
        ("LPWSTR", &c_wchar_p),
        ("LPCSTR", &c_char_p),
        ("LPSTR", &c_char_p),
        ("LPCVOID", &c_void_p),
        ("LPVOID", &c_void_p),
        ("WPARAM", &wparam),
        ("LPARAM", &lparam),
        ("ATOM", &word),
        ("LANGID", &word),
        ("COLORREF", &dword),
        ("LGRPID", &dword),
        ("LCTYPE", &dword),
        ("LCID", &dword),
        ("HANDLE", &handle),
        ("HACCEL", &handle),
        ("HBITMAP", &handle),
        ("HBRUSH", &handle),
        ("HCOLORSPACE", &handle),
        ("HDC", &handle),
        ("HDESK", &handle),
        ("HDWP", &handle),
        ("HENHMETAFILE", &handle),
        ("HFONT", &handle),
        ("HGDIOBJ", &handle),
        ("HGLOBAL", &handle),
        ("HHOOK", &handle),
        ("HICON", &handle),
        ("HINSTANCE", &handle),
        ("HKEY", &handle),
        ("HKL", &handle),
        ("HLOCAL", &handle),
        ("HMENU", &handle),
        ("HMETAFILE", &handle),
        ("HMODULE", &handle),
        ("HMONITOR", &handle),
        ("HPALETTE", &handle),
        ("HPEN", &handle),
        ("HRGN", &handle),
        ("HRSRC", &handle),
        ("HSTR", &handle),
        ("HTASK", &handle),
        ("HWINSTA", &handle),
        ("HWND", &handle),
        ("SC_HANDLE", &handle),
        ("SERVICE_STATUS_HANDLE", &handle),
        ("RECT", &rect),
        ("tagRECT", &rect),
        ("_RECTL", &rect),
        ("RECTL", &rect),
        ("_SMALL_RECT", &small_rect),
        ("SMALL_RECT", &small_rect),
        ("_COORD", &coord),
        ("POINT", &point),
        ("tagPOINT", &point),
        ("_POINTL", &point),
        ("POINTL", &point),
        ("SIZE", &size),
        ("tagSIZE", &size),
        ("SIZEL", &size),
        ("FILETIME", &filetime),
        ("_FILETIME", &filetime),
        ("MSG", &msg),
        ("tagMSG", &msg),
        ("WIN32_FIND_DATAA", &find_data_a),
        ("WIN32_FIND_DATAW", &find_data_w),
        ("LPBOOL", &pbool),
        ("PBOOL", &pbool),
        ("PBOOLEAN", &pboolean),
        ("LPBYTE", &pbyte),
        ("PBYTE", &pbyte),
        ("PCHAR", &pchar),
        ("LPCOLORREF", &pdword),
        ("LPDWORD", &pdword),
        ("PDWORD", &pdword),
        ("LPFILETIME", &pfiletime),
        ("PFILETIME", &pfiletime),
        ("PFLOAT", &pfloat),
        ("LPHANDLE", &phandle),
        ("PHANDLE", &phandle),
        ("PHKEY", &phandle),
        ("LPHKL", &phandle),
        ("LPINT", &pint),
        ("PINT", &pint),
        ("PLARGE_INTEGER", &plarge_integer),
        ("PLCID", &pdword),
        ("LPLONG", &plong),
        ("PLONG", &plong),
        ("LPMSG", &pmsg),
        ("PMSG", &pmsg),
        ("LPPOINT", &ppoint),
        ("PPOINT", &ppoint),
        ("PPOINTL", &ppoint),
        ("LPRECT", &prect),
        ("PRECT", &prect),
        ("LPRECTL", &prect),
        ("PRECTL", &prect),
        ("LPSC_HANDLE", &phandle),
        ("PSHORT", &pshort),
        ("LPSIZE", &psize),
        ("PSIZE", &psize),
        ("LPSIZEL", &psize),
        ("PSIZEL", &psize),
        ("PSMALL_RECT", &psmall_rect),
        ("LPUINT", &puint),
        ("PUINT", &puint),
        ("PULARGE_INTEGER", &pularge_integer),
        ("PULONG", &pulong),
        ("PUSHORT", &pushort),
        ("PWCHAR", &pwchar),
        ("LPWIN32_FIND_DATAA", &pfind_data_a),
        ("PWIN32_FIND_DATAA", &pfind_data_a),
        ("LPWIN32_FIND_DATAW", &pfind_data_w),
        ("PWIN32_FIND_DATAW", &pfind_data_w),
        ("LPWORD", &pword),
        ("PWORD", &pword),
    ];

    for &(name, cls) in type_bindings {
        m.insert_attr(name.to_string(), Value::py_class(Rc::clone(cls)));
    }
    Ok(())
}

// `multiprocessing.spawn` — 100% native-Rust Python 3.12 `'spawn'` child
// bootstrapper and configuration helpers (`freeze_support`, `set_executable`,
// `get_executable`, `get_preparation_data`, `get_command_line`,
// `import_main_path`, `prepare`, `spawn_main`, `_main`) (issue #3264).

use std::cell::RefCell;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::mp_rt::{
    call_pos, get_executable_path, get_start_method_impl, set_executable_path,
    set_start_method_impl,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyPath, PyStr, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

/// Python-level unpickler snippet mapping `__main__` -> `__mp_main__` inside
/// spawned child processes so top-level functions/classes from the parent's
/// `__main__` script unpickle seamlessly without re-executing `if __name__ ==
/// '__main__':`.
const WORKER_LOADS_SOURCE: &str = r#"
import io, pickle, sys

class _MpWorkerUnpickler(pickle.Unpickler):
    def find_class(self, module, name):
        if module == "__main__" and "__mp_main__" in sys.modules:
            return getattr(sys.modules["__mp_main__"], name)
        return super().find_class(module, name)

def _mp_worker_loads(data):
    return _MpWorkerUnpickler(io.BytesIO(data)).load()
"#;

fn read_stdin_frame(interp: &mut Interpreter) -> Result<Option<Vec<u8>>> {
    let res: std::io::Result<Option<Vec<u8>>> = interp.release_gil_during(|| {
        let stdin = std::io::stdin();
        let mut lock = stdin.lock();
        let mut hdr = [0u8; 4];
        match lock.read_exact(&mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let len = u32::from_be_bytes(hdr) as usize;
        let mut buf = vec![0u8; len];
        if len > 0 {
            lock.read_exact(&mut buf)?;
        }
        Ok(Some(buf))
    });
    res.map_err(|e| PyError::from_io_error(&e, None))
}

fn ensure_worker_loads_fn(interp: &mut Interpreter) -> Result<Value> {
    let spawn_mod = interp.load_module("multiprocessing.spawn")?;
    if let Ok(existing) = interp.get_attr(&spawn_mod, "_cached_worker_loads")
        && !existing.is_none()
    {
        return Ok(existing);
    }
    let ns = Value::dict(PyDict::default());
    ns.dict_insert(
        PyKey::str_from("__name__"),
        Value::string("multiprocessing.spawn"),
    )?;
    interp.exec_source(WORKER_LOADS_SOURCE, Some(ns.clone()), None)?;
    let fn_val = ns
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from("_mp_worker_loads")).cloned())
        .ok_or_else(|| PyError::named("RuntimeError", "failed to create _mp_worker_loads"))?;
    if let ValueKind::PyModule(m_rc) = spawn_mod.kind() {
        m_rc.borrow_mut()
            .insert_attr("_cached_worker_loads".to_string(), fn_val.clone());
    }
    Ok(fn_val)
}

fn import_main_path_impl(interp: &mut Interpreter, main_path: &str) -> Result<()> {
    if main_path.is_empty() || !std::path::Path::new(main_path).is_file() {
        return Ok(());
    }
    let src = std::fs::read_to_string(main_path)
        .map_err(|e| PyError::from_io_error(&e, Some(main_path)))?;
    let types_mod = interp.load_module("types")?;
    let mod_type = interp.get_attr(&types_mod, "ModuleType")?;
    let mp_main = call_pos(interp, mod_type, &[Value::string("__mp_main__")])?;
    let _ = interp.assign_attr(mp_main.clone(), "__file__", Value::string(main_path));

    let sys_mod = interp.load_module("sys")?;
    if let Ok(modules) = interp.get_attr(&sys_mod, "modules") {
        let _ = modules.dict_insert(PyKey::str_from("__mp_main__"), mp_main.clone());
    }
    let mod_dict = interp.get_attr(&mp_main, "__dict__")?;
    interp.exec_source(&src, Some(mod_dict), None)?;
    Ok(())
}

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    _module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("_main"),
            Value::string("freeze_support"),
            Value::string("set_executable"),
            Value::string("get_executable"),
            Value::string("get_preparation_data"),
            Value::string("get_command_line"),
            Value::string("import_main_path"),
        ]),
        "WINEXE" => Value::bool_(false),
        "WINSERVICE" => Value::bool_(false),
        "old_main_modules" => Value::list(Vec::new()),
    }

    fn freeze_support() -> Result<Value> {
        Ok(Value::none())
    }

    fn set_executable(#[positional_only] exe: PyPath) -> Result<Value> {
        set_executable_path(exe.into_path_buf());
        Ok(Value::none())
    }

    fn get_executable() -> Result<Value> {
        let p = get_executable_path()?;
        Ok(Value::string(p.to_string_lossy().as_ref()))
    }

    fn get_start_method(#[default(false.into())] allow_none: PyBool) -> Result<Value> {
        Ok(get_start_method_impl(allow_none.0)
            .map(|s| Value::string(&s))
            .unwrap_or_else(Value::none))
    }

    fn set_start_method(
        #[positional_only] method: PyStr<'_>,
        #[default(false.into())] force: PyBool,
    ) -> Result<Value> {
        set_start_method_impl(&method.0, force.0)?;
        Ok(Value::none())
    }

    fn is_forking(#[positional_only] argv: PyValueRef<'_>) -> Result<Value> {
        let items = _interp.collect_iterable(argv.0)?;
        let is_f = items.len() >= 2
            && items[1]
                .as_str()
                .is_some_and(|s| s == "--multiprocessing-fork");
        Ok(Value::bool_(is_f))
    }

    fn get_command_line(#[varkwargs] kwds: indexmap::IndexMap<String, Value>) -> Result<Value> {
        let exe = get_executable_path()?;
        let mut parts = Vec::new();
        for (k, v) in kwds {
            parts.push(format!("{k}={}", v.repr_raw()));
        }
        let prog = format!(
            "from multiprocessing.spawn import spawn_main; spawn_main({})",
            parts.join(", ")
        );
        Ok(Value::list(vec![
            Value::string(exe.to_string_lossy().as_ref()),
            Value::string("-c"),
            Value::string(&prog),
        ]))
    }

    fn get_preparation_data(#[positional_only] name: PyStr<'_>) -> Result<Value> {
        let d = Value::dict(PyDict::default());
        d.dict_insert(PyKey::str_from("name"), Value::string(name.0))?;
        let sys_mod = _interp.load_module("sys")?;
        if let Ok(path) = _interp.get_attr(&sys_mod, "path") {
            d.dict_insert(PyKey::str_from("sys_path"), path)?;
        }
        if let Ok(argv) = _interp.get_attr(&sys_mod, "argv") {
            d.dict_insert(PyKey::str_from("sys_argv"), argv)?;
        }
        if let Ok(cwd) = std::env::current_dir() {
            d.dict_insert(
                PyKey::str_from("dir"),
                Value::string(cwd.to_string_lossy().as_ref()),
            )?;
        }
        if let Some(sm) = get_start_method_impl(true) {
            d.dict_insert(PyKey::str_from("start_method"), Value::string(&sm))?;
        }
        Ok(d)
    }

    fn import_main_path(#[positional_only] main_path: PyStr<'_>) -> Result<Value> {
        import_main_path_impl(_interp, &main_path.0)?;
        Ok(Value::none())
    }

    fn prepare(#[positional_only] data: PyValueRef<'_>) -> Result<Value> {
        if let Some(d) = data.0.as_dict()
            && let Some(main_path) = d.get(&PyKey::str_from("init_main_from_path"))
            && let Some(s) = main_path.as_str()
        {
            import_main_path_impl(_interp, s)?;
        }
        Ok(Value::none())
    }

    fn _worker_loads(#[positional_only] data: PyBytesLike<'_>) -> Result<Value> {
        let loads_fn = ensure_worker_loads_fn(_interp)?;
        call_pos(_interp, loads_fn, &[Value::bytes(data.as_bytes().to_vec())])
    }

    fn _run_spawn_child() -> Result<Value> {
        let Some(frame) = read_stdin_frame(_interp)? else {
            std::process::exit(0);
        };
        let pickle_mod = _interp.load_module("pickle")?;
        let loads_fn = _interp.get_attr(&pickle_mod, "loads")?;
        let envelope = call_pos(_interp, loads_fn, &[Value::bytes(frame)])?;
        let ValueKind::Tuple(items) = envelope.kind() else {
            std::process::exit(1);
        };
        if items.len() != 5 {
            std::process::exit(1);
        }
        let sys_path_items = _interp.collect_iterable(&items[0]).unwrap_or_default();
        let main_file = items[1].as_str().map(str::to_owned);
        let cwd_opt = items[2].as_str().map(PathBuf::from);
        let parent_pid = items[3].as_int().unwrap_or(0);
        let proc_bytes = match items[4].kind() {
            ValueKind::Bytes(b) => b.as_slice().to_vec(),
            _ => std::process::exit(1),
        };

        if let Some(cwd) = cwd_opt {
            let _ = std::env::set_current_dir(cwd);
        }
        let sys_mod = _interp.load_module("sys")?;
        if let Ok(sp) = _interp.get_attr(&sys_mod, "path") {
            let existing: Vec<String> = _interp
                .collect_iterable(&sp)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            for p_val in sys_path_items.into_iter().rev() {
                if let Some(ps) = p_val.as_str()
                    && !existing.iter().any(|e| e == ps)
                    && let Ok(ins_fn) = _interp.get_attr(&sp, "insert")
                {
                    let _ = call_pos(_interp, ins_fn, &[Value::int(0), p_val]);
                }
            }
        }

        if let Some(ref mf) = main_file {
            import_main_path_impl(_interp, mf)?;
        }

        let worker_loads = ensure_worker_loads_fn(_interp)?;
        let proc_obj = call_pos(_interp, worker_loads, &[Value::bytes(proc_bytes)])?;
        super::multiprocessing_process::install_child_process_context(
            _interp,
            &proc_obj,
            parent_pid,
        )?;
        let boot_fn = _interp.get_attr(&proc_obj, "_bootstrap")?;
        let code_val = call_pos(_interp, boot_fn, &[])?;
        let code = code_val.as_int().unwrap_or(0) as i32;
        std::process::exit(code);
    }

    fn spawn_main(
        #[default(OptionalArg::Omitted)] _pipe_handle: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] _parent_pid: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] _tracker_fd: OptValueRef<'_>,
    ) -> Result<Value> {
        Ok(Value::none())
    }

    fn _main(
        #[positional_only] _fd: PyIndex,
        #[default(OptionalArg::Omitted)] _parent_sentinel: OptValueRef<'_>,
    ) -> Result<Value> {
        Ok(Value::none())
    }
}

// `multiprocessing.reduction` — 100% native-Rust Python 3.12 cross-process
// pickling/reduction utilities (`ForkingPickler`, `register`, `dump`,
// `send_handle`, `recv_handle`, `DupHandle`, `duplicate`, `steal_handle`,
// `AbstractReducer`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::mp_rt::call_pos;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyBool, PyIndex, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let pickle = interp.load_module("pickle")?;
    let pickler = interp.get_attr(&pickle, "Pickler")?;
    if let Some(fp_val) = module.borrow().get_attr_value("ForkingPickler")
        && let crate::value::ValueKind::PyClass(fp_rc) = fp_val.kind()
        && let crate::value::ValueKind::PyClass(base_rc) = pickler.kind()
    {
        crate::builtin_modules::macros::link_class_bases(fp_rc, vec![Rc::clone(base_rc)]);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("send_handle"),
            Value::string("recv_handle"),
            Value::string("ForkingPickler"),
            Value::string("register"),
            Value::string("dump"),
            Value::string("DupHandle"),
            Value::string("duplicate"),
            Value::string("steal_handle"),
        ]),
        "HAVE_SEND_HANDLE" => Value::bool_(true),
    }

    class ForkingPickler {
        #[classmethod]
        fn register(
            #[positional_only] _cls: PyValueRef<'_>,
            #[positional_only] type_obj: PyValueRef<'_>,
            #[positional_only] reduce: PyValueRef<'_>,
        ) -> Result<Value> {
            let copyreg = _interp.load_module("copyreg")?;
            let pickle_fn = _interp.get_attr(&copyreg, "pickle")?;
            call_pos(_interp, pickle_fn, &[type_obj.0.clone(), reduce.0.clone()])
        }

        #[classmethod]
        fn dumps(
            #[positional_only] _cls: PyValueRef<'_>,
            #[positional_only] obj: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
        ) -> Result<Value> {
            let pickle = _interp.load_module("pickle")?;
            let dumps_fn = _interp.get_attr(&pickle, "dumps")?;
            match protocol.as_value() {
                Some(p) if !p.is_none() => call_pos(_interp, dumps_fn, &[obj.0.clone(), p.clone()]),
                _ => call_pos(_interp, dumps_fn, std::slice::from_ref(obj.0)),
            }
        }

        #[staticmethod]
        fn loads(#[positional_only] data: PyValueRef<'_>) -> Result<Value> {
            let pickle = _interp.load_module("pickle")?;
            let loads_fn = _interp.get_attr(&pickle, "loads")?;
            call_pos(_interp, loads_fn, std::slice::from_ref(data.0))
        }
    }

    fn register(
        #[positional_only] type_obj: PyValueRef<'_>,
        #[positional_only] reduce: PyValueRef<'_>,
    ) -> Result<Value> {
        let copyreg = _interp.load_module("copyreg")?;
        let pickle_fn = _interp.get_attr(&copyreg, "pickle")?;
        call_pos(_interp, pickle_fn, &[type_obj.0.clone(), reduce.0.clone()])
    }

    fn dump(
        #[positional_only] obj: PyValueRef<'_>,
        #[positional_only] file: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] protocol: OptValueRef<'_>,
    ) -> Result<Value> {
        let pickle = _interp.load_module("pickle")?;
        let dump_fn = _interp.get_attr(&pickle, "dump")?;
        match protocol.as_value() {
            Some(p) if !p.is_none() => {
                call_pos(_interp, dump_fn, &[obj.0.clone(), file.0.clone(), p.clone()])
            }
            _ => call_pos(_interp, dump_fn, &[obj.0.clone(), file.0.clone()]),
        }
    }

    fn send_handle(
        #[positional_only] conn: PyValueRef<'_>,
        #[positional_only] handle: PyIndex,
        #[positional_only] _destination_pid: PyIndex,
    ) -> Result<Value> {
        let send_fn = _interp.get_attr(conn.0, "send")?;
        call_pos(
            _interp,
            send_fn,
            &[Value::int(handle.expect_i64(FN_NAME, "handle")?)],
        )
    }

    fn recv_handle(#[positional_only] conn: PyValueRef<'_>) -> Result<Value> {
        let recv_fn = _interp.get_attr(conn.0, "recv")?;
        call_pos(_interp, recv_fn, &[])
    }

    fn duplicate(
        #[positional_only] handle: PyIndex,
        #[default(OptionalArg::Omitted)] _target_process: OptValueRef<'_>,
        #[default(false.into())] _inheritable: PyBool,
    ) -> Result<Value> {
        Ok(Value::int(handle.expect_i64(FN_NAME, "handle")?))
    }

    fn steal_handle(
        #[positional_only] _source_pid: PyIndex,
        #[positional_only] handle: PyIndex,
    ) -> Result<Value> {
        Ok(Value::int(handle.expect_i64(FN_NAME, "handle")?))
    }

    class DupHandle {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] handle: PyIndex,
            #[positional_only] _access: PyIndex,
            #[default(OptionalArg::Omitted)] _pid: OptValueRef<'_>,
        ) -> Result<Value> {
            let h = handle.expect_i64(FN_NAME, "handle")?;
            _interp.assign_attr(self_val.0.clone(), "_handle", Value::int(h))?;
            Ok(Value::none())
        }

        fn detach(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp
                .get_attr(self_val.0, "_handle")
                .map_err(|_| PyError::named("ValueError", "handle already detached"))
        }
    }

    class AbstractReducer {
        fn __init__(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }
}

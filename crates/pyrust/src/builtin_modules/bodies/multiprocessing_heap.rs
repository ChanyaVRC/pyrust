// `multiprocessing.heap` — 100% native-Rust Python 3.12 shared-memory heap
// abstractions (`Arena`, `BufferWrapper`, `Heap`) (issue #3264).

use std::sync::Arc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::mp_rt::shm::OsSharedMem;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyIndex, PyValueRef};
use crate::value::Value;

struct ArenaState {
    shm: Arc<OsSharedMem>,
    size: usize,
}

struct BufferWrapperState {
    shm: Arc<OsSharedMem>,
    size: usize,
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![Value::string("BufferWrapper")]),
    }

    #[native_state(ArenaState)]
    class Arena {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] size: PyIndex,
            #[default((-1_i64).into())] _fd: PyIndex,
        ) -> Result<Value> {
            let sz = size.expect_i64(FN_NAME, "size")?;
            if sz <= 0 {
                return Err(PyError::named("ValueError", "Arena size must be positive"));
            }
            let shm = OsSharedMem::open(None, true, sz as usize, true)?;
            set_state(
                self_val.0,
                ArenaState {
                    shm,
                    size: sz as usize,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn size(#[state] st: &ArenaState) -> Result<Value> {
            Ok(Value::int(st.size as i64))
        }

        #[getter]
        fn name(#[state] st: &ArenaState) -> Result<Value> {
            Ok(Value::string(&st.shm.name))
        }
    }

    #[native_state(BufferWrapperState)]
    class BufferWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] size: PyIndex,
        ) -> Result<Value> {
            let sz = size.expect_i64(FN_NAME, "size")?;
            if sz < 0 {
                return Err(PyError::named("ValueError", "BufferWrapper size must be >= 0"));
            }
            let shm = OsSharedMem::open(None, true, (sz as usize).max(1), true)?;
            set_state(
                self_val.0,
                BufferWrapperState {
                    shm,
                    size: sz as usize,
                },
            )?;
            Ok(Value::none())
        }

        fn create_memoryview(#[state] st: &BufferWrapperState) -> Result<Value> {
            let mut buf = vec![0u8; st.size];
            if st.size > 0 {
                st.shm.read_bytes(0, &mut buf)?;
            }
            Ok(Value::bytes(buf))
        }

        fn get_size(#[state] st: &BufferWrapperState) -> Result<Value> {
            Ok(Value::int(st.size as i64))
        }
    }

    class Heap {
        fn __init__(
            #[positional_only] _self_val: PyValueRef<'_>,
            #[default(4096i64.into())] _size: PyIndex,
        ) -> Result<Value> {
            Ok(Value::none())
        }
    }
}

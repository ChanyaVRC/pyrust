// `itertools` registration facade — included into `pub mod itertools { … }`
// declared by `pyrust_builtin_modules!` in `builtin_modules/mod.rs`.
//
// Python-visible classes and callables stay declared here while private
// submodules implement focused responsibilities:
//
// - `helpers`: shared self/attribute/arity helpers and binary iterator init;
// - `infinite`: `count`, `repeat`, `cycle`;
// - `slicing`: `chain`, `islice`, `tee`, `pairwise`, `batched`;
// - `grouping`: `groupby`, `_grouper`, and shared lookahead cursor helpers;
// - `reducers`: `takewhile`, `dropwhile`, `starmap`, `accumulate`, `compress`,
//   `zip_longest`, `filterfalse`;
// - `combinatorics` / `combinatoric_cursors`: `product`, `combinations`,
//   `combinations_with_replacement`, `permutations`;
// - `native_iterators`: low-level native iterator frames (`chain.from_iterable`, `tee`);
// - `class_registry.inc.rs`: module finalization and generation-local identities.
//
// Reference: <https://docs.python.org/3/library/itertools.html>

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};
use pyrust_derive::pyrust_module;

#[path = "itertools/combinatoric_cursors.rs"]
mod combinatoric_cursors;
#[path = "itertools/combinatorics.rs"]
mod combinatorics;
#[path = "itertools/grouping.rs"]
mod grouping;
#[path = "itertools/helpers.rs"]
mod helpers;
#[path = "itertools/infinite.rs"]
mod infinite;
#[path = "itertools/native_iterators.rs"]
pub(crate) mod native_iterators;
#[path = "itertools/reducers.rs"]
mod reducers;
#[path = "itertools/slicing.rs"]
mod slicing;

use helpers::{internal, is_stop_iteration};

pyrust_module! {
    class chain {
        iter_self;
        fn __init__(args) -> Result<Value> {
            slicing::chain_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            slicing::chain_next(_interp, args, FN_NAME)
        }

        fn from_iterable(args) -> Result<Value> {
            slicing::chain_from_iterable(_interp, args)
        }
    }

    class islice {
        iter_self;
        fn __init__(args) -> Result<Value> {
            slicing::islice_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            slicing::islice_next(_interp, args, FN_NAME)
        }
    }

    class count {
        iter_self;
        fn __init__(args) -> Result<Value> {
            infinite::count_init(args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            infinite::count_next(_interp, args, FN_NAME)
        }

        fn __repr__(args) -> Result<Value> {
            infinite::count_repr(args, FN_NAME)
        }
    }

    class repeat {
        iter_self;
        fn __init__(args) -> Result<Value> {
            infinite::repeat_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            infinite::repeat_next(args, FN_NAME)
        }

        fn __repr__(args) -> Result<Value> {
            infinite::repeat_repr(_interp, args, FN_NAME)
        }
    }

    class cycle {
        iter_self;
        fn __init__(args) -> Result<Value> {
            infinite::cycle_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            infinite::cycle_next(_interp, args, FN_NAME)
        }
    }

    class takewhile {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::takewhile_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::takewhile_next(_interp, args, FN_NAME)
        }
    }

    class dropwhile {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::dropwhile_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::dropwhile_next(_interp, args, FN_NAME)
        }
    }

    class starmap {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::starmap_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::starmap_next(_interp, args, FN_NAME)
        }
    }

    class accumulate {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::accumulate_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::accumulate_next(_interp, args, FN_NAME)
        }
    }

    class product {
        iter_self;
        fn __init__(args) -> Result<Value> {
            combinatorics::product_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            combinatorics::product_next(args, FN_NAME)
        }
    }

    class combinations {
        iter_self;
        fn __init__(args) -> Result<Value> {
            combinatorics::init_combo_state(_interp, args, "combinations", false)
        }

        fn __next__(args) -> Result<Value> {
            combinatorics::advance_combinations(args, "combinations", false)
        }
    }

    class combinations_with_replacement {
        iter_self;
        fn __init__(args) -> Result<Value> {
            combinatorics::init_combo_state(_interp, args, "combinations_with_replacement", true)
        }

        fn __next__(args) -> Result<Value> {
            combinatorics::advance_combinations(args, "combinations_with_replacement", true)
        }
    }

    class permutations {
        iter_self;
        fn __init__(args) -> Result<Value> {
            combinatorics::permutations_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            combinatorics::permutations_next(args, FN_NAME)
        }
    }

    class groupby {
        iter_self;
        fn __init__(args) -> Result<Value> {
            grouping::groupby_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            grouping::groupby_next(_interp, args, FN_NAME)
        }
    }

    class _grouper {
        iter_self;
        fn __next__(args) -> Result<Value> {
            grouping::grouper_next(_interp, args, FN_NAME)
        }
    }

    class compress {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::compress_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::compress_next(_interp, args, FN_NAME)
        }
    }

    class zip_longest {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::zip_longest_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::zip_longest_next(_interp, args, FN_NAME)
        }
    }

    class filterfalse {
        iter_self;
        fn __init__(args) -> Result<Value> {
            reducers::filterfalse_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            reducers::filterfalse_next(_interp, args, FN_NAME)
        }
    }

    fn tee(args) -> Result<Value> {
        slicing::tee(_interp, args)
    }

    class pairwise {
        iter_self;
        fn __init__(args) -> Result<Value> {
            slicing::pairwise_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            slicing::pairwise_next(_interp, args, FN_NAME)
        }
    }

    class batched {
        iter_self;
        fn __init__(args) -> Result<Value> {
            slicing::batched_init(_interp, args, FN_NAME)
        }

        fn __next__(args) -> Result<Value> {
            slicing::batched_next(_interp, args, FN_NAME)
        }
    }
}

include!("itertools/class_registry.inc.rs");

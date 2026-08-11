// Environment composition facade.
//
// Responsibilities stay separated below while sharing this module's imports
// and visibility boundary:
// - root_namespace: root identity, cache generations, namespace mirrors, aliases;
// - env_values: compact name-keyed binding storage and shared name-set type;
// - lexical_scope: Environment construction, lookup, mutation, and provider API;
// - tests: invariants spanning those responsibilities.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use indexmap::IndexMap;
use rustc_hash::FxBuildHasher;

use crate::object_model::{ModuleMutationState, PyClass, PyDict, StrKey, Value};

// Include order preserves the original declaration order and private access
// between the four mechanically extracted sections.
include!("environment/root_namespace.rs");
include!("environment/env_values.rs");
include!("environment/lexical_scope.rs");
include!("environment/tests.rs");

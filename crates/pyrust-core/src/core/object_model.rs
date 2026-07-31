//! Cohesive Python value/object representation.
//!
//! `Value`, its NaN-box storage, containers, keys, user functions and classes
//! are deliberately kept in one module: their representation-level helpers
//! are mutually recursive and splitting them would expose unsafe layout
//! details as a pseudo-public API.  Dependencies on the surrounding domains
//! are explicit here.

use std::alloc::{Layout, alloc, dealloc, realloc};
use std::any::Any;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use indexmap::{IndexMap, IndexSet};
use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive, Zero};
use rustc_hash::FxBuildHasher;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

use crate::cycle_guards::{EqGuard, ReprGuard};
use crate::environment::{EnvRef, Environment, NameSet};
use crate::errors::{PyError, Result};
use crate::object_identity::{EncodedObjectIdentity, ObjectIdentity, next_obj_id};

include!("prelude.rs");
include!("int_string_limits.rs");
include!("keys.rs");
include!("functions.rs");
include!("classes.rs");
include!("instance_attrs.rs");
include!("nanbox_strings.rs");
include!("nanbox_pointers.rs");
include!("builtin_types.rs");
include!("generator_cell.rs");
include!("containers.rs");
include!("value_model.rs");
include!("value_constructors.rs");
include!("value_access.rs");
include!("value_containers.rs");
include!("value_conversion.rs");
include!("value_lifecycle.rs");
include!("value_equality.rs");
include!("value_helpers.rs");
include!("weak_value_cache.rs");
include!("tests.rs");

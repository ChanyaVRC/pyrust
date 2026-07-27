//! Typed identity and iteration policy for `collections.OrderedDict`.
//!
//! The standard-library provider registers each import generation here. The
//! interpreter consumes only the typed identity/policy surface, never mutable
//! Python presentation metadata or an exact class name.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pyrust_core::{PyClass, PyError, Value};

thread_local! {
    static CLASSES: RefCell<Vec<Weak<RefCell<PyClass>>>> =
        const { RefCell::new(Vec::new()) };
    static CLEAR_SEQUENCE: Cell<u64> = const { Cell::new(1) };
    static CLEAR_REGISTRY: RefCell<HashMap<i64, ClearMark>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone, Copy)]
struct ClearMark {
    sequence: u64,
    previous_len: usize,
}

/// Immutable policy selected by canonical class/view identity.
#[derive(Clone, Copy)]
pub struct IterationPolicy {
    pub iterator_type_name: &'static str,
    pub mutation_message: &'static str,
    pub exhaust_first: bool,
}

const POLICY: IterationPolicy = IterationPolicy {
    iterator_type_name: "odict_iterator",
    mutation_message: "OrderedDict mutated during iteration",
    exhaust_first: true,
};

/// Register one canonical class generation. Registration is idempotent and
/// weak so removing the owning module does not leak a dead generation.
pub fn register_class(class: &Rc<RefCell<PyClass>>) {
    CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|registered| registered.strong_count() > 0);
        if classes
            .iter()
            .any(|registered| registered.as_ptr() == Rc::as_ptr(class))
        {
            return;
        }
        classes.push(Rc::downgrade(class));
    });
}

pub fn is_class(class: &Rc<RefCell<PyClass>>) -> bool {
    CLASSES.with(|classes| {
        classes
            .borrow()
            .iter()
            .any(|registered| registered.as_ptr() == Rc::as_ptr(class))
    })
}

/// Resolve the canonical owner through a subclass's complete base graph.
pub fn owner(class: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    if is_class(class) {
        return Some(Rc::clone(class));
    }
    let (base, extra_bases) = {
        let borrowed = class.borrow();
        (borrowed.base.clone(), borrowed.extra_bases.clone())
    };
    if let Some(base) = base
        && let Some(owner) = owner(&base)
    {
        return Some(owner);
    }
    extra_bases.iter().find_map(owner)
}

pub fn class_policy(class: &Rc<RefCell<PyClass>>) -> Option<IterationPolicy> {
    owner(class).map(|_| POLICY)
}

/// Native-type assignment/deletion policy for the canonical class itself.
/// Subclasses remain mutable.
pub fn immutable_class_attribute_error(
    class: &Rc<RefCell<PyClass>>,
    attribute: &str,
) -> Option<PyError> {
    is_class(class).then(|| {
        PyError::named(
            "TypeError",
            format!(
                "cannot set '{attribute}' attribute of immutable type 'collections.OrderedDict'"
            ),
        )
    })
}

pub fn view_policy(value: &Value) -> Option<IterationPolicy> {
    super::dict_views::is_ordered_view(value).then_some(POLICY)
}

/// Current clear-event sequence, captured when a guarded iterator is created.
pub fn clear_sequence() -> u64 {
    CLEAR_SEQUENCE.with(Cell::get)
}

/// Record a clear of the canonical mapping's backing dict.
pub fn note_clear(backing_id: i64, previous_len: usize) {
    let sequence = CLEAR_SEQUENCE.with(|counter| {
        let current = counter.get();
        // A wrapped generation could make a post-creation clear look older
        // than a long-lived iterator. Saturation preserves monotonic ordering;
        // equality caches are not involved, so sharing the terminal stamp is
        // sufficient for every later clear to remain observable.
        counter.set(current.saturating_add(1));
        current
    });
    CLEAR_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.len() >= 1024 {
            registry.clear();
        }
        registry.insert(
            backing_id,
            ClearMark {
                sequence,
                previous_len,
            },
        );
    });
}

/// Select the provider-owned guard diagnostic after generic iteration has
/// resolved the backing identity and current length.
pub fn guard_message(
    backing_id: Option<i64>,
    recorded_len: usize,
    current_len: Option<usize>,
    iterator_sequence: u64,
) -> &'static str {
    let cleared = backing_id
        .and_then(|id| CLEAR_REGISTRY.with(|registry| registry.borrow().get(&id).copied()))
        .is_some_and(|mark| {
            mark.sequence >= iterator_sequence
                && mark.previous_len == recorded_len
                && current_len == Some(0)
        });
    if cleared {
        "OrderedDict changed size during iteration"
    } else {
        POLICY.mutation_message
    }
}

#[cfg(test)]
mod tests {
    use super::{CLEAR_REGISTRY, CLEAR_SEQUENCE, clear_sequence, guard_message, note_clear};

    #[test]
    fn clear_sequence_saturates_without_hiding_later_mutation() {
        CLEAR_SEQUENCE.with(|sequence| sequence.set(u64::MAX));
        note_clear(17, 3);

        assert_eq!(clear_sequence(), u64::MAX);
        assert_eq!(
            guard_message(Some(17), 3, Some(0), u64::MAX),
            "OrderedDict changed size during iteration"
        );

        CLEAR_REGISTRY.with(|registry| registry.borrow_mut().remove(&17));
        CLEAR_SEQUENCE.with(|sequence| sequence.set(1));
    }
}

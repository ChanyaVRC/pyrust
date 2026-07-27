//! Interpreter-free identity metadata for standard-library ABC classes.
//!
//! The owning module registers its actual class objects. Runtime protocol
//! helpers can then consume typed roots without importing that module or
//! comparing mutable Python-visible class names.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use pyrust_core::PyClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbcKind {
    Mapping,
}

struct AbcEntry {
    class: Weak<RefCell<PyClass>>,
    kind: AbcKind,
}

thread_local! {
    static ABC_CLASSES: RefCell<Vec<AbcEntry>> = const { RefCell::new(Vec::new()) };
}

pub fn register(class: &Rc<RefCell<PyClass>>, kind: AbcKind) {
    ABC_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        classes.retain(|entry| entry.class.strong_count() > 0);
        if classes.iter().any(|entry| {
            entry.kind == kind
                && entry
                    .class
                    .upgrade()
                    .is_some_and(|registered| Rc::ptr_eq(&registered, class))
        }) {
            return;
        }
        classes.push(AbcEntry {
            class: Rc::downgrade(class),
            kind,
        });
    });
}

/// Test the live identity roots for one ABC role without allocating a
/// per-query snapshot.
pub fn any_live_class(
    kind: AbcKind,
    mut predicate: impl FnMut(&Rc<RefCell<PyClass>>) -> bool,
) -> bool {
    ABC_CLASSES.with(|classes| {
        let mut classes = classes.borrow_mut();
        let result = classes
            .iter()
            .filter(|entry| entry.kind == kind)
            .filter_map(|entry| entry.class.upgrade())
            .any(|class| predicate(&class));
        classes.retain(|entry| entry.class.strong_count() > 0);
        result
    })
}

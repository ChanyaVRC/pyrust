//! Interpreter-free identity registry for classes used as native `typing`
//! construction markers.
//!
//! The `typing` module owns class generation. Runtime adapters only consume
//! this typed identity metadata, so neither generic class construction nor
//! `pyrust-core` needs Python-visible marker names.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pyrust_core::PyClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypingMarkerKind {
    NamedTuple,
    TypedDict,
}

struct MarkerEntry {
    class: Weak<RefCell<PyClass>>,
    kind: TypingMarkerKind,
}

thread_local! {
    static MARKERS: RefCell<HashMap<*const RefCell<PyClass>, MarkerEntry>> =
        RefCell::new(HashMap::new());
}

/// Associate a module-owned marker class with its native construction policy.
///
/// Entries are weak so discarding a re-imported `typing` generation does not
/// keep that module's synthetic classes alive.
pub fn register(class: &Rc<RefCell<PyClass>>, kind: TypingMarkerKind) {
    MARKERS.with(|markers| {
        let mut markers = markers.borrow_mut();
        markers.retain(|_, entry| entry.class.strong_count() > 0);
        markers.insert(
            Rc::as_ptr(class),
            MarkerEntry {
                class: Rc::downgrade(class),
                kind,
            },
        );
    });
}

/// Return native construction metadata for a live marker class.
#[inline]
pub fn classify(class: &Rc<RefCell<PyClass>>) -> Option<TypingMarkerKind> {
    MARKERS.with(|markers| {
        let markers = markers.borrow();
        // Most programs never import `typing`; keep ordinary class calls at an
        // empty-registry length check rather than hashing every class pointer.
        if markers.is_empty() {
            return None;
        }
        let entry = markers.get(&Rc::as_ptr(class))?;
        entry
            .class
            .upgrade()
            .filter(|registered| Rc::ptr_eq(registered, class))
            .map(|_| entry.kind)
    })
}

#[cfg(test)]
mod tests {
    use super::{TypingMarkerKind, classify, register};
    use pyrust_core::PyClass;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn class(name: &str) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass::new(
            name,
            name,
            None,
            Default::default(),
        )))
    }

    #[test]
    fn marker_classification_is_identity_based() {
        let marker = class("NamedTuple");
        let same_named_user_class = class("NamedTuple");
        register(&marker, TypingMarkerKind::NamedTuple);

        assert_eq!(classify(&marker), Some(TypingMarkerKind::NamedTuple));
        assert_eq!(classify(&same_named_user_class), None);
    }

    #[test]
    fn registration_updates_the_typed_role() {
        let marker = class("Marker");
        register(&marker, TypingMarkerKind::NamedTuple);
        register(&marker, TypingMarkerKind::TypedDict);

        assert_eq!(classify(&marker), Some(TypingMarkerKind::TypedDict));
    }
}

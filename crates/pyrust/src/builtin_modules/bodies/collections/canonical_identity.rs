// Weak, generation-aware identities for native collections classes.
//
// Python-visible metadata is mutable, so native dispatch must use class
// identity and inheritance rather than names such as "Counter" or "deque".

/// Stable identities for native `collections` classes whose operations need
/// to accept proper subclasses without trusting mutable Python metadata.
///
/// `collections` can be removed from `sys.modules` and imported again. Keep
/// weak identities for every still-live generation so old instances retain
/// their native semantics without leaking the class objects.
struct CanonicalCollectionClasses {
    counters: Vec<Weak<RefCell<PyClass>>>,
    defaultdicts: Vec<Weak<RefCell<PyClass>>>,
    deques: Vec<Weak<RefCell<PyClass>>>,
    /// `collections` imports `itertools.chain` once per module generation.
    /// Preserve that association so reloading only `itertools` does not
    /// retroactively change the class returned by this generation's
    /// `Counter.elements()`.
    counter_chain_classes: Vec<CounterChainClass>,
}

struct CounterChainClass {
    counter: Weak<RefCell<PyClass>>,
    // A collections generation owns its imported `_chain` provider just as
    // CPython's module global does. The weak Counter key lets the complete
    // association be pruned once that collections generation dies.
    chain: Rc<RefCell<PyClass>>,
}

#[derive(Copy, Clone)]
enum CanonicalCollectionKind {
    Counter,
    DefaultDict,
    Deque,
}

// Thread-local justification:
// (a) Why `thread_local!`: Holds `!Send` `Weak<RefCell<PyClass>>` registries
//     for `collections` classes (`Counter`, `defaultdict`, `deque`) across
//     import generations.
// (b) Multi-interpreter / multi-threaded impact: Weak entries prune dead
//     generations and match by `Rc::ptr_eq`, keeping interpreters on the same
//     thread isolated; separate threads maintain independent registries.
pyrust_core::gil_local! {
    static CANONICAL_COLLECTION_CLASSES: RefCell<CanonicalCollectionClasses> =
        const { RefCell::new(CanonicalCollectionClasses {
            counters: Vec::new(),
            defaultdicts: Vec::new(),
            deques: Vec::new(),
            counter_chain_classes: Vec::new(),
        }) };
}

impl CanonicalCollectionClasses {
    fn classes_mut(&mut self, kind: CanonicalCollectionKind) -> &mut Vec<Weak<RefCell<PyClass>>> {
        match kind {
            CanonicalCollectionKind::Counter => &mut self.counters,
            CanonicalCollectionKind::DefaultDict => &mut self.defaultdicts,
            CanonicalCollectionKind::Deque => &mut self.deques,
        }
    }
}

fn register_canonical_collection_class(
    kind: CanonicalCollectionKind,
    class: &Rc<RefCell<PyClass>>,
) {
    CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        let classes = registry.classes_mut(kind);
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

fn is_canonical_collection_class_or_subclass(
    class: &Rc<RefCell<PyClass>>,
    kind: CanonicalCollectionKind,
) -> bool {
    CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        let classes = registry.classes_mut(kind);
        classes.retain(|registered| registered.strong_count() > 0);
        classes
            .iter()
            .filter_map(Weak::upgrade)
            .any(|canonical| class_is_subclass_of(class, &canonical))
    })
}

/// Whether `class` is one of the exact native `collections.deque` classes.
///
/// `MappingProxyType` mirrors CPython's `PyMapping_Check`: deque's inherited
/// sequence-only `__getitem__` does not provide `mp_subscript`, while a deque
/// subclass that defines its own `__getitem__` does. The caller compares the
/// MRO attribute owner against this typed, reload-aware identity.
pub(crate) fn is_canonical_deque_class(class: &Rc<RefCell<PyClass>>) -> bool {
    CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        let classes = registry.classes_mut(CanonicalCollectionKind::Deque);
        classes.retain(|registered| registered.strong_count() > 0);
        classes
            .iter()
            .any(|canonical| canonical.as_ptr() == Rc::as_ptr(class))
    })
}

/// Whether `class` is a canonical `collections.Counter` class or any subclass
/// thereof (used by `copy.deepcopy` to match CPython's Counter reduction
/// ordering without relying on mutable class names, issue #2968).
pub(crate) fn is_canonical_counter_class_or_subclass(class: &Rc<RefCell<PyClass>>) -> bool {
    is_canonical_collection_class_or_subclass(class, CanonicalCollectionKind::Counter)
}

#[allow(dead_code)]
pub(crate) fn is_canonical_defaultdict_class_or_subclass(class: &Rc<RefCell<PyClass>>) -> bool {
    is_canonical_collection_class_or_subclass(class, CanonicalCollectionKind::DefaultDict)
}

/// Return the canonical base class from a concrete receiver's generation.
///
/// Counter arithmetic and unary normalization are specified to return a base
/// `Counter`, even when invoked on a subclass. A retained old receiver must
/// therefore resolve through its own base chain rather than silently switching
/// to the newest imported `collections` generation.
fn canonical_collection_base_for_receiver(
    receiver: &Rc<RefCell<PyClass>>,
    kind: CanonicalCollectionKind,
) -> Option<Rc<RefCell<PyClass>>> {
    CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        let classes = registry.classes_mut(kind);
        classes.retain(|registered| registered.strong_count() > 0);
        classes.iter().rev().find_map(|registered| {
            let canonical = registered.upgrade()?;
            class_is_subclass_of(receiver, &canonical).then_some(canonical)
        })
    })
}

fn register_counter_chain_class(counter: &Rc<RefCell<PyClass>>, chain: &Rc<RefCell<PyClass>>) {
    CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry
            .counter_chain_classes
            .retain(|entry| entry.counter.strong_count() > 0);
        if registry
            .counter_chain_classes
            .iter()
            .any(|entry| entry.counter.as_ptr() == Rc::as_ptr(counter))
        {
            return;
        }
        registry.counter_chain_classes.push(CounterChainClass {
            counter: Rc::downgrade(counter),
            chain: Rc::clone(chain),
        });
    });
}

/// Resolve the chain class captured by the receiver's Counter generation.
///
/// A subclass inherits its canonical Counter generation's association. If the
/// Counter generation predates the first explicit itertools import, bind it on
/// first use and keep that choice stable across later itertools reloads.
fn counter_elements_chain_class(class: &Rc<RefCell<PyClass>>) -> Option<Rc<RefCell<PyClass>>> {
    let registered = CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry
            .counter_chain_classes
            .retain(|entry| entry.counter.strong_count() > 0);
        registry
            .counter_chain_classes
            .iter()
            .rev()
            .find_map(|entry| {
                let counter = entry.counter.upgrade()?;
                if class_is_subclass_of(class, &counter) {
                    Some(Rc::clone(&entry.chain))
                } else {
                    None
                }
            })
    });
    if registered.is_some() {
        return registered;
    }

    let chain = crate::interpreter::itertools_chain_class()?;
    let counter = CANONICAL_COLLECTION_CLASSES.with(|registry| {
        let mut registry = registry.borrow_mut();
        let classes = registry.classes_mut(CanonicalCollectionKind::Counter);
        classes.retain(|registered| registered.strong_count() > 0);
        classes.iter().rev().find_map(|registered| {
            let counter = registered.upgrade()?;
            class_is_subclass_of(class, &counter).then_some(counter)
        })
    })?;
    register_counter_chain_class(&counter, &chain);
    Some(chain)
}

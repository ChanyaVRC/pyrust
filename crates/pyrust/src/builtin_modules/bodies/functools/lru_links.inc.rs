// Opaque doubly-linked state for bounded LRU caches.

/// One node in the bounded cache's private doubly-linked LRU list.
#[derive(Debug)]
struct LruNode {
    key: PyKey,
    previous: Option<usize>,
    next: Option<usize>,
}

#[derive(Debug, Default)]
struct LruLinks {
    nodes: Vec<Option<LruNode>>,
    free: Vec<usize>,
    head: Option<usize>,
    tail: Option<usize>,
}

impl LruLinks {
    fn insert_mru(&mut self, key: PyKey) -> usize {
        let index = self.free.pop().unwrap_or_else(|| {
            self.nodes.push(None);
            self.nodes.len() - 1
        });
        self.nodes[index] = Some(LruNode {
            key,
            previous: self.tail,
            next: None,
        });
        if let Some(tail) = self.tail
            && let Some(node) = self.nodes[tail].as_mut()
        {
            node.next = Some(index);
        } else {
            self.head = Some(index);
        }
        self.tail = Some(index);
        index
    }

    fn promote(&mut self, index: usize) {
        if self.tail == Some(index) {
            return;
        }
        let Some((previous, next)) = self
            .nodes
            .get(index)
            .and_then(Option::as_ref)
            .map(|node| (node.previous, node.next))
        else {
            // A stale entry can only arise from external mutation of private
            // attrs; tolerate it rather than panic.
            return;
        };
        if let Some(previous) = previous
            && let Some(node) = self.nodes[previous].as_mut()
        {
            node.next = next;
        } else {
            self.head = next;
        }
        if let Some(next) = next
            && let Some(node) = self.nodes[next].as_mut()
        {
            node.previous = previous;
        }
        if let Some(tail) = self.tail
            && let Some(node) = self.nodes[tail].as_mut()
        {
            node.next = Some(index);
        }
        if let Some(node) = self.nodes[index].as_mut() {
            node.previous = self.tail;
            node.next = None;
        }
        self.tail = Some(index);
    }

    fn remove(&mut self, index: usize) -> Option<PyKey> {
        let node = self.nodes.get_mut(index)?.take()?;
        if let Some(previous) = node.previous
            && let Some(previous_node) = self.nodes[previous].as_mut()
        {
            previous_node.next = node.next;
        } else {
            self.head = node.next;
        }
        if let Some(next) = node.next
            && let Some(next_node) = self.nodes[next].as_mut()
        {
            next_node.previous = node.previous;
        } else {
            self.tail = node.previous;
        }
        self.free.push(index);
        Some(node.key)
    }

    fn pop_lru(&mut self) -> Option<PyKey> {
        self.remove(self.head?)
    }

    fn clear(&mut self) {
        self.nodes.clear();
        self.free.clear();
        self.head = None;
        self.tail = None;
    }
}

struct LruLinksOps;
const LRU_LINKS_OPS: &LruLinksOps = &LruLinksOps;

impl BuiltinTypeOps for LruLinksOps {
    fn type_name(&self) -> &'static str {
        "_functools_lru_links"
    }
}

fn lru_links_value() -> Value {
    let state: Box<dyn Any> = Box::new(LruLinks::default());
    Value::builtin_object(LRU_LINKS_OPS, state)
}

fn with_lru_links<R>(
    inst: &Rc<RefCell<PyInstance>>,
    fn_name: &str,
    action: impl FnOnce(&mut LruLinks) -> R,
) -> Result<R> {
    let state_value = inst
        .borrow()
        .attrs
        .get("_links")
        .cloned()
        .ok_or_else(|| internal(fn_name))?;
    let ValueKind::BuiltinObject { ops, state } = state_value.kind() else {
        return Err(internal(fn_name));
    };
    if !pyrust_core::builtin_ops_is::<LruLinksOps>(ops) {
        return Err(internal(fn_name));
    }
    let mut state = state.borrow_mut();
    let links = state
        .downcast_mut::<LruLinks>()
        .ok_or_else(|| internal(fn_name))?;
    Ok(action(links))
}

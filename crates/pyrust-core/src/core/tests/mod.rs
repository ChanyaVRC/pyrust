#[cfg(test)]
mod tests {
    use std::alloc::alloc;
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, HashSet};
    use std::rc::Rc;

    use indexmap::IndexMap;

    use crate::environment::Environment;
    use crate::pool::{drain_free_list, pool_b_len, pool_opaque_len, take_thread_exit_drains};

    use super::{
        ACTIVE_COLLECTION_MUTATION_STATES, BuiltinTypeOps, COLLECTION_MUTATION_STATES,
        CanonicalClassTag, FrozenSetKey, InstanceAttrs, LazyModuleAttrSource, LazyModuleAttrs,
        LazyModuleSlotVisitor, MemberSlotId, ModuleAttrs, NANBOX_HEAP_POINTER_ALIGNMENT,
        PAYLOAD_MASK, PyClass, PyDict, PyInstance, PyKey, PyModule, PySet, PySetProbeSnapshot,
        STR_CODEPOINT_LEN_SHIFT, STR_CODEPOINT_LEN_TAG, STR_MAX_BYTE_LEN, STR_OWNED_HEADER_SIZE,
        STR_RC_MAX, STR_RC_ONE, STR_SLICE_LAYOUT_SIZE, SetProbeEntry, StrKey, StrSegment,
        TAG_LIST_BITS, UserFunction, UserFunctionKind, Value, ValueKind, builtin_type_name,
        cesu8_codepoints, cesu8_prev_codepoint, encode_nanbox_heap_pointer, error_type_name,
        float_bits_as_exact_i64, instance_backing_for_repr, map_str_segments, next_fn_id,
        opaque_layout, push_codepoint, py_hash_nan, py_hash_pykey, range_len, str_codepoint_count,
        str_codepoint_offset, str_codepoint_prefix, str_has_surrogates, str_is_inline_bits,
        str_segments, try_nanbox_heap_pointer_payload, try_nanbox_owned_string_layout,
        uncached_utf8_byte_offset, utf8_codepoint_count,
    };

    include!("nanbox.rs");
    include!("dict_probe.rs");
    include!("dict_probe_journal.rs");
    include!("containers.rs");
    include!("keys.rs");
    include!("strings.rs");
    include!("objects.rs");
}

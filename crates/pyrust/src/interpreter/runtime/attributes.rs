// Python object attribute and descriptor semantics.
//
// Owns lookup, assignment, deletion, descriptor invocation, class/instance
// attribute policy, and member descriptors.

fn typing_object_readonly_attr_error(class: &Rc<RefCell<PyClass>>, name: &str) -> Option<String> {
    if is_typevar_class(class) {
        typevar_readonly_attr_error(name)
    } else if is_type_alias_class(class) {
        type_alias_readonly_attr_error(name)
    } else {
        None
    }
}

fn dict_view_mapping_descriptor_owner(target: &Value, name: &str) -> Option<&'static str> {
    let class = crate::interpreter::primitive_class_for_value(target)?;
    let descriptor = lookup_class_attr(&class, name)?;
    pyrust_builtins::numeric_attrs_descriptor::as_dict_view_mapping_descriptor(&descriptor)
        .map(|info| info.class_name)
}

include!("attributes/attribute_lookup.rs");
include!("attributes/protocol_attributes.rs");
include!("attributes/attribute_cache_policy.rs");
include!("attributes/function_attributes.rs");
include!("attributes/class_attributes.rs");
include!("attributes/instance_attributes.rs");
include!("attributes/attribute_assignment.rs");
include!("attributes/attribute_deletion.rs");
include!("attributes/attribute_support.rs");
include!("attributes/descriptors.rs");
include!("attributes/member_descriptors.rs");
include!("attributes/descriptor_support.rs");

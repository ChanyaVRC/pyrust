// Attribute-domain protocol adapters.
// Attribute lookup adapters for bytecode-level protocol requirements.

impl Interpreter {
    /// The canonical `TypeError` a context-manager / async-iteration opcode
    /// (`GetAttrForWith`) raises when its required protocol attribute cannot be
    /// resolved on `target` — any lookup failure maps to it.  `requirement` is
    /// the opcode's protocol tag.
    #[cold]
    pub(crate) fn protocol_attr_type_error(target: &Value, requirement: u8) -> PyError {
        let type_name = value_type_name_str(target);
        let message = match requirement {
            1 => format!(
                "'{type_name}' object does not support the context manager protocol \
                 (missed __exit__ method)"
            ),
            2 => format!("'async for' requires an object with __aiter__ method, got {type_name}"),
            3 => format!(
                "'{type_name}' object does not support the asynchronous context \
                 manager protocol"
            ),
            4 => format!(
                "'{type_name}' object does not support the asynchronous context \
                 manager protocol (missed __aexit__ method)"
            ),
            5 => format!(
                "'async for' received an object from __aiter__ that does not implement \
                 __anext__: {type_name}"
            ),
            _ => format!("'{type_name}' object does not support the context manager protocol"),
        };
        pyrust_core::type_err!(message)
    }
}

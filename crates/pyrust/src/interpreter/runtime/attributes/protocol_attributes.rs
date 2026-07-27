// Attribute-domain protocol adapters.
// Attribute lookup adapters for bytecode-level protocol requirements.

impl Interpreter {
    /// Resolve an attribute required by a context-manager / async-iteration
    /// opcode, translating any lookup failure to the protocol's canonical
    /// `TypeError`.
    pub(crate) fn get_required_protocol_attr(
        &mut self,
        target: &Value,
        name: &str,
        requirement: u8,
    ) -> Result<Value> {
        match self.get_attr(target, name) {
            Ok(value) => Ok(value),
            Err(_) => {
                let type_name = value_type_name_str(target);
                let message = match requirement {
                    1 => format!(
                        "'{type_name}' object does not support the context manager protocol \
                         (missed __exit__ method)"
                    ),
                    2 => format!(
                        "'async for' requires an object with __aiter__ method, got {type_name}"
                    ),
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
                    _ => format!(
                        "'{type_name}' object does not support the context manager protocol"
                    ),
                };
                Err(pyrust_core::type_err!(message))
            }
        }
    }
}

// Expansion-only specialization for the common tagged-integer unary path. A
// macro is intentional here: the VM expands the successful case before its
// `Result`-returning semantic fallback, while `expressions` consumes the same
// definition when called independently. Protocol dispatch and overflow
// promotion remain canonical expression responsibilities.
macro_rules! try_tagged_int_unary {
    ($value:expr, $op:expr) => {{
        match $op {
            // `not` needs protocol-aware truthiness and is never an integer
            // specialization. Test the operation first so the VM and its
            // fallback do not both decode the same tagged value.
            crate::ast::UnaryOp::Not => None,
            operation => {
                if let Some(integer) = ($value).as_int() {
                    match operation {
                        crate::ast::UnaryOp::Neg => {
                            integer.checked_neg().map(pyrust_core::Value::int)
                        }
                        crate::ast::UnaryOp::BitNot => Some(pyrust_core::Value::int(!integer)),
                        crate::ast::UnaryOp::Pos => Some(pyrust_core::Value::int(integer)),
                        crate::ast::UnaryOp::Not => unreachable!("handled above"),
                    }
                } else {
                    None
                }
            }
        }
    }};
}

pub(super) use try_tagged_int_unary;

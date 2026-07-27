// Iterable unpacking semantics, independent of register placement.

impl Interpreter {
    pub(crate) fn unpack_exact_values(
        &mut self,
        source: &Value,
        expected: usize,
    ) -> Result<Vec<Value>> {
        let values = self.collect_iterable(source)?;
        if values.len() < expected {
            return Err(pyrust_core::value_err!(
                "not enough values to unpack (expected {}, got {})",
                expected,
                values.len()
            ));
        }
        if values.len() > expected {
            return Err(pyrust_core::value_err!(
                "too many values to unpack (expected {})",
                expected
            ));
        }
        Ok(values)
    }

    pub(crate) fn unpack_extended_values(
        &mut self,
        source: &Value,
        before: usize,
        after: usize,
    ) -> Result<Vec<Value>> {
        let mut values = self.collect_iterable(source)?;
        let minimum = before + after;
        if values.len() < minimum {
            return Err(pyrust_core::value_err!(
                "not enough values to unpack (expected at least {}, got {})",
                minimum,
                values.len()
            ));
        }

        let suffix_start = values.len() - after;
        let suffix = values.split_off(suffix_start);
        let middle = values.split_off(before);
        values.push(Value::list(middle));
        values.extend(suffix);
        Ok(values)
    }
}

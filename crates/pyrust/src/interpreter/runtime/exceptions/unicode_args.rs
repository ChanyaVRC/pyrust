impl Interpreter {
    fn resolve_unicode_error_index(&mut self, args: &mut [Value], index: usize) -> Result<()> {
        let resolved = self.value_to_index(&args[index], |value| {
            pyrust_core::type_err!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(value)
            )
        })?;
        args[index] = resolved;
        Ok(())
    }

    pub(super) fn validate_unicode_decode_args(&mut self, args: &mut [Value]) -> Result<()> {
        if args.len() != 5 {
            return Err(pyrust_core::type_err!(
                "function takes exactly 5 arguments ({} given)",
                args.len()
            ));
        }
        if !matches!(args[0].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 1 must be str, not {}",
                pyrust_core::builtin_type_name(&args[0])
            ));
        }
        if !matches!(args[1].kind(), ValueKind::Bytes(_)) {
            return Err(pyrust_core::type_err!(
                "a bytes-like object is required, not '{}'",
                pyrust_core::builtin_type_name(&args[1])
            ));
        }
        for index in [2, 3] {
            self.resolve_unicode_error_index(args, index)?;
        }
        if !matches!(args[4].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 5 must be str, not {}",
                pyrust_core::builtin_type_name(&args[4])
            ));
        }
        Ok(())
    }

    pub(super) fn validate_unicode_encode_args(&mut self, args: &mut [Value]) -> Result<()> {
        if args.len() != 5 {
            return Err(pyrust_core::type_err!(
                "function takes exactly 5 arguments ({} given)",
                args.len()
            ));
        }
        if !matches!(args[0].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 1 must be str, not {}",
                pyrust_core::builtin_type_name(&args[0])
            ));
        }
        if !matches!(args[1].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 2 must be str, not {}",
                pyrust_core::builtin_type_name(&args[1])
            ));
        }
        for index in [2, 3] {
            self.resolve_unicode_error_index(args, index)?;
        }
        if !matches!(args[4].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 5 must be str, not {}",
                pyrust_core::builtin_type_name(&args[4])
            ));
        }
        Ok(())
    }

    pub(super) fn validate_unicode_translate_args(&mut self, args: &mut [Value]) -> Result<()> {
        if args.len() != 4 {
            return Err(pyrust_core::type_err!(
                "function takes exactly 4 arguments ({} given)",
                args.len()
            ));
        }
        if !matches!(args[0].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 1 must be str, not {}",
                pyrust_core::builtin_type_name(&args[0])
            ));
        }
        for index in [1, 2] {
            self.resolve_unicode_error_index(args, index)?;
        }
        if !matches!(args[3].kind(), ValueKind::Str(_)) {
            return Err(pyrust_core::type_err!(
                "argument 4 must be str, not {}",
                pyrust_core::builtin_type_name(&args[3])
            ));
        }
        Ok(())
    }
}

// Callable adapters for runtime objects exported by the `typing` module.
//
// Generic class-call routing must not know that these marker classes delegate
// to Python helpers. Keep the marker identity and helper names at the concrete
// builtin adapter boundary.

impl Interpreter {
    pub(super) fn try_build_builtin_class_adapter(
        &mut self,
        class_name: &str,
        attrs: &IndexMap<String, Value>,
        base: Option<&Rc<RefCell<PyClass>>>,
        extra_bases: &[Rc<RefCell<PyClass>>],
        class_kwargs: &[ExpandedCallArg],
    ) -> Result<Option<Value>> {
        let has_marker = |kind| {
            base.into_iter()
                .chain(extra_bases)
                .any(|class| pyrust_builtins::typing_marker::classify(class) == Some(kind))
        };

        if has_marker(pyrust_builtins::typing_marker::TypingMarkerKind::NamedTuple) {
            return self.build_typing_namedtuple(class_name, attrs).map(Some);
        }
        if has_marker(pyrust_builtins::typing_marker::TypingMarkerKind::TypedDict) {
            let total = class_kwargs
                .iter()
                .find(|arg| arg.name.as_deref() == Some("total"))
                .map(|arg| arg.value.clone());
            return self
                .build_typing_typeddict(class_name, attrs, total)
                .map(Some);
        }
        Ok(None)
    }

    #[inline]
    pub(super) fn try_call_typing_marker(
        &mut self,
        class: &Rc<RefCell<PyClass>>,
        args: &[ExpandedCallArg],
    ) -> Result<Option<Value>> {
        let helper_name = match pyrust_builtins::typing_marker::classify(class) {
            Some(pyrust_builtins::typing_marker::TypingMarkerKind::NamedTuple) => {
                "_namedtuple_functional"
            }
            Some(pyrust_builtins::typing_marker::TypingMarkerKind::TypedDict) => {
                "_typeddict_functional"
            }
            None => return Ok(None),
        };

        let helper = self
            .load_module("typing")
            .ok()
            .and_then(|module| match module.kind() {
                ValueKind::PyModule(module) => module.borrow().attrs.get(helper_name).cloned(),
                _ => None,
            })
            .ok_or_else(|| PyError::Runtime(format!("typing.{helper_name} unavailable")))?;

        self.call_function_expanded(helper, args).map(Some)
    }

    fn build_typing_namedtuple(
        &mut self,
        class_name: &str,
        attrs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let field_names: Vec<String> = attrs
            .get("__annotations__")
            .and_then(|annotations| {
                annotations.as_dict().map(|dict| {
                    dict.keys()
                        .filter_map(|key| match key {
                            PyKey::Str(name) => name.as_str().map(str::to_string),
                            _ => None,
                        })
                        .collect()
                })
            })
            .unwrap_or_default();
        let field_set: std::collections::HashSet<&str> =
            field_names.iter().map(String::as_str).collect();

        let fields = Value::list(field_names.iter().cloned().map(Value::string).collect());
        let defaults = Value::dict(PyDict::default());
        let namespace = Value::dict(PyDict::default());
        for (key, value) in attrs {
            if key == "__annotations__" || key == "__qualname__" {
                continue;
            }
            if field_set.contains(key.as_str()) {
                defaults.dict_insert(PyKey::str_from(key), value.clone())?;
            } else {
                namespace.dict_insert(PyKey::str_from(key), value.clone())?;
            }
        }

        let builder = self.typing_helper("_build_namedtuple_class")?;
        let args = [
            ExpandedCallArg {
                name: None,
                value: Value::string(class_name),
            },
            ExpandedCallArg {
                name: None,
                value: fields,
            },
            ExpandedCallArg {
                name: None,
                value: defaults,
            },
            ExpandedCallArg {
                name: None,
                value: namespace,
            },
        ];
        self.call_function_expanded(builder, &args)
    }

    fn build_typing_typeddict(
        &mut self,
        class_name: &str,
        attrs: &IndexMap<String, Value>,
        total: Option<Value>,
    ) -> Result<Value> {
        let annotations = attrs
            .get("__annotations__")
            .cloned()
            .unwrap_or_else(|| Value::dict(PyDict::default()));
        let builder = self.typing_helper("_build_typeddict_class")?;
        let mut args = vec![
            ExpandedCallArg {
                name: None,
                value: Value::string(class_name),
            },
            ExpandedCallArg {
                name: None,
                value: annotations,
            },
        ];
        if let Some(total) = total {
            args.push(ExpandedCallArg {
                name: Some("total".to_string()),
                value: total,
            });
        }
        self.call_function_expanded(builder, &args)
    }

    fn typing_helper(&mut self, name: &str) -> Result<Value> {
        self.load_module("typing")
            .ok()
            .and_then(|module| match module.kind() {
                ValueKind::PyModule(module) => module.borrow().attrs.get(name).cloned(),
                _ => None,
            })
            .ok_or_else(|| PyError::Runtime(format!("typing.{name} unavailable")))
    }
}

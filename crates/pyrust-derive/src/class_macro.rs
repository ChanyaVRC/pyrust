//! `class ClassName[(Bases) | : base] { … }` parsing and codegen inside `pyrust_module!`.

use std::collections::{HashMap, HashSet};

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::{Attribute, Block, Expr, Ident, LitStr, Token, Type};

use crate::attrs::{
    ArityStyle, ClassAttrs, ConstAttrs, MethodKind, cfg_key, parse_class_attrs, parse_const_attrs,
    peek_contextual_kw,
};
use crate::builtin_fn::{NativeStateCtx, emit_attr_insert_into, emit_fn_artefacts};
use crate::overload::emit_overload_set_artefacts;
use crate::signature::{
    ModuleFn, ModuleFnArgs, group_funcs_by_name, has_state_param, parse_module_fn,
};

/// One constant entry inside a class's `constants { … }` block.
pub(crate) struct ClassConst {
    pub(crate) cfg_attrs: Vec<Attribute>,
    pub(crate) key: LitStr,
    pub(crate) value: Expr,
}

/// A `class ClassName { … }` block inside `pyrust_module!`.
pub(crate) struct ModuleClass {
    /// `#[cfg(...)]` attributes gating the entire class and all its methods.
    pub(crate) cfg_attrs: Vec<Attribute>,
    /// Optional `#[native_state(T)]` Rust state type attached to instances.
    pub(crate) native_state: Option<Type>,
    pub(crate) name: Ident,
    /// Optional base-class expressions (empty => plain `object` subclass).
    pub(crate) bases: Vec<Expr>,
    /// Class-level constants (`constants { "K" => expr, ... }`).
    pub(crate) constants: Vec<ClassConst>,
    /// Regular, `#[classmethod]`, and `#[staticmethod]` methods.
    pub(crate) methods: Vec<ModuleFn>,
    /// `#[getter]` and `#[setter]` property accessors.
    pub(crate) accessors: Vec<ModuleFn>,
    /// `true` when the class body opens with the contextual marker `iter_self;`.
    /// Injects the canonical return-self `__iter__` (`Ok(args[0].value.clone())`).
    pub(crate) iter_self: bool,
}

/// Generated items for one `ModuleClass`.
pub(crate) struct ClassArtefacts {
    pub(crate) fn_items: Vec<TokenStream>,
    pub(crate) reg_entries: Vec<TokenStream>,
    pub(crate) class_item: TokenStream,
}

/// Parse optional class bases:
/// - `class Sub(base1, base2) { … }`
/// - `class Sub : [base1, base2] { … }`
/// - `class Sub : base_expr { … }`
/// - `class Foo { … }` (no bases)
fn parse_class_bases(input: ParseStream) -> syn::Result<Vec<Expr>> {
    if input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in input);
        let bases = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?;
        Ok(bases.into_iter().collect())
    } else if input.peek(Token![:]) {
        let _: Token![:] = input.parse()?;
        if input.peek(syn::token::Bracket) {
            let content;
            syn::bracketed!(content in input);
            let bases = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?;
            Ok(bases.into_iter().collect())
        } else {
            let expr = Expr::parse_without_eager_brace(input)?;
            Ok(vec![expr])
        }
    } else {
        Ok(Vec::new())
    }
}

/// Parse a `constants { [#[cfg(...)]] "KEY" => expr, ... }` block.
pub(crate) fn parse_constants_block(input: ParseStream) -> syn::Result<Vec<ClassConst>> {
    let _: Ident = input.parse()?; // `constants`
    let content;
    syn::braced!(content in input);
    let mut out = Vec::new();
    while !content.is_empty() {
        let raw_attrs = content.call(Attribute::parse_outer)?;
        let ConstAttrs { cfg_attrs } = parse_const_attrs(raw_attrs)?;
        let key: LitStr = content.parse()?;
        let _: Token![=>] = content.parse()?;
        let value: Expr = content.parse()?;
        out.push(ClassConst {
            cfg_attrs,
            key,
            value,
        });
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    if input.peek(Token![,]) {
        let _: Token![,] = input.parse()?;
    }
    Ok(out)
}

/// Parse one `class ClassName[(bases) | : base] { … }` declaration.
/// The `class` ident is at the head of `input`.
pub(crate) fn parse_class(input: ParseStream, attrs: Vec<Attribute>) -> syn::Result<ModuleClass> {
    let ClassAttrs {
        cfg_attrs,
        native_state,
    } = parse_class_attrs(attrs)?;

    let class_kw: Ident = input.parse()?;
    debug_assert_eq!(class_kw.to_string(), "class");
    let name: Ident = input.parse()?;
    let bases = parse_class_bases(input)?;
    let content;
    syn::braced!(content in input);

    let mut iter_self = false;
    let mut constants: Vec<ClassConst> = Vec::new();
    let mut methods: Vec<ModuleFn> = Vec::new();
    let mut accessors: Vec<ModuleFn> = Vec::new();

    // Track property getter/setter uniqueness per (prop_name, cfg_key).
    let mut seen_getters: HashSet<(String, String)> = HashSet::new();
    let mut seen_setters: HashSet<(String, String)> = HashSet::new();
    let mut seen_prop_names: HashSet<(String, String)> = HashSet::new();

    while !content.is_empty() {
        if peek_contextual_kw(&content, "iter_self")? {
            let kw: Ident = content.parse()?;
            let _: Token![;] = content.parse()?;
            if iter_self {
                return Err(syn::Error::new(
                    kw.span(),
                    format!("duplicate `iter_self;` marker in class `{name}`"),
                ));
            }
            iter_self = true;
            continue;
        }
        if peek_contextual_kw(&content, "constants")? {
            constants.extend(parse_constants_block(&content)?);
            continue;
        }

        let method_attrs = content.call(Attribute::parse_outer)?;
        let method = parse_module_fn(&content, method_attrs)?;

        // Validate #[state] / #[state_mut] usage inside this class.
        if let ModuleFnArgs::Typed { params } = &method.args
            && has_state_param(params)
        {
            if native_state.is_none() {
                let sp = params.iter().find(|p| p.is_state_param()).unwrap();
                return Err(syn::Error::new(
                    sp.name.span(),
                    format!(
                        "`#[state]` / `#[state_mut]` requires `#[native_state(MyState)]` on `class {name}`"
                    ),
                ));
            }
            if matches!(
                method.method_kind,
                MethodKind::ClassMethod | MethodKind::StaticMethod
            ) {
                let sp = params.iter().find(|p| p.is_state_param()).unwrap();
                return Err(syn::Error::new(
                    sp.name.span(),
                    "`#[state]` / `#[state_mut]` cannot be used on `#[classmethod]` or `#[staticmethod]`",
                ));
            }
        }

        let m_cfg = cfg_key(&method.cfg_attrs);

        match &method.method_kind {
            MethodKind::Getter(_) => {
                let prop = method.property_name().expect("getter has property name");
                if !seen_getters.insert((prop.clone(), m_cfg.clone())) {
                    return Err(syn::Error::new(
                        method.short_name.span(),
                        format!("duplicate `#[getter]` for property `{prop}` in class `{name}`"),
                    ));
                }
                seen_prop_names.insert((prop, m_cfg));
                accessors.push(method);
            }
            MethodKind::Setter(_) => {
                let prop = method.property_name().expect("setter has property name");
                if !seen_setters.insert((prop.clone(), m_cfg.clone())) {
                    return Err(syn::Error::new(
                        method.short_name.span(),
                        format!("duplicate `#[setter]` for property `{prop}` in class `{name}`"),
                    ));
                }
                seen_prop_names.insert((prop, m_cfg));
                accessors.push(method);
            }
            MethodKind::Regular | MethodKind::ClassMethod | MethodKind::StaticMethod => {
                let (py_name, _) = method.py_and_rust_short_name();
                if iter_self && py_name == "__iter__" {
                    // Exact diagnostic pinned by `tests/compile_fail/iter_self_explicit_iter.rs`.
                    return Err(syn::Error::new(
                        method.short_name.span(),
                        format!(
                            "duplicate method `{py_name}` in class `{name}` \
                             (the second declaration would silently shadow the first)",
                        ),
                    ));
                }
                methods.push(method);
            }
        }
    }

    // Check collisions between property names and regular/class/static methods.
    for method in &methods {
        let (py_name, _) = method.py_and_rust_short_name();
        let m_cfg = cfg_key(&method.cfg_attrs);
        if seen_prop_names.contains(&(py_name.clone(), m_cfg)) {
            return Err(syn::Error::new(
                method.short_name.span(),
                format!(
                    "method `{py_name}` in class `{name}` collides with a `#[getter]`/`#[setter]` property of the same name"
                ),
            ));
        }
    }

    // Validate overload sets on class methods right at parse time so errors
    // surface with accurate spans.
    group_funcs_by_name(&methods)?;

    if methods.is_empty()
        && accessors.is_empty()
        && constants.is_empty()
        && bases.is_empty()
        && !iter_self
    {
        return Err(syn::Error::new(
            name.span(),
            format!(
                "`class {name}` must declare at least one method (`fn __init__(args) {{ … }}`)",
            ),
        ));
    }

    Ok(ModuleClass {
        cfg_attrs,
        native_state,
        name,
        bases,
        constants,
        methods,
        accessors,
        iter_self,
    })
}

/// Synthesize the `ModuleFn` for an `iter_self` class's injected `__iter__` (#1895).
fn synthesized_iter_self(span: Span) -> ModuleFn {
    let args_ident = Ident::new("args", span);
    let body: Block = syn::parse_quote!({ Ok(args[0].value.clone()) });
    ModuleFn {
        attrs: Vec::new(),
        cfg_attrs: Vec::new(),
        short_name: Ident::new("__iter__", span),
        py_name_override: None,
        args: ModuleFnArgs::Legacy { args_ident },
        arity_style: ArityStyle::Standard,
        bare_name: false,
        method_descriptor: true,
        method_kind: MethodKind::Regular,
        method_kind_span: None,
        body,
    }
}

/// Emit method functions, registry entries, and the `PyClass` construction
/// statement for one `ModuleClass`.
pub(crate) fn emit_class_artefacts(class: &ModuleClass) -> ClassArtefacts {
    let class_name_ident = &class.name;
    let class_name_str = class_name_ident.to_string();
    let class_name_lit = LitStr::new(&class_name_str, class_name_ident.span());
    let class_cfg = &class.cfg_attrs;
    let state_ctx = class.native_state.as_ref().map(|ty| NativeStateCtx {
        state_ty: ty,
        class_name: &class_name_str,
    });

    let defines_eq = class
        .methods
        .iter()
        .any(|method| method.py_and_rust_short_name().0 == "__eq__");
    let defines_hash = class
        .methods
        .iter()
        .any(|method| method.py_and_rust_short_name().0 == "__hash__");
    let implicit_hash_none = (defines_eq && !defines_hash).then(|| {
        quote! {
            __class_attrs.insert("__hash__".to_string(), crate::value::Value::none());
        }
    });

    let mut fn_items = Vec::new();
    let mut reg_entries = Vec::new();
    let mut method_attr_inserts = Vec::new();
    let mut classmethod_installs = Vec::new();

    let class_attrs_ident = Ident::new("__class_attrs", Span::call_site());

    // 1. Class methods (with type-based overload grouping!).
    let groups = group_funcs_by_name(&class.methods)
        .expect("class method groups already validated during parse_class");

    for group in &groups {
        let head = group[0];
        let (py_short, rust_short) = head.py_and_rust_short_name();
        let short_lit = LitStr::new(&py_short, head.short_name.span());
        let name_suffix = format!("{class_name_str}.{py_short}");
        let rust_suffix = format!("{class_name_str}__{rust_short}");
        let m_cfg = &head.cfg_attrs;

        let (group_fn_items, reg_entry, name_static_ident) = if group.len() == 1 {
            let (fn_item, reg_entry, name_static_ident) =
                emit_fn_artefacts(head, &name_suffix, &rust_suffix, true, class_cfg, state_ctx);
            (vec![fn_item], reg_entry, name_static_ident)
        } else {
            emit_overload_set_artefacts(
                group,
                &name_suffix,
                &rust_suffix,
                true,
                class_cfg,
                state_ctx,
            )
        };

        fn_items.extend(group_fn_items);
        reg_entries.push(reg_entry);

        match head.method_kind {
            MethodKind::Regular => {
                method_attr_inserts.push(emit_attr_insert_into(
                    &class_attrs_ident,
                    &short_lit,
                    &name_static_ident,
                    m_cfg,
                ));
            }
            MethodKind::StaticMethod => {
                method_attr_inserts.push(quote! {
                    #(#m_cfg)*
                    {
                        let __sm_fn = if FN_PREFIX.is_empty() {
                            crate::value::Value::builtin_function(*#name_static_ident)
                        } else {
                            crate::value::Value::fresh_builtin_function(*#name_static_ident)
                        };
                        __class_attrs.insert(
                            #short_lit.to_string(),
                            pyrust_builtins::classmethod::static_method_any(__sm_fn),
                        );
                    }
                });
            }
            MethodKind::ClassMethod => {
                classmethod_installs.push(quote! {
                    #(#m_cfg)*
                    {
                        let __cm_fn = if FN_PREFIX.is_empty() {
                            crate::value::Value::builtin_function(*#name_static_ident)
                        } else {
                            crate::value::Value::fresh_builtin_function(*#name_static_ident)
                        };
                        let __cm_desc =
                            pyrust_builtins::classmethod::native_class_method_descriptor(
                                __cm_fn,
                                &__cls,
                                #short_lit,
                            );
                        __cls
                            .borrow_mut()
                            .attrs
                            .insert(#short_lit.to_string(), __cm_desc);
                    }
                });
            }
            MethodKind::Getter(_) | MethodKind::Setter(_) => {
                unreachable!("getters and setters are stored in `class.accessors`");
            }
        }
    }

    // 2. Synthesized `iter_self;` `__iter__` method (if requested).
    if class.iter_self {
        let iter_fn = synthesized_iter_self(class_name_ident.span());
        let short_lit = LitStr::new("__iter__", class_name_ident.span());
        let name_suffix = format!("{class_name_str}.__iter__");
        let rust_suffix = format!("{class_name_str}____iter__");
        let (fn_item, reg_entry, name_static_ident) = emit_fn_artefacts(
            &iter_fn,
            &name_suffix,
            &rust_suffix,
            true,
            class_cfg,
            state_ctx,
        );
        fn_items.push(fn_item);
        reg_entries.push(reg_entry);
        method_attr_inserts.push(emit_attr_insert_into(
            &class_attrs_ident,
            &short_lit,
            &name_static_ident,
            &[],
        ));
    }

    // 3. `#[getter]` / `#[setter]` properties grouped by property name in declaration order.
    struct PropEntry<'a> {
        prop_name: String,
        getters: Vec<(&'a [Attribute], Ident)>,
        setters: Vec<(&'a [Attribute], Ident)>,
    }
    let mut props: Vec<PropEntry<'_>> = Vec::new();
    let mut prop_index: HashMap<String, usize> = HashMap::new();

    for acc in &class.accessors {
        let prop_name = acc.property_name().expect("accessor has property name");
        let (_, rust_short) = acc.py_and_rust_short_name();
        let is_getter = matches!(acc.method_kind, MethodKind::Getter(_));
        let (name_suffix, rust_suffix) = if is_getter {
            (
                format!("{class_name_str}.{prop_name}.__get__"),
                format!("{class_name_str}__get_{rust_short}"),
            )
        } else {
            (
                format!("{class_name_str}.{prop_name}.__set__"),
                format!("{class_name_str}__set_{rust_short}"),
            )
        };
        let (fn_item, reg_entry, name_static_ident) =
            emit_fn_artefacts(acc, &name_suffix, &rust_suffix, true, class_cfg, state_ctx);
        fn_items.push(fn_item);
        reg_entries.push(reg_entry);

        let idx = match prop_index.get(&prop_name).copied() {
            Some(i) => i,
            None => {
                let i = props.len();
                prop_index.insert(prop_name.clone(), i);
                props.push(PropEntry {
                    prop_name,
                    getters: Vec::new(),
                    setters: Vec::new(),
                });
                i
            }
        };
        if is_getter {
            props[idx].getters.push((&acc.cfg_attrs, name_static_ident));
        } else {
            props[idx].setters.push((&acc.cfg_attrs, name_static_ident));
        }
    }

    let mut property_attr_inserts = Vec::with_capacity(props.len());
    for prop in props {
        let prop_lit = LitStr::new(&prop.prop_name, class_name_ident.span());
        let getter_assigns: Vec<TokenStream> = prop
            .getters
            .iter()
            .map(|(g_cfg, ident)| {
                quote! {
                    #(#g_cfg)*
                    {
                        __fget = if FN_PREFIX.is_empty() {
                            crate::value::Value::builtin_function(*#ident)
                        } else {
                            crate::value::Value::fresh_builtin_function(*#ident)
                        };
                    }
                }
            })
            .collect();
        let setter_assigns: Vec<TokenStream> = prop
            .setters
            .iter()
            .map(|(s_cfg, ident)| {
                quote! {
                    #(#s_cfg)*
                    {
                        __fset = if FN_PREFIX.is_empty() {
                            crate::value::Value::builtin_function(*#ident)
                        } else {
                            crate::value::Value::fresh_builtin_function(*#ident)
                        };
                    }
                }
            })
            .collect();
        property_attr_inserts.push(quote! {
            {
                #[allow(unused_mut, unused_assignments)]
                let mut __fget = crate::value::Value::none();
                #[allow(unused_mut, unused_assignments)]
                let mut __fset = crate::value::Value::none();
                #(#getter_assigns)*
                #(#setter_assigns)*
                if !__fget.is_none() || !__fset.is_none() {
                    __class_attrs.insert(
                        #prop_lit.to_string(),
                        pyrust_builtins::property::property(
                            __fget,
                            __fset,
                            crate::value::Value::none(),
                        ),
                    );
                }
            }
        });
    }

    // 4. Class-level constants.
    let const_inserts: Vec<TokenStream> = class
        .constants
        .iter()
        .map(|c| {
            let c_cfg = &c.cfg_attrs;
            let k = &c.key;
            let v = &c.value;
            quote! {
                #(#c_cfg)*
                __class_attrs.insert(#k.to_string(), #v);
            }
        })
        .collect();

    // 5. Base class linking.
    let bases_link = if class.bases.is_empty() {
        quote!()
    } else {
        let base_exprs = &class.bases;
        quote! {
            let __bases: Vec<Rc<RefCell<crate::value::PyClass>>> = vec![
                #(
                    crate::builtin_modules::macros::IntoClassBase::into_class_base(
                        #base_exprs,
                        &attrs,
                    )
                ),*
            ];
            crate::builtin_modules::macros::link_class_bases(&__cls, __bases);
        }
    };

    let class_item = quote! {
        #(#class_cfg)*
        {
            let __cls = {
                use std::cell::RefCell;
                use std::rc::Rc;
                use indexmap::IndexMap;
                let mut __class_attrs: IndexMap<String, crate::value::Value> = IndexMap::new();
                __class_attrs.insert(
                    "__module__".to_string(),
                    crate::value::Value::string(MODULE_NAME),
                );
                #(#const_inserts)*
                #(#method_attr_inserts)*
                #(#property_attr_inserts)*
                // Match class-statement finalization: defining equality without
                // an explicit hash makes every macro-generated class unhashable.
                #implicit_hash_none
                let __cls = Rc::new(RefCell::new(crate::value::PyClass::new(
                    #class_name_lit,
                    #class_name_lit,
                    None,
                    __class_attrs,
                )));
                #(#classmethod_installs)*
                #bases_link
                __cls
            };
            attrs.insert(
                #class_name_lit.to_string(),
                crate::value::Value::py_class(__cls),
            );
        }
    };

    ClassArtefacts {
        fn_items,
        reg_entries,
        class_item,
    }
}

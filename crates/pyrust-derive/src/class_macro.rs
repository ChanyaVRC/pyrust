//! `class ClassName { fn … }` parsing and codegen inside `pyrust_module!`.

use std::collections::HashSet;

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::ParseStream;
use syn::{Attribute, Block, Ident, LitStr, Token};

use crate::attrs::{ArityStyle, peek_contextual_kw, validate_class_attrs};
use crate::builtin_fn::{emit_attr_insert, emit_fn_artefacts};
use crate::signature::{ModuleFn, ModuleFnArgs, parse_module_fn};

/// A `class ClassName { fn … }` block inside `pyrust_module!`.
///
/// Each method generates a separate registry entry under the qualified
/// name `<FN_PREFIX><ClassName>.<method>` (e.g.
/// `"collections.Counter.__init__"`).  At `module()` construction time,
/// the class is built as a real `PyClass` with the methods bound as
/// `BuiltinFunction` attrs.
pub(crate) struct ModuleClass {
    /// Outer attributes (doc comments only — validated at parse time).
    pub(crate) attrs: Vec<Attribute>,
    pub(crate) name: Ident,
    pub(crate) methods: Vec<ModuleFn>,
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

/// Parse one `class ClassName { [iter_self;] fn method(args) … }` declaration.
/// The `class` ident is at the head of `input`.
pub(crate) fn parse_class(input: ParseStream, attrs: Vec<Attribute>) -> syn::Result<ModuleClass> {
    validate_class_attrs(&attrs)?;

    let class_kw: Ident = input.parse()?;
    debug_assert_eq!(class_kw.to_string(), "class");
    let name: Ident = input.parse()?;
    let content;
    syn::braced!(content in input);

    let mut iter_self = false;
    if peek_contextual_kw(&content, "iter_self")? {
        let _: Ident = content.parse()?;
        let _: Token![;] = content.parse()?;
        iter_self = true;
    }

    let mut methods: Vec<ModuleFn> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    if iter_self {
        seen_names.insert("__iter__".to_string());
    }

    while !content.is_empty() {
        let method_attrs = content.call(Attribute::parse_outer)?;
        let method = parse_module_fn(&content, method_attrs)?;
        let (py_name, _) = method.py_and_rust_short_name();
        if !seen_names.insert(py_name.clone()) {
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

    if methods.is_empty() && !iter_self {
        return Err(syn::Error::new(
            name.span(),
            format!(
                "`class {name}` must declare at least one method (`fn __init__(args) {{ … }}`)",
            ),
        ));
    }

    Ok(ModuleClass {
        attrs,
        name,
        methods,
        iter_self,
    })
}

/// Synthesize the `ModuleFn` for an `iter_self` class's injected `__iter__` (#1895).
fn synthesized_iter_self(span: Span) -> ModuleFn {
    let args_ident = Ident::new("args", span);
    let body: Block = syn::parse_quote!({ Ok(args[0].value.clone()) });
    ModuleFn {
        attrs: Vec::new(),
        short_name: Ident::new("__iter__", span),
        py_name_override: None,
        args: ModuleFnArgs::Legacy { args_ident },
        arity_style: ArityStyle::Standard,
        bare_name: false,
        method_descriptor: true,
        body,
    }
}

/// Emit method functions, registry entries, and the `PyClass` construction
/// statement for one `ModuleClass`.
pub(crate) fn emit_class_artefacts(class: &ModuleClass) -> ClassArtefacts {
    let class_name_ident = &class.name;
    let class_name_str = class_name_ident.to_string();
    let class_name_lit = LitStr::new(&class_name_str, class_name_ident.span());
    // `class.attrs` are source-only doc comments (emitting them on a statement
    // would trip `unused_doc_comments`).
    let _ = &class.attrs;

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
            attrs.insert("__hash__".to_string(), crate::value::Value::none());
        }
    });

    let injected_iter = class
        .iter_self
        .then(|| synthesized_iter_self(class_name_ident.span()));

    let mut fn_items = Vec::new();
    let mut reg_entries = Vec::new();
    let mut method_attr_inserts = Vec::new();

    for method in class.methods.iter().chain(injected_iter.iter()) {
        let (py_short, rust_short) = method.py_and_rust_short_name();
        let short_lit = LitStr::new(&py_short, method.short_name.span());
        let name_suffix = format!("{class_name_str}.{py_short}");
        let rust_suffix = format!("{class_name_str}__{rust_short}");

        let (fn_item, reg_entry, name_static_ident) =
            emit_fn_artefacts(method, &name_suffix, &rust_suffix, true);
        fn_items.push(fn_item);
        reg_entries.push(reg_entry);
        method_attr_inserts.push(emit_attr_insert(&short_lit, &name_static_ident));
    }

    let class_item = quote! {
        attrs.insert(#class_name_lit.to_string(), {
            use std::cell::RefCell;
            use std::rc::Rc;
            use indexmap::IndexMap;
            let mut attrs: IndexMap<String, crate::value::Value> = IndexMap::new();
            #(#method_attr_inserts)*
            // Match class-statement finalization: defining equality without
            // an explicit hash makes every macro-generated class unhashable.
            #implicit_hash_none
            crate::value::Value::py_class(Rc::new(RefCell::new(
                crate::value::PyClass::new(
                    #class_name_lit,
                    #class_name_lit,
                    None,
                    attrs,
                ),
            )))
        });
    };

    ClassArtefacts {
        fn_items,
        reg_entries,
        class_item,
    }
}

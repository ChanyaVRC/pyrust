//! `pyrust_module! { … }` input parsing and module-level expansion.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Expr, Ident, LitStr, Token, parse_macro_input};

use crate::attrs::peek_contextual_kw;
use crate::builtin_fn::{emit_attr_insert, emit_fn_artefacts};
use crate::class_macro::{ClassArtefacts, ModuleClass, emit_class_artefacts, parse_class};
use crate::overload::emit_overload_set_artefacts;
use crate::signature::{ModuleFn, group_funcs_by_name, parse_module_fn};

/// Parsed `pyrust_module! { … }` input.
///
/// The Python-level module name is *not* part of the input — it is read from
/// a sibling `MODULE_NAME: &str` constant injected by `pyrust_builtin_modules!`.
struct ModuleInput {
    constants: Vec<(LitStr, Expr)>,
    funcs: Vec<ModuleFn>,
    classes: Vec<ModuleClass>,
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut constants: Vec<(LitStr, Expr)> = Vec::new();
        if peek_contextual_kw(input, "constants")? {
            let _: Ident = input.parse()?;
            let content;
            syn::braced!(content in input);
            while !content.is_empty() {
                let key: LitStr = content.parse()?;
                let _: Token![=>] = content.parse()?;
                let value: Expr = content.parse()?;
                constants.push((key, value));
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }
            }
            if input.peek(Token![,]) {
                let _: Token![,] = input.parse()?;
            }
        }

        let mut funcs: Vec<ModuleFn> = Vec::new();
        let mut classes: Vec<ModuleClass> = Vec::new();
        while !input.is_empty() {
            let raw_attrs = input.call(Attribute::parse_outer)?;
            if peek_contextual_kw(input, "class")? {
                classes.push(parse_class(input, raw_attrs)?);
            } else {
                funcs.push(parse_module_fn(input, raw_attrs)?);
            }
        }

        Ok(ModuleInput {
            constants,
            funcs,
            classes,
        })
    }
}

/// Expansion of `pyrust_module! { … }`.
pub(crate) fn expand_pyrust_module(input: TokenStream) -> TokenStream {
    let ModuleInput {
        constants,
        funcs,
        classes,
    } = parse_macro_input!(input as ModuleInput);

    let groups = match group_funcs_by_name(&funcs) {
        Ok(g) => g,
        Err(e) => return e.to_compile_error().into(),
    };

    let mut fn_items: Vec<TokenStream2> = Vec::new();
    let mut reg_entries: Vec<TokenStream2> = Vec::new();
    let mut attr_entries: Vec<TokenStream2> = Vec::new();
    let mut class_items: Vec<TokenStream2> = Vec::new();

    for group in &groups {
        let (py_short, rust_short) = group[0].py_and_rust_short_name();
        let short_lit = LitStr::new(&py_short, group[0].short_name.span());

        let (group_fn_items, reg_entry, name_static_ident) = if group.len() == 1 {
            let (fn_item, reg_entry, name_static_ident) =
                emit_fn_artefacts(group[0], &py_short, &rust_short, false);
            (vec![fn_item], reg_entry, name_static_ident)
        } else {
            emit_overload_set_artefacts(group, &py_short, &rust_short)
        };

        fn_items.extend(group_fn_items);
        reg_entries.push(reg_entry);
        attr_entries.push(emit_attr_insert(&short_lit, &name_static_ident));
    }

    for class in &classes {
        let ClassArtefacts {
            fn_items: class_fn_items,
            reg_entries: class_reg_entries,
            class_item,
        } = emit_class_artefacts(class);
        fn_items.extend(class_fn_items);
        reg_entries.extend(class_reg_entries);
        class_items.push(class_item);
    }

    let const_entries: Vec<TokenStream2> = constants
        .iter()
        .map(|(k, v)| quote! { attrs.insert(#k.to_string(), #v); })
        .collect();

    let map_capacity = const_entries.len() + attr_entries.len() + class_items.len();

    quote! {
        // Per-function bodies.
        #(#fn_items)*

        /// The per-module registry slice.  Composed at first call by
        /// leaking `FN_PREFIX + short_name` into a `'static` string for
        /// each entry.  Consumed by `crate::builtin_modules::all_regs`.
        pub(crate) fn regs() -> &'static [crate::builtin_registry::BuiltinReg] {
            static REGS_CELL: std::sync::LazyLock<Vec<crate::builtin_registry::BuiltinReg>> =
                std::sync::LazyLock::new(|| {
                    vec![
                        #(#reg_entries),*
                    ]
                });
            REGS_CELL.as_slice()
        }

        // Back-compat thin shim for callers referencing `REGS()`.
        #[doc(hidden)]
        pub(crate) fn REGS() -> &'static [crate::builtin_registry::BuiltinReg] {
            regs()
        }

        /// Build the PyModule for this built-in module.  Called from the
        /// interpreter's `load_module` path on first `import`.
        pub(crate) fn module() -> crate::value::Value {
            use std::cell::RefCell;
            use std::rc::Rc;
            // Insertion-ordered (issue #2918): constants, then functions, then
            // classes so `vars(module)` reflects source order deterministically.
            let mut attrs: crate::value::ModuleAttrs =
                crate::value::ModuleAttrs::with_capacity_and_hasher(
                    #map_capacity,
                    Default::default(),
                );
            #(#const_entries)*
            #(#attr_entries)*
            #(#class_items)*
            crate::value::Value::py_module(Rc::new(RefCell::new(
                crate::value::PyModule::new(MODULE_NAME.to_string(), attrs)
            )))
        }
    }
    .into()
}

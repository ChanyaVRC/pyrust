//! `pyrust_module! { … }` input parsing and module-level expansion.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, Ident, LitStr, Token, parse_macro_input};

use crate::attrs::{ExceptionAttrs, parse_exception_attrs, peek_contextual_kw};
use crate::builtin_fn::{emit_attr_insert, emit_fn_artefacts};
use crate::class_macro::{
    ClassArtefacts, ClassConst, ModuleClass, emit_class_artefacts, parse_class,
    parse_constants_block,
};
use crate::overload::emit_overload_set_artefacts;
use crate::signature::{ModuleFn, group_funcs_by_name, parse_module_fn, validate_top_level_fn};

/// Base specification inside `exception MyError(Base1, Base2);`.
enum ExceptionBase {
    /// Bare Python exception name (e.g. `OSError`, `Exception`, `MyEarlierError`).
    Named(Ident),
    /// Arbitrary Rust expression implementing `IntoClassBase`.
    Expr(Expr),
}

/// An `exception MyError[(Base1, ...)];` declaration inside `pyrust_module!`.
struct ModuleException {
    cfg_attrs: Vec<Attribute>,
    doc: Option<String>,
    name: Ident,
    bases: Vec<ExceptionBase>,
}

/// Preserves exact declaration order across `exception` and `class` items so
/// later items can inherit from earlier ones in the same module.
enum ModuleTypeDecl {
    Exception(ModuleException),
    Class(Box<ModuleClass>),
}

/// Parsed `pyrust_module! { … }` input.
///
/// The Python-level module name is *not* part of the input — it is read from
/// a sibling `MODULE_NAME: &str` constant injected by `pyrust_builtin_modules!`.
struct ModuleInput {
    constants: Vec<ClassConst>,
    funcs: Vec<ModuleFn>,
    types: Vec<ModuleTypeDecl>,
}

fn parse_exception(input: ParseStream, raw_attrs: Vec<Attribute>) -> syn::Result<ModuleException> {
    let ExceptionAttrs { cfg_attrs, doc } = parse_exception_attrs(raw_attrs)?;
    let exc_kw: Ident = input.parse()?;
    debug_assert_eq!(exc_kw.to_string(), "exception");
    let name: Ident = input.parse()?;

    let mut bases = Vec::new();
    if input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in input);
        let exprs = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?;
        for expr in exprs {
            if let Expr::Path(ref p) = expr
                && p.qself.is_none()
                && p.path.leading_colon.is_none()
                && p.path.segments.len() == 1
                && p.path.segments[0].arguments.is_none()
            {
                bases.push(ExceptionBase::Named(p.path.segments[0].ident.clone()));
            } else {
                bases.push(ExceptionBase::Expr(expr));
            }
        }
    }
    let _: Token![;] = input.parse()?;

    Ok(ModuleException {
        cfg_attrs,
        doc,
        name,
        bases,
    })
}

fn emit_exception_item(exc: &ModuleException) -> TokenStream2 {
    let cfg_attrs = &exc.cfg_attrs;
    let name_str = exc.name.to_string();
    let name_lit = LitStr::new(&name_str, exc.name.span());

    let doc_insert = exc.doc.as_ref().map(|d| {
        let doc_lit = LitStr::new(d, exc.name.span());
        quote! {
            __exc_attrs.insert("__doc__".to_string(), crate::value::Value::string(#doc_lit));
        }
    });

    let base_exprs: Vec<TokenStream2> = if exc.bases.is_empty() {
        vec![quote! {
            crate::builtin_modules::macros::resolve_exception_base(&attrs, "Exception")
        }]
    } else {
        exc.bases
            .iter()
            .map(|b| match b {
                ExceptionBase::Named(ident) => {
                    let base_lit = LitStr::new(&ident.to_string(), ident.span());
                    quote! {
                        crate::builtin_modules::macros::resolve_exception_base(&attrs, #base_lit)
                    }
                }
                ExceptionBase::Expr(expr) => {
                    quote! {
                        crate::builtin_modules::macros::IntoClassBase::into_class_base(#expr, &attrs)
                    }
                }
            })
            .collect()
    };

    quote! {
        #(#cfg_attrs)*
        {
            use std::cell::RefCell;
            use std::rc::Rc;
            let mut __exc_attrs: indexmap::IndexMap<String, crate::value::Value> =
                indexmap::IndexMap::new();
            __exc_attrs.insert(
                "__module__".to_string(),
                crate::value::Value::string(MODULE_NAME),
            );
            #doc_insert
            let __exc_cls = Rc::new(RefCell::new(crate::value::PyClass::new(
                #name_lit,
                #name_lit,
                None,
                __exc_attrs,
            )));
            let __exc_bases: Vec<Rc<RefCell<crate::value::PyClass>>> = vec![#(#base_exprs),*];
            crate::builtin_modules::macros::link_class_bases(&__exc_cls, __exc_bases);
            attrs.insert(
                #name_lit.to_string(),
                crate::value::Value::py_class(__exc_cls),
            );
        }
    }
}

impl Parse for ModuleInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut constants: Vec<ClassConst> = Vec::new();
        let mut funcs: Vec<ModuleFn> = Vec::new();
        let mut types: Vec<ModuleTypeDecl> = Vec::new();

        while !input.is_empty() {
            if peek_contextual_kw(input, "constants")? {
                constants.extend(parse_constants_block(input)?);
                continue;
            }
            let raw_attrs = input.call(Attribute::parse_outer)?;
            if peek_contextual_kw(input, "class")? {
                types.push(ModuleTypeDecl::Class(Box::new(parse_class(
                    input, raw_attrs,
                )?)));
            } else if peek_contextual_kw(input, "exception")? {
                types.push(ModuleTypeDecl::Exception(parse_exception(
                    input, raw_attrs,
                )?));
            } else {
                let func = parse_module_fn(input, raw_attrs)?;
                validate_top_level_fn(&func)?;
                funcs.push(func);
            }
        }

        Ok(ModuleInput {
            constants,
            funcs,
            types,
        })
    }
}

/// Expansion of `pyrust_module! { … }`.
pub(crate) fn expand_pyrust_module(input: TokenStream) -> TokenStream {
    let ModuleInput {
        constants,
        funcs,
        types,
    } = parse_macro_input!(input as ModuleInput);

    let groups = match group_funcs_by_name(&funcs) {
        Ok(g) => g,
        Err(e) => return e.to_compile_error().into(),
    };

    let mut fn_items: Vec<TokenStream2> = Vec::new();
    let mut reg_entries: Vec<TokenStream2> = Vec::new();
    let mut attr_entries: Vec<TokenStream2> = Vec::new();
    let mut type_items: Vec<TokenStream2> = Vec::new();

    for group in &groups {
        let head = group[0];
        let (py_short, rust_short) = head.py_and_rust_short_name();
        let short_lit = LitStr::new(&py_short, head.short_name.span());

        let (group_fn_items, reg_entry, name_static_ident) = if group.len() == 1 {
            let (fn_item, reg_entry, name_static_ident) =
                emit_fn_artefacts(head, &py_short, &rust_short, false, &[], None);
            (vec![fn_item], reg_entry, name_static_ident)
        } else {
            emit_overload_set_artefacts(group, &py_short, &rust_short, false, &[], None)
        };

        fn_items.extend(group_fn_items);
        reg_entries.push(reg_entry);
        attr_entries.push(emit_attr_insert(
            &short_lit,
            &name_static_ident,
            &head.cfg_attrs,
        ));
    }

    for decl in &types {
        match decl {
            ModuleTypeDecl::Exception(exc) => {
                type_items.push(emit_exception_item(exc));
            }
            ModuleTypeDecl::Class(class) => {
                let ClassArtefacts {
                    fn_items: class_fn_items,
                    reg_entries: class_reg_entries,
                    class_item,
                } = emit_class_artefacts(class);
                fn_items.extend(class_fn_items);
                reg_entries.extend(class_reg_entries);
                type_items.push(class_item);
            }
        }
    }

    let const_entries: Vec<TokenStream2> = constants
        .iter()
        .map(|c| {
            let cfg_attrs = &c.cfg_attrs;
            let k = &c.key;
            let v = &c.value;
            quote! {
                #(#cfg_attrs)*
                attrs.insert(#k.to_string(), #v);
            }
        })
        .collect();

    let reg_capacity = reg_entries.len();
    let map_capacity = const_entries.len() + attr_entries.len() + type_items.len();

    quote! {
        // Per-function bodies.
        #(#fn_items)*

        /// The per-module registry slice.  Composed at first call by
        /// leaking `FN_PREFIX + short_name` into a `'static` string for
        /// each entry.  Consumed by `crate::builtin_modules::all_regs`.
        pub(crate) fn regs() -> &'static [crate::builtin_registry::BuiltinReg] {
            static REGS_CELL: std::sync::LazyLock<Vec<crate::builtin_registry::BuiltinReg>> =
                std::sync::LazyLock::new(|| {
                    #[allow(unused_mut)]
                    let mut regs: Vec<crate::builtin_registry::BuiltinReg> =
                        Vec::with_capacity(#reg_capacity);
                    #(#reg_entries)*
                    regs
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
            // exceptions/classes so `vars(module)` reflects source order deterministically.
            #[allow(unused_mut)]
            let mut attrs: crate::value::ModuleAttrs =
                crate::value::ModuleAttrs::with_capacity_and_hasher(
                    #map_capacity,
                    Default::default(),
                );
            #(#const_entries)*
            #(#attr_entries)*
            #(#type_items)*
            crate::value::Value::py_module(Rc::new(RefCell::new(
                crate::value::PyModule::new(MODULE_NAME.to_string(), attrs)
            )))
        }
    }
    .into()
}

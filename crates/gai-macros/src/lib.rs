//! `#[gai::tool]`: write an agent tool as a plain async function.
//!
//! ```ignore
//! /// Add two numbers.
//! #[gai::tool]
//! async fn add(#[arg(desc = "The first number")] a: f64, b: f64) -> Result<f64, String> {
//!     Ok(a + b)
//! }
//!
//! runtime.register_tool(add_tool());
//! ```
//!
//! The function stays as written. Next to it the macro adds `<name>_tool()`,
//! which returns the tool as a `gai::tools::SharedTool`: its name is the
//! function's, its description the doc comment, and its input schema comes
//! from the parameters (`Option<T>` parameters are optional). A parameter of
//! type `ToolCtx` or `&ToolCtx` receives the call's context instead of an
//! input. The return value can be anything `gai::tools::IntoToolOutput`
//! covers: text, numbers, `serde_json::Value`, `gai::tools::Json(..)`,
//! `ToolOutput`, or a `Result` of those (errors are shown to the model).
//!
//! Options: `#[gai::tool(name = "add_numbers", policy = "approve",
//! idempotent)]`, and per parameter `#[arg(desc = "...", default)]`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, FnArg, Ident, ItemFn, Lit, LitStr, Meta, Pat, Token, Type,
    parse_macro_input,
};

#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let opts = parse_macro_input!(attr as Options);
    let func = parse_macro_input!(item as ItemFn);
    expand(opts, func)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[derive(Default)]
struct Options {
    name: Option<LitStr>,
    policy: Option<LitStr>,
    idempotent: bool,
}

impl Parse for Options {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut o = Options::default();
        for meta in Punctuated::<Meta, Token![,]>::parse_terminated(input)? {
            match &meta {
                Meta::Path(p) if p.is_ident("idempotent") => o.idempotent = true,
                Meta::NameValue(nv) if nv.path.is_ident("name") => {
                    o.name = Some(lit_str(&nv.value)?)
                }
                Meta::NameValue(nv) if nv.path.is_ident("policy") => {
                    let v = lit_str(&nv.value)?;
                    if !matches!(v.value().as_str(), "auto" | "approve" | "deny") {
                        return Err(syn::Error::new(v.span(), "policy is auto, approve or deny"));
                    }
                    o.policy = Some(v);
                }
                m => {
                    return Err(syn::Error::new(
                        m.span(),
                        "expected `name = \"..\"`, `policy = \"auto|approve|deny\"` or `idempotent`",
                    ));
                }
            }
        }
        Ok(o)
    }
}

fn lit_str(e: &Expr) -> syn::Result<LitStr> {
    match e {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Ok(s.clone()),
        e => Err(syn::Error::new(e.span(), "expected a string")),
    }
}

/// The text of `///` comments: wrapped lines joined, paragraphs kept apart.
fn docs(attrs: &[Attribute]) -> String {
    let lines = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => lit_str(&nv.value).ok().map(|s| s.value().trim().to_owned()),
            _ => None,
        });
    let mut out = String::new();
    for l in lines {
        if l.is_empty() {
            out.push_str("\n\n");
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(&l);
        }
    }
    out.trim().to_owned()
}

/// `Some(by_ref)` when the type is `ToolCtx` or `&ToolCtx`.
fn is_ctx(ty: &Type) -> Option<bool> {
    let (inner, by_ref) = match ty {
        Type::Reference(r) => (&*r.elem, true),
        t => (t, false),
    };
    match inner {
        Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "ToolCtx") => {
            Some(by_ref)
        }
        _ => None,
    }
}

struct Param {
    ident: Ident,
    ty: Type,
    desc: Option<String>,
    default: bool,
}

fn expand(opts: Options, mut func: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.fn_token.span(),
            "#[gai::tool] goes on an `async fn`",
        ));
    }
    if !func.sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            func.sig.generics.span(),
            "a tool function can't be generic",
        ));
    }
    let fn_name = func.sig.ident.clone();
    let tool_name = opts
        .name
        .as_ref()
        .map(LitStr::value)
        .unwrap_or_else(|| fn_name.to_string());
    let description = docs(&func.attrs);
    if description.is_empty() {
        return Err(syn::Error::new(
            fn_name.span(),
            "add a /// doc comment: it is the tool's description for the model",
        ));
    }

    let mut params = Vec::new();
    // How each argument is passed to the function.
    let mut call_args = Vec::new();
    for input in func.sig.inputs.iter_mut() {
        let FnArg::Typed(pt) = input else {
            return Err(syn::Error::new(input.span(), "a tool can't take `self`"));
        };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new(
                pt.pat.span(),
                "use a plain name for each parameter",
            ));
        };
        let ident = pi.ident.clone();
        let mut desc = None;
        let mut default = false;
        let mut kept = Vec::new();
        for a in std::mem::take(&mut pt.attrs) {
            if !a.path().is_ident("arg") {
                kept.push(a);
                continue;
            }
            a.parse_nested_meta(|m| {
                if m.path.is_ident("desc") || m.path.is_ident("description") {
                    desc = Some(m.value()?.parse::<LitStr>()?.value());
                    Ok(())
                } else if m.path.is_ident("default") {
                    default = true;
                    Ok(())
                } else {
                    Err(m.error("expected `desc = \"..\"` or `default`"))
                }
            })?;
        }
        pt.attrs = kept;
        match is_ctx(&pt.ty) {
            Some(true) => call_args.push(quote!(&__ctx)),
            Some(false) => call_args.push(quote!(__ctx.clone())),
            None => {
                call_args.push(quote!(__args.#ident));
                params.push(Param {
                    ident,
                    ty: (*pt.ty).clone(),
                    desc,
                    default,
                });
            }
        }
    }

    let vis = &func.vis;
    let args_ty = format_ident!("__GaiToolArgs_{}", fn_name);
    let tool_fn = format_ident!("{}_tool", fn_name);
    let fields = params.iter().map(|p| {
        let (ident, ty) = (&p.ident, &p.ty);
        let doc = p.desc.as_ref().map(|d| quote!(#[doc = #d]));
        let default = p.default.then(|| quote!(#[serde(default)]));
        quote!(#doc #default #ident: #ty)
    });
    let policy = opts.policy.as_ref().map(|p| {
        let variant = match p.value().as_str() {
            "approve" => quote!(Approve),
            "deny" => quote!(Deny),
            _ => quote!(Auto),
        };
        quote!(.policy(::gai::tools::ToolPolicy::#variant))
    });
    let idempotent = opts.idempotent.then(|| quote!(.idempotent()));
    let doc_line = format!(" The `{tool_name}` tool, made from [`{fn_name}`].");
    let span = Span::call_site();
    let tool_name = LitStr::new(&tool_name, span);
    let description = LitStr::new(&description, span);

    Ok(quote! {
        #func

        #[allow(non_camel_case_types)]
        #[doc(hidden)]
        #[derive(::gai::__private::serde::Deserialize, ::gai::__private::schemars::JsonSchema)]
        #[serde(crate = "::gai::__private::serde", deny_unknown_fields)]
        #[schemars(crate = "::gai::__private::schemars")]
        struct #args_ty { #(#fields),* }

        #[doc = #doc_line]
        #vis fn #tool_fn() -> ::gai::tools::SharedTool {
            ::std::sync::Arc::new(
                ::gai::tools::typed_tool(
                    #tool_name,
                    #description,
                    |__ctx: ::gai::tools::ToolCtx, __args: #args_ty| async move {
                        ::gai::tools::IntoToolOutput::into_tool_output(#fn_name(#(#call_args),*).await)
                    },
                )
                #policy
                #idempotent
            )
        }
    })
}

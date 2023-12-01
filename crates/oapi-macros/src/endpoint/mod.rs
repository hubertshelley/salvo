use proc_macro2::{Span, TokenStream};
use quote::{quote, ToTokens};
use syn::{Expr, Ident, ImplItem, Item, Pat, ReturnType, Signature, Type};

use crate::doc_comment::CommentAttributes;
use crate::{omit_type_path_lifetimes, parse_input_type, Array, InputType, Operation};

mod attr;
pub(crate) use attr::EndpointAttr;

fn metadata(
    salvo: &Ident,
    oapi: &Ident,
    attr: EndpointAttr,
    name: &Ident,
    mut modifiers: Vec<TokenStream>,
) -> syn::Result<TokenStream> {
    let tfn = Ident::new(&format!("__salvo_oapi_endpoint_type_id_{}", name), Span::call_site());
    let cfn = Ident::new(&format!("__salvo_oapi_endpoint_creator_{}", name), Span::call_site());
    let opt = Operation::new(&attr);
    modifiers.append(opt.modifiers().as_mut());
    let status_codes = Array::from_iter(attr.status_codes.iter().map(|expr| match expr {
        Expr::Lit(lit) => {
            quote! {
                #salvo::http::StatusCode::from_u16(#lit).unwrap()
            }
        }
        _ => {
            quote! {
                #expr
            }
        }
    }));
    let stream = quote! {
        fn #tfn() -> ::std::any::TypeId {
            ::std::any::TypeId::of::<#name>()
        }
        fn #cfn() -> #oapi::oapi::Endpoint {
            let mut components = #oapi::oapi::Components::new();
            let status_codes: &[#salvo::http::StatusCode] = &#status_codes;
            fn modify(components: &mut #oapi::oapi::Components, operation: &mut #oapi::oapi::Operation) {
                #(#modifiers)*
            }
            let mut operation = #oapi::oapi::Operation::new();
            modify(&mut components, &mut operation);
            if operation.operation_id.is_none() {
                operation.operation_id = Some(::std::any::type_name::<#name>().replace("::", "."));
            }
            if !status_codes.is_empty() {
                let responses = std::ops::DerefMut::deref_mut(&mut operation.responses);
                responses.retain(|k,_| {
                    if let Ok(code) = <#salvo::http::StatusCode as std::str::FromStr>::from_str(k) {
                        status_codes.contains(&code)
                    } else {
                        true
                    }
                });
            }
            #oapi::oapi::Endpoint{
                operation,
                components,
            }
        }
        #oapi::oapi::__private::inventory::submit! {
            #oapi::oapi::EndpointRegistry::save(#tfn, #cfn)
        }
    };
    Ok(stream)
}
pub(crate) fn generate(mut attr: EndpointAttr, input: Item) -> syn::Result<TokenStream> {
    let salvo = crate::salvo_crate();
    let oapi = crate::oapi_crate();
    match input {
        Item::Fn(mut item_fn) => {
            let attrs = &item_fn.attrs;
            let vis = &item_fn.vis;
            let sig = &mut item_fn.sig;
            let body = &item_fn.block;
            let name = &sig.ident;
            let docs = item_fn
                .attrs
                .iter()
                .filter(|attr| attr.path().is_ident("doc"))
                .cloned()
                .collect::<Vec<_>>();

            let sdef = quote! {
                #(#docs)*
                #[allow(non_camel_case_types)]
                #[derive(Debug)]
                #vis struct #name;
                impl #name {
                    #(#attrs)*
                    #sig {
                        #body
                    }
                }
            };

            attr.doc_comments = Some(CommentAttributes::from_attributes(attrs).0);
            attr.deprecated = if attrs.iter().any(|attr| attr.path().is_ident("deprecated")) {
                Some(true)
            } else {
                None
            };

            let (hfn, modifiers) = handle_fn(&salvo, &oapi, sig)?;
            let meta = metadata(&salvo, &oapi, attr, name, modifiers)?;
            Ok(quote! {
                #sdef
                #[#salvo::async_trait]
                impl #salvo::Handler for #name {
                    #hfn
                }
                #meta
            })
        }
        Item::Impl(item_impl) => {
            let attrs = &item_impl.attrs;

            attr.doc_comments = Some(CommentAttributes::from_attributes(attrs).0);
            attr.deprecated = if attrs.iter().any(|attr| attr.path().is_ident("deprecated")) {
                Some(true)
            } else {
                None
            };

            let mut hmtd = None;
            for item in &item_impl.items {
                if let ImplItem::Fn(method) = item {
                    if method.sig.ident == Ident::new("handle", Span::call_site()) {
                        hmtd = Some(method);
                    }
                }
            }
            if hmtd.is_none() {
                return Err(syn::Error::new_spanned(item_impl.impl_token, "missing handle function"));
            }
            let hmtd = hmtd.unwrap();
            let (hfn, modifiers) = handle_fn(&salvo, &oapi, &hmtd.sig)?;
            let ty = &item_impl.self_ty;
            let (impl_generics, _, where_clause) = &item_impl.generics.split_for_impl();
            let name = Ident::new(&ty.to_token_stream().to_string(), Span::call_site());
            let meta = metadata(&salvo, &oapi, attr, &name, modifiers)?;

            Ok(quote! {
                #item_impl
                #[#salvo::async_trait]
                impl #impl_generics #salvo::Handler for #ty #where_clause {
                    #hfn
                }
                #meta
            })
        }
        _ => Err(syn::Error::new_spanned(
            input,
            "#[handler] must added to `impl` or `fn`",
        )),
    }
}

fn handle_fn(salvo: &Ident, oapi: &Ident, sig: &Signature) -> syn::Result<(TokenStream, Vec<TokenStream>)> {
    let name = &sig.ident;
    let mut extract_ts = Vec::with_capacity(sig.inputs.len());
    let mut call_args: Vec<Ident> = Vec::with_capacity(sig.inputs.len());
    let mut modifiers = Vec::new();
    for input in &sig.inputs {
        match parse_input_type(input) {
            InputType::Request(_pat) => {
                call_args.push(Ident::new("__macro_generated_req", Span::call_site()));
            }
            InputType::Depot(_pat) => {
                call_args.push(Ident::new("depot", Span::call_site()));
            }
            InputType::Response(_pat) => {
                call_args.push(Ident::new("res", Span::call_site()));
            }
            InputType::FlowCtrl(_pat) => {
                call_args.push(Ident::new("ctrl", Span::call_site()));
            }
            InputType::Unknown => {
                return Err(syn::Error::new_spanned(
                    &sig.inputs,
                    "the inputs parameters must be Request, Depot, Response or FlowCtrl",
                ))
            }
            InputType::NoReference(pat) => {
                if let (Pat::Ident(ident), Type::Path(ty)) = (&*pat.pat, &*pat.ty) {
                    call_args.push(ident.ident.clone());
                    let id = &pat.pat;
                    let ty = omit_type_path_lifetimes(ty);
                    let idv = id.to_token_stream().to_string();
                    // If id like `mut pdata`, then idv is `pdata`;
                    let idv = idv.rsplit_once(' ').map(|(_, v)| v.to_owned()).unwrap_or(idv);
                    let id = Ident::new(&idv, Span::call_site());
                    let idv = idv.trim_start_matches('_');
                    extract_ts.push(quote! {
                            let #id: #ty = match <#ty as #salvo::Extractible>::extract_with_arg(__macro_generated_req, #idv).await {
                                Ok(data) => {
                                    data
                                },
                                Err(e) => {
                                    #salvo::__private::tracing::error!(error = ?e, "failed to extract data in endpoint macro");
                                    res.render(#salvo::http::errors::StatusError::bad_request().brief(
                                        "Failed to extract data in endpoint macro."
                                    ).cause(e));
                                    return;
                                }
                            };
                        });
                    modifiers.push(quote! {
                         <#ty as #oapi::oapi::EndpointArgRegister>::register(components, operation, #idv);
                    });
                } else {
                    return Err(syn::Error::new_spanned(pat, "invalid param definition"));
                }
            }
            InputType::Receiver(_) => {
                call_args.push(Ident::new("self", Span::call_site()));
            }
        }
    }

    let hfn = match &sig.output {
        ReturnType::Default => {
            if sig.asyncness.is_none() {
                quote! {
                    #[inline]
                    async fn handle(&self, __macro_generated_req: &mut #salvo::Request, depot: &mut #salvo::Depot, res: &mut #salvo::Response, ctrl: &mut #salvo::FlowCtrl) {
                        #(#extract_ts)*
                        Self::#name(#(#call_args),*)
                    }
                }
            } else {
                quote! {
                    #[inline]
                    async fn handle(&self, __macro_generated_req: &mut #salvo::Request, depot: &mut #salvo::Depot, res: &mut #salvo::Response, ctrl: &mut #salvo::FlowCtrl) {
                        #(#extract_ts)*
                        Self::#name(#(#call_args),*).await
                    }
                }
            }
        }
        ReturnType::Type(_, ty) => {
            modifiers.push(quote! {
                <#ty as #oapi::oapi::EndpointOutRegister>::register(components, operation);
            });
            if sig.asyncness.is_none() {
                quote! {
                    #[inline]
                    async fn handle(&self, __macro_generated_req: &mut #salvo::Request, depot: &mut #salvo::Depot, res: &mut #salvo::Response, ctrl: &mut #salvo::FlowCtrl) {
                        #(#extract_ts)*
                        #salvo::Writer::write(Self::#name(#(#call_args),*), __macro_generated_req, depot, res).await;
                    }
                }
            } else {
                quote! {
                    #[inline]
                    async fn handle(&self, __macro_generated_req: &mut #salvo::Request, depot: &mut #salvo::Depot, res: &mut #salvo::Response, ctrl: &mut #salvo::FlowCtrl) {
                        #(#extract_ts)*
                        #salvo::Writer::write(Self::#name(#(#call_args),*).await, __macro_generated_req, depot, res).await;
                    }
                }
            }
        }
    };
    Ok((hfn, modifiers))
}

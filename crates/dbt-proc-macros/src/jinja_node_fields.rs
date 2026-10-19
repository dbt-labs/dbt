use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Meta, Type};

/// Matches `dbt_yaml`'s check for keys flattened with the `flatten_dunder` feature.
pub(crate) fn is_flatten_key(key: &[u8]) -> bool {
    key.len() > 4
        && key[0] == b'_'
        && key[1] == b'_'
        && key[key.len() - 2] == b'_'
        && key[key.len() - 1] == b'_'
}

struct FieldInfo<'a> {
    field: &'a syn::Field,
    key: String,
    flatten: bool,
    skip_if: Option<syn::Path>,
    serializer: Option<syn::Path>,
}

fn parse_field(field: &syn::Field) -> syn::Result<Option<FieldInfo<'_>>> {
    let ident = field.ident.as_ref().unwrap();
    let mut key = ident.to_string();
    let mut flatten = is_flatten_key(key.as_bytes());
    let mut skip_if = None;
    let mut serializer = None;
    for attr in &field.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let metas = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;
        for meta in metas {
            if meta.path().is_ident("skip") || meta.path().is_ident("skip_serializing") {
                return Ok(None);
            } else if meta.path().is_ident("flatten") {
                flatten = true;
            } else if let Meta::NameValue(nv) = meta {
                let syn::Expr::Lit(expr) = nv.value else {
                    continue;
                };
                let syn::Lit::Str(lit) = expr.lit else {
                    continue;
                };
                if nv.path.is_ident("rename") {
                    key = lit.value();
                } else if nv.path.is_ident("skip_serializing_if") {
                    skip_if = Some(lit.parse::<syn::Path>()?);
                } else if nv.path.is_ident("serialize_with") {
                    serializer = Some(lit.parse::<syn::Path>()?);
                }
            }
        }
    }
    Ok(Some(FieldInfo {
        field,
        key,
        flatten,
        skip_if,
        serializer,
    }))
}

pub fn derive(input: DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            name,
            "JinjaNodeFields requires a struct",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            name,
            "JinjaNodeFields requires named fields",
        ));
    };
    let mut infos = Vec::new();
    for field in &fields.named {
        if let Some(info) = parse_field(field)? {
            infos.push(info);
        }
    }
    // Only flattened structs can repeat a key: `__other__` holds the keys no typed field took.
    let may_repeat_keys = infos
        .iter()
        .any(|info| info.flatten && info.field.ident.as_ref().unwrap() != "__other__");

    let mode_ty = quote! { crate::schemas::serialization_utils::SerializationMode };
    let keep_none = quote! { #mode_ty::KeepNone };
    let mut getters = Vec::new();
    let mut keys = Vec::new();
    let mut contains = Vec::new();
    // `keys` capacity: typed fields, assuming every `skip_serializing_if` lets them through, plus
    // the entries of each `__other__` map.
    let mut plain_count = 0usize;
    let mut flattened_capacity = Vec::new();
    let mut other_capacity = Vec::new();
    for info in &infos {
        let ident = info.field.ident.as_ref().unwrap();
        let key = &info.key;
        let allowed = info
            .skip_if
            .as_ref()
            .map(|path| quote! { !#path(&self.#ident) });
        let and_allowed = allowed.as_ref().map(|allowed| quote! { && #allowed });
        let guard = |body: TokenStream| match &allowed {
            Some(allowed) => quote! { if #allowed { #body } },
            None => body,
        };
        if info.flatten {
            if ident == "__other__" {
                getters.push(guard(quote! {
                    if let Some(value) = self.#ident.get(key) {
                        return crate::jinja_node::serialize_field(value, mode);
                    }
                }));
                contains.push(guard(quote! {
                    if let Some(value) = self.#ident.get(key) {
                        return mode == #keep_none || !matches!(value, ::dbt_yaml::Value::Null(_));
                    }
                }));
                keys.push(guard(if may_repeat_keys {
                    quote! { keys.extend(self.#ident.keys().map(String::as_str)); }
                } else {
                    quote! {
                        keys.extend(
                            self.#ident
                                .iter()
                                .filter(|(_, value)| mode == #keep_none || !matches!(value, ::dbt_yaml::Value::Null(_)))
                                .map(|(key, _)| key.as_str()),
                        );
                    }
                }));
                other_capacity.push(quote! { self.#ident.len() });
            } else {
                getters.push(guard(quote! {
                    if crate::jinja_node::JinjaNodeFields::contains_key(&self.#ident, key, #keep_none) {
                        return crate::jinja_node::JinjaNodeFields::get(&self.#ident, key, mode);
                    }
                }));
                contains.push(guard(quote! {
                    if crate::jinja_node::JinjaNodeFields::contains_key(&self.#ident, key, #keep_none) {
                        return crate::jinja_node::JinjaNodeFields::contains_key(&self.#ident, key, mode);
                    }
                }));
                keys.push(guard(quote! {
                    keys.extend(crate::jinja_node::JinjaNodeFields::keys(&self.#ident, #keep_none));
                }));
                let ty = &info.field.ty;
                flattened_capacity.push(quote! { <#ty>::JINJA_NODE_KEYS_CAPACITY });
            }
            continue;
        }
        plain_count += 1;
        let serialize = match &info.serializer {
            Some(serializer) => quote! {
                crate::jinja_node::serialize_field_with(&self.#ident, mode, |value, serializer| {
                    #serializer(value, serializer)
                })
            },
            None => quote! { crate::jinja_node::serialize_field(&self.#ident, mode) },
        };
        let present = nonnull_check(info).map(|nonnull| quote! { mode == #keep_none || #nonnull });
        getters.push(quote! {
            if key == #key #and_allowed { return #serialize; }
        });
        let present_or_true = present.clone().unwrap_or_else(|| quote! { true });
        contains.push(quote! {
            if key == #key #and_allowed { return #present_or_true; }
        });
        // When keys can't repeat, only present ones are added. Otherwise presence is checked after
        // deduplication, as a later field may serve the same key.
        let present = present.filter(|_| !may_repeat_keys);
        keys.push(match (&allowed, &present) {
            (None, None) => quote! { keys.push(#key); },
            (Some(allowed), None) => quote! { if #allowed { keys.push(#key); } },
            (None, Some(present)) => quote! { if #present { keys.push(#key); } },
            (Some(allowed), Some(present)) => {
                quote! { if #allowed && (#present) { keys.push(#key); } }
            }
        });
    }
    // YAML flattened fields overwrite earlier fields, including earlier flattened structs.
    getters.reverse();
    contains.reverse();
    // Serde keeps a repeated key at its first position, with the value of the last field that
    // serves it.
    let dedup = may_repeat_keys.then(|| {
        quote! {
            let mut seen = ::std::collections::BTreeSet::new();
            keys.retain(|key| {
                seen.insert(*key)
                    && (mode == #keep_none
                        || crate::jinja_node::JinjaNodeFields::contains_key(self, key, mode))
            });
        }
    });
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #name #ty_generics #where_clause {
            /// Most keys `JinjaNodeFields::keys` can return from typed fields.
            pub(crate) const JINJA_NODE_KEYS_CAPACITY: usize = #plain_count #(+ #flattened_capacity)*;
        }

        // `serialize_with` paths come from the field attributes, as with serde's derive.
        #[automatically_derived]
        #[allow(unused_qualifications)]
        impl #impl_generics crate::jinja_node::JinjaNodeFields for #name #ty_generics #where_clause {
            fn get(&self, key: &str, mode: #mode_ty) -> Option<::dbt_yaml::Value> {
                #(#getters)*
                None
            }
            fn contains_key(&self, key: &str, mode: #mode_ty) -> bool {
                #(#contains)*
                false
            }
            fn keys(&self, mode: #mode_ty) -> Vec<&str> {
                let mut keys: Vec<&str> =
                    Vec::with_capacity(Self::JINJA_NODE_KEYS_CAPACITY #(+ #other_capacity)*);
                #(#keys)*
                #dedup
                keys
            }
        }
    })
}

/// A check that the field doesn't serialize to null, or `None` when it never does.
fn nonnull_check(info: &FieldInfo<'_>) -> Option<TokenStream> {
    let ident = info.field.ident.as_ref().unwrap();
    let key = &info.key;
    let serialized = quote! { crate::jinja_node::JinjaNodeFields::get(self, #key, mode).is_some() };
    if info.serializer.is_some() {
        return Some(serialized);
    }
    let Type::Path(path) = &info.field.ty else {
        return Some(serialized);
    };
    let name = &path.path.segments.last().unwrap().ident;
    if name == "Option" {
        Some(quote! { self.#ident.is_some() })
    } else if name == "Value" || name == "YmlValue" {
        Some(quote! { !self.#ident.is_null() })
    } else {
        None
    }
}

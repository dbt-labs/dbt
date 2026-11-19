use proc_macro2::TokenStream;
use quote::{ToTokens, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, GenericArgument, Meta, PathArguments, Type};

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
    // Served without caching: a scalar type, or `#[jinja_node(disable_cache)]`.
    uncached: bool,
    // `#[jinja_node(allow_uncached_transform)]`.
    allow_uncached_transform: bool,
    // Serde attributes this derive doesn't recognize. Each one gets a compiler warning.
    unknown_serde: Vec<Meta>,
}

/// Serde field attributes that don't change how a field serializes.
const IGNORED_SERDE_ATTRIBUTES: &[&str] = &["default", "deserialize_with", "skip_deserializing"];

/// Types that always serialize to a string, number, bool or none.
const SCALAR_TYPES: &[&str] = &[
    "String", "bool", "char", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32",
    "u64", "u128", "usize", "f32", "f64", "PathBuf",
];

/// Whether `ty` is one of [`SCALAR_TYPES`], or an `Option` of one.
fn is_scalar_type(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    match &segment.arguments {
        PathArguments::None => SCALAR_TYPES.iter().any(|name| segment.ident == name),
        PathArguments::AngleBracketed(args) if segment.ident == "Option" => {
            matches!(
                args.args.first(),
                Some(GenericArgument::Type(inner)) if args.args.len() == 1 && is_scalar_type(inner)
            )
        }
        _ => false,
    }
}

fn parse_field(field: &syn::Field) -> syn::Result<Option<FieldInfo<'_>>> {
    let ident = field.ident.as_ref().unwrap();
    let mut key = ident.to_string();
    let mut flatten = is_flatten_key(key.as_bytes());
    let mut skip_if = None;
    let mut serializer = None;
    let mut disable_cache = false;
    let mut allow_uncached_transform = false;
    let mut unknown_serde = Vec::new();
    for attr in &field.attrs {
        if attr.path().is_ident("jinja_node") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("disable_cache") {
                    disable_cache = true;
                } else if meta.path.is_ident("allow_uncached_transform") {
                    allow_uncached_transform = true;
                } else {
                    return Err(
                        meta.error("expected `disable_cache` or `allow_uncached_transform`")
                    );
                }
                Ok(())
            })?;
            continue;
        }
        if !attr.path().is_ident("serde") {
            continue;
        }
        let metas = attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )?;
        for meta in metas {
            let path = meta.path();
            let value = match &meta {
                Meta::NameValue(syn::MetaNameValue {
                    value:
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(lit),
                            ..
                        }),
                    ..
                }) => Some(lit),
                _ => None,
            };
            if path.is_ident("skip") || path.is_ident("skip_serializing") {
                return Ok(None);
            } else if path.is_ident("flatten") {
                flatten = true;
            } else if let Some(lit) = value
                && path.is_ident("rename")
            {
                key = lit.value();
            } else if let Some(lit) = value
                && path.is_ident("skip_serializing_if")
            {
                skip_if = Some(lit.parse::<syn::Path>()?);
            } else if let Some(lit) = value
                && path.is_ident("serialize_with")
            {
                serializer = Some(lit.parse::<syn::Path>()?);
            } else if let Some(lit) = value
                && path.is_ident("with")
            {
                let mut module = lit.parse::<syn::Path>()?;
                module.segments.push(syn::parse_quote!(serialize));
                serializer = Some(module);
            } else if !IGNORED_SERDE_ATTRIBUTES
                .iter()
                .any(|name| path.is_ident(name))
            {
                unknown_serde.push(meta.clone());
            }
        }
    }
    if disable_cache && flatten {
        return Err(syn::Error::new_spanned(
            field,
            "`#[jinja_node(disable_cache)]` doesn't apply to flattened fields",
        ));
    }
    // A custom serializer can produce a map or a list, whatever the field's type, and so might a
    // serde attribute this derive doesn't recognize.
    let uncached = disable_cache
        || (!flatten
            && serializer.is_none()
            && unknown_serde.is_empty()
            && is_scalar_type(&field.ty));
    if allow_uncached_transform && !uncached {
        return Err(syn::Error::new_spanned(
            field,
            "`#[jinja_node(allow_uncached_transform)]` applies only to uncached fields: scalar \
             types, or `#[jinja_node(disable_cache)]`",
        ));
    }
    Ok(Some(FieldInfo {
        field,
        key,
        flatten,
        skip_if,
        serializer,
        uncached,
        allow_uncached_transform,
        unknown_serde,
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
    let field_kind = quote! { crate::jinja_node::FieldKind };
    let mut getters = Vec::new();
    let mut keys = Vec::new();
    let mut contains = Vec::new();
    let mut kinds = Vec::new();
    let mut jinja_values = Vec::new();
    // `get_jinja_value` converts uncached fields directly and leaves every other field to `get`.
    let via_get = quote! { return self.get(key, mode).map(::minijinja::Value::from_serialize); };
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
                kinds.push(guard(quote! {
                    if self.#ident.contains_key(key) {
                        return #field_kind::Cached;
                    }
                }));
                jinja_values.push(guard(quote! {
                    if self.#ident.contains_key(key) { #via_get }
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
                kinds.push(guard(quote! {
                    if crate::jinja_node::JinjaNodeFields::contains_key(&self.#ident, key, #keep_none) {
                        return crate::jinja_node::JinjaNodeFields::field_kind(&self.#ident, key);
                    }
                }));
                jinja_values.push(guard(quote! {
                    if crate::jinja_node::JinjaNodeFields::contains_key(&self.#ident, key, #keep_none) {
                        return crate::jinja_node::JinjaNodeFields::get_jinja_value(&self.#ident, key, mode);
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
        let kind = field_kind_variant(info);
        kinds.push(quote! {
            if key == #key #and_allowed { return #kind; }
        });
        // An uncached field is a scalar, so it converts straight to a Jinja value. Other fields go
        // through `get`'s YAML, which flattens `__dunder__` fields and strips nested nulls. They need
        // an arm only when a flattened struct may serve the same key from a lower-precedence arm.
        if !info.uncached {
            if may_repeat_keys {
                jinja_values.push(quote! {
                    if key == #key #and_allowed { #via_get }
                });
            }
        } else {
            let value = direct_jinja_value(info);
            jinja_values.push(quote! {
                if key == #key #and_allowed {
                    let value = #value;
                    return (mode == #keep_none || !value.is_none()).then_some(value);
                }
            });
        }
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
    kinds.reverse();
    jinja_values.reverse();
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
    let warnings = unknown_serde_warnings(&infos);
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        #(#warnings)*

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
            fn field_kind(&self, key: &str) -> #field_kind {
                #(#kinds)*
                #field_kind::Cached
            }
            fn get_jinja_value(&self, key: &str, mode: #mode_ty) -> Option<::minijinja::Value> {
                #(#jinja_values)*
                #via_get
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

/// The `FieldKind` of a plain field.
fn field_kind_variant(info: &FieldInfo<'_>) -> TokenStream {
    match (info.uncached, info.allow_uncached_transform) {
        (false, _) => quote! { crate::jinja_node::FieldKind::Cached },
        (true, false) => quote! { crate::jinja_node::FieldKind::Uncached },
        (true, true) => quote! { crate::jinja_node::FieldKind::UncachedTransformable },
    }
}

/// An uncached field converted straight to a Jinja value, through its custom serializer if it has
/// one.
fn direct_jinja_value(info: &FieldInfo<'_>) -> TokenStream {
    let ident = info.field.ident.as_ref().unwrap();
    let Some(serializer) = &info.serializer else {
        return quote! { ::minijinja::Value::from_serialize(&self.#ident) };
    };
    let ty = &info.field.ty;
    quote! {{
        struct SerializeWith<'a>(&'a #ty);
        impl ::serde::Serialize for SerializeWith<'_> {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                #serializer(self.0, serializer)
            }
        }
        ::minijinja::Value::from_serialize(SerializeWith(&self.#ident))
    }}
}

/// Proc macros can't emit warnings on stable Rust, so each unknown serde attribute uses a
/// deprecated constant at the attribute's span.
fn unknown_serde_warnings<'a>(
    infos: &'a [FieldInfo<'_>],
) -> impl Iterator<Item = TokenStream> + 'a {
    infos.iter().flat_map(|info| {
        let field = info.field.ident.as_ref().unwrap();
        info.unknown_serde.iter().map(move |meta| {
            let note = format!(
                "`JinjaNodeFields` doesn't recognize `#[serde({})]` on `{field}`, so it serves the \
                 field cached. Add the attribute to `IGNORED_SERDE_ATTRIBUTES` in dbt-proc-macros \
                 if it doesn't change serialization, or support it there.",
                meta.to_token_stream()
            );
            quote_spanned! {meta.span()=>
                const _: () = {
                    #[deprecated(note = #note)]
                    const UNKNOWN_SERDE_ATTRIBUTE: () = ();
                    UNKNOWN_SERDE_ATTRIBUTE
                };
            }
        })
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

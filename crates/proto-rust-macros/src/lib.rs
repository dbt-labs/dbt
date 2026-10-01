extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Fields, Lit, Meta, Path, PathArguments, Token,
    Type, parse_macro_input, punctuated::Punctuated, spanned::Spanned,
};

/// Derive macro to implement Serde serialization and deserialization for
/// Protobuf enums (i32-backed) using their string names as well as
/// associated functions to serialize/deserialize i32 values as proto structs use
/// i32 instead of enums as enum field types. You can read prost docs for more info.
#[proc_macro_derive(ProtoEnumSerde, attributes(prost))]
pub fn derive_proto_enum_serde(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_proto_enum_serde(&input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive macro to implement a `new` associated function for Protobuf message
/// structs that takes parameters for all fields and returns a new instance.
/// For fields that are Protobuf enums (i32-backed with `#[prost(enumeration = "...")]`
/// attribute), the parameter type is the enum type instead of i32,
/// and the initializer converts the enum value(s) back to i32 as needed.
#[proc_macro_derive(ProtoNew, attributes(prost))]
pub fn derive_proto_new(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match impl_proto_new(&input) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn impl_proto_new(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_ident = &input.ident;

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    s.fields.span(),
                    "ProtoNew only supports structs with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "ProtoNew can only be derived for structs",
            ));
        }
    };

    // Build parameter list and field initializers
    let mut params = Vec::new();
    let mut inits = Vec::new();
    let mut arg_docs: Vec<String> = Vec::new();

    for field in fields {
        let field_ident = field
            .ident
            .clone()
            .ok_or_else(|| syn::Error::new(field.span(), "Unnamed field not supported"))?;

        let is_enum = find_prost_enumeration_attr(&field.attrs)?;

        let param_ident = field_ident.clone();

        // Collect doc for this field
        if let Some(doc) = extract_field_doc(&field.attrs) {
            let name = field_ident.to_string();
            let arg_doc = format!("* `{name}` - {doc}");
            arg_docs.push(arg_doc);
        }

        if let Some(enum_path) = is_enum {
            // Replace i32 in the outer shape with enum path for the parameter type,
            // and create an initializer that converts enum(s) back to i32.
            let (param_ty_tokens, init_expr) =
                map_enum_param_and_init(&field.ty, &enum_path, &param_ident)?;
            params.push(quote! { #param_ident: #param_ty_tokens });
            inits.push(quote! { #field_ident: #init_expr });
        } else {
            // Non-enum field: keep types as-is
            let ty = &field.ty;
            params.push(quote! { #param_ident: #ty });
            inits.push(quote! { #field_ident: #param_ident });
        }
    }

    // Build doc comment for the function
    let mut doc_lines = Vec::new();
    doc_lines.push(format!("Creates a new `{struct_ident}` message."));
    if !arg_docs.is_empty() {
        doc_lines.push(String::new());
        doc_lines.push(String::from("# Arguments"));
        doc_lines.extend(arg_docs);
    }
    let doc_attrs = doc_lines.into_iter().map(|line| quote! { #[doc = #line] });

    let output = quote! {
        impl #struct_ident {
            #(#doc_attrs)*
            #[allow(clippy::too_many_arguments)]
            pub fn new(
                #(#params),*
            ) -> Self {
                Self { #(#inits),* }
            }
        }
    };

    Ok(output)
}

fn find_prost_enumeration_attr(attrs: &[Attribute]) -> syn::Result<Option<Path>> {
    for attr in attrs {
        if !attr.path().is_ident("prost") {
            continue;
        }
        if let Meta::List(list) = &attr.meta
            && let Some(path) = scan_enumeration_in_tokens(&list.tokens)?
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn scan_enumeration_in_tokens(tokens: &TokenStream2) -> syn::Result<Option<Path>> {
    let mut iter = tokens.clone().into_iter();
    while let Some(tt) = iter.next() {
        match tt {
            TokenTree::Ident(ident) if ident == "enumeration" => {
                match iter.next() {
                    Some(TokenTree::Punct(p)) if p.as_char() == '=' => {}
                    _ => continue,
                }
                if let Some(TokenTree::Literal(lit)) = iter.next() {
                    let lit_ts: TokenStream2 = TokenTree::Literal(lit).into();
                    if let Lit::Str(s) = syn::parse2::<Lit>(lit_ts)? {
                        let path: Path = syn::parse_str(&s.value())?;
                        return Ok(Some(path));
                    }
                }
            }
            TokenTree::Group(g) => {
                if let Some(path) = scan_enumeration_in_tokens(&g.stream())? {
                    return Ok(Some(path));
                }
            }
            _ => {}
        }
    }
    Ok(None)
}

fn extract_field_doc(attrs: &[Attribute]) -> Option<String> {
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(nv) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
        {
            let text = s.value().trim().to_string();
            if !text.is_empty() {
                docs.push(text);
            }
        }
    }
    if docs.is_empty() {
        None
    } else {
        Some(docs.join(" ").replace("  ", " ").trim().to_string())
    }
}

// Map the field type to a parameter type (with enum instead of i32) and an initializer expression
// that converts enum(s) back to i32 as needed.
fn map_enum_param_and_init(
    field_ty: &Type,
    enum_path: &Path,
    param_ident: &syn::Ident,
) -> syn::Result<(proc_macro2::TokenStream, proc_macro2::TokenStream)> {
    // Plain i32
    if is_i32(field_ty) {
        let param_ty = quote! { #enum_path };
        let init = quote! { #param_ident as i32 };
        return Ok((param_ty, init));
    }

    // Option<i32>
    if let Some(inner) = extract_generic(field_ty, "Option")
        && is_i32(inner)
    {
        let param_ty = quote! { ::core::option::Option<#enum_path> };
        let init = quote! { #param_ident.map(|v| v as i32) };
        return Ok((param_ty, init));
    }

    // Vec<i32> and ::prost::alloc::vec::Vec<i32>
    if is_vec_of_i32(field_ty) {
        let vec_path = vec_path_for(field_ty);
        let param_ty = quote! { #vec_path<#enum_path> };
        let init = quote! { #param_ident.into_iter().map(|v| v as i32).collect() };
        return Ok((param_ty, init));
    }

    Err(syn::Error::new(
        field_ty.span(),
        "Unsupported enum field type shape; expected i32, Option<i32>, or Vec<i32>",
    ))
}

fn is_i32(ty: &Type) -> bool {
    match ty {
        Type::Path(tp) => {
            if let Some(seg) = tp.path.segments.last() {
                seg.ident == "i32" && matches!(seg.arguments, PathArguments::None)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn extract_generic<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    if let Type::Path(tp) = ty
        && let Some(seg) = tp.path.segments.last()
        && seg.ident == name
        && let PathArguments::AngleBracketed(ab) = &seg.arguments
        && ab.args.len() == 1
        && let syn::GenericArgument::Type(inner) = &ab.args[0]
    {
        return Some(inner);
    }
    None
}

fn is_vec_of_i32(ty: &Type) -> bool {
    if let Type::Path(tp) = ty
        && let Some(seg) = tp.path.segments.last()
        && seg.ident == "Vec"
        && let PathArguments::AngleBracketed(ab) = &seg.arguments
        && let Some(syn::GenericArgument::Type(inner)) = ab.args.first()
    {
        return is_i32(inner);
    }
    false
}

fn vec_path_for(ty: &Type) -> proc_macro2::TokenStream {
    // Preserve the original vector path (Vec or ::prost::alloc::vec::Vec)
    if let Type::Path(tp) = ty {
        let segments = tp.path.segments.clone();
        if let Some(last) = segments.last()
            && last.ident == "Vec"
        {
            let path = &tp.path;
            return quote! { #path };
        }
    }
    quote! { ::prost::alloc::vec::Vec }
}

fn impl_proto_enum_serde(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    match input.data {
        Data::Enum(_) => {}
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "ProtoEnumSerde can only be derived for enums",
            ));
        }
    }

    if !has_repr_i32(input) {
        // Not an i32-backed enum; skip deriving anything per request.
        return Ok(quote! {});
    }

    let enum_ident = &input.ident;

    let ser_impl = quote! {
        impl ::serde::Serialize for #enum_ident {
            fn serialize<S>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                serializer.serialize_str(self.as_str_name())
            }
        }
    };

    let de_impl = quote! {
        impl<'de> ::serde::Deserialize<'de> for #enum_ident {
            fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                struct __Visitor;
                impl<'de> ::serde::de::Visitor<'de> for __Visitor {
                    type Value = #enum_ident;
                    fn expecting(&self, formatter: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
                        formatter.write_str("string enum name")
                    }
                    fn visit_str<E>(self, value: &str) -> ::core::result::Result<Self::Value, E>
                    where
                        E: ::serde::de::Error,
                    {
                        #enum_ident::from_str_name(value).ok_or_else(|| E::custom(format!("invalid enum variant: {value}")))
                    }
                }
                deserializer.deserialize_str(__Visitor)
            }
        }
    };

    let assoc_fns = quote! {
        impl #enum_ident {
            pub fn serialize<S>(raw: &i32, serializer: S) -> ::core::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                let enum_value = #enum_ident::try_from(*raw).map_err(::serde::ser::Error::custom)?;
                ::serde::Serialize::serialize(&enum_value, serializer)
            }

            pub fn deserialize<'de, D>(deserializer: D) -> ::core::result::Result<i32, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let enum_value: #enum_ident = ::serde::Deserialize::deserialize(deserializer)?;
                Ok(enum_value as i32)
            }

            pub fn serialize_optional<S>(raw: &::core::option::Option<i32>, serializer: S) -> ::core::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                if let Some(raw) = raw {
                    let enum_value = #enum_ident::try_from(*raw).map_err(::serde::ser::Error::custom)?;
                    ::serde::Serialize::serialize(&enum_value, serializer)
                } else {
                    ::serde::Serialize::serialize(&::core::option::Option::<::prost::alloc::string::String>::None, serializer)
                }
            }

            pub fn deserialize_optional<'de, D>(deserializer: D) -> ::core::result::Result<::core::option::Option<i32>, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let enum_value: ::core::option::Option<#enum_ident> = ::serde::Deserialize::deserialize(deserializer)?;
                Ok(enum_value.map(|s| s as i32))
            }
        }
    };

    Ok(quote! {
        #ser_impl
        #de_impl
        #assoc_fns
    })
}

fn has_repr_i32(input: &DeriveInput) -> bool {
    for attr in &input.attrs {
        if attr.path().is_ident("repr")
            && let Ok(nested) =
                attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        {
            for meta in nested {
                if let Meta::Path(p) = meta
                    && p.is_ident("i32")
                {
                    return true;
                }
            }
        }
    }
    false
}

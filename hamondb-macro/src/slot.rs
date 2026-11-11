use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input, Ident, Token, Type,
};
use syn::{Attribute, Visibility};

struct SlotInput {
    attrs: Vec<Attribute>,
    vis: Visibility,
    name: Ident,
    ty: Type,
}

impl Parse for SlotInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let vis = input.parse()?;
        let name = input.parse()?;
        let _: Token![:] = input.parse()?;
        let ty = input.parse()?;
        Ok(Self {
            attrs,
            vis,
            name,
            ty,
        })
    }
}

pub(super) fn expand(input: TokenStream) -> TokenStream {
    let SlotInput {
        attrs,
        vis,
        name,
        ty,
    } = parse_macro_input!(input as SlotInput);
    let marker = format_ident!("__Slot_marker_{}", name);
    let name_str = name.to_string();
    quote! {
        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #vis struct #marker;

        #(#attrs)*
        #vis const #name: ::hamondb::Slot<#marker, #ty> =
          ::hamondb::Slot::new(concat!(module_path!(), "::", #name_str));
    }
    .into()
}

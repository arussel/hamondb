mod slot;
mod table;
mod resource;

use proc_macro::TokenStream;

#[proc_macro_derive(Table, attributes(primary_key, index, computed, skip))]
pub fn derive_table(_input: TokenStream) -> TokenStream {
    table::derive(_input).into()
}

#[proc_macro]
pub fn slot(input: TokenStream) -> TokenStream {
    slot::expand(input)
}

#[proc_macro]
pub fn resource(input: TokenStream) -> TokenStream {
    resource::expand(input)
}
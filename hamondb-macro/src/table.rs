use heck::ToShoutySnakeCase;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;

pub(super) fn derive(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    let name = &input.ident;
    let mut pk_type_opt = None;
    let mut column_definitions: Vec<proc_macro2::TokenStream> = vec![];
    let mut index_definitions: Vec<proc_macro2::TokenStream> = vec![];
    let mut index_registrations: Vec<proc_macro2::TokenStream> = vec![];
    let mut computed_column_registrations: Vec<proc_macro2::TokenStream> = vec![];
    let mut regular_column_registrations: Vec<proc_macro2::TokenStream> = vec![];
    let mut extractor_column_opt = None;
    match input.data {
        syn::Data::Struct(syn::DataStruct { ref fields, .. }) => {
            let mut index_counter: usize = 0;
            for (index, field) in fields.iter().enumerate() {
                let Some(field_ident) = field.ident.as_ref() else {
                    return syn::Error::new(
                        field.span(),
                        "Table derive only supports named-field structs",
                    )
                    .to_compile_error()
                    .into();
                };
                let field_name_ident = field_ident.to_string();
                let field_name = format_ident!("{}", field_name_ident);
                let field_name_shouty =
                    format_ident!("{}", field_name_ident.to_shouty_snake_case());
                let field_type = &field.ty;
                let mut ignore = false;
                let mut pk = false;
                let mut idx = false;
                let mut computed = false;
                for attr in &field.attrs {
                    if attr.path().is_ident("skip") {
                        ignore = true;
                    }
                    if attr.path().is_ident("primary_key") {
                        pk_type_opt = Some(field.ty.clone());
                        extractor_column_opt = Some(field_name_shouty.clone());
                        pk = true;
                    }
                    if attr.path().is_ident("index") {
                        let index_name = attr
                            .parse_args::<syn::Ident>()
                            .unwrap_or(format_ident!("{}_IDX", field_name_shouty));
                        index_definitions.push(quote! {
                            pub const #index_name: ::hamondb::Index<#name, #field_type> = ::hamondb::Index::new(&[#name::#field_name_shouty.id], #index_counter);
                        });
                        index_registrations.push(quote! {
                            Box::new(::hamondb::IndexImpl::<#name, #field_type>::new(&[#name::#field_name_shouty.id], |r| r.#field_name.clone(), #index_counter))   as Box<dyn ::hamondb::IndexMaintainer<#name>>
                        });
                        index_counter += 1;
                        idx = true;
                    }
                    if attr.path().is_ident("computed") {
                        let mut dependencies: Vec<proc_macro2::TokenStream> = Vec::new();
                        match &attr.meta {
                            syn::Meta::Path(_) => vec![], // #[computed]
                            syn::Meta::List(_) => match attr.parse_args_with(
                                Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated,
                            ) {
                                Ok(p) => p.into_iter().collect(),
                                Err(e) => return e.to_compile_error().into(),
                            },
                            _ => {
                                return syn::Error::new(
                                    attr.span(),
                                    "expected `#[computed]` or `#[computed(DEP1, DEP2, ...)]`",
                                )
                                .to_compile_error()
                                .into()
                            }
                        }
                        .iter()
                        .for_each(|dep| {
                            dependencies.push(quote! {
                                #name::#dep.id
                            })
                        });
                        computed_column_registrations.push(quote! {
                            Box::new(::hamondb::ComputedColumn::<#name, #field_type>{
                                column:#name::#field_name_shouty,
                                closure: Box::new(#name::#field_name),
                                computed_column_dependencies: std::collections::HashSet::<::hamondb::ColumnId>::from([#(#dependencies),*]),
                            }) as Box<dyn ::hamondb::AnyComputedColumn<#name>>
                        });
                        computed = true;
                    }
                }
                if (!ignore) {
                    column_definitions.push(quote! {
                    pub const #field_name_shouty: ::hamondb::Column<#name, #field_type> = ::hamondb::Column {
                        id: #index,
                        name:  concat!(stringify!(#name), "::", stringify!(#field_name_shouty)),
                        get: |r| r.#field_name.clone(),
                        set: |r, v| r.#field_name = v,
                    };
                });
                    if (!computed && !pk) {
                        regular_column_registrations.push(quote! {
                            Box::new(#name::#field_name_shouty) as Box<dyn ::hamondb::AnyColumn<#name>>
                        });
                    }
                }
                if (ignore) {
                    if pk {
                        return syn::Error::new(field.span(), "primary_key cannot be skip")
                            .to_compile_error()
                            .into();
                    }
                    if idx {
                        return syn::Error::new(field.span(), "index cannot be skip")
                            .to_compile_error()
                            .into();
                    }
                    if computed {
                        return syn::Error::new(field.span(), "computed column cannot be skip")
                            .to_compile_error()
                            .into();
                    }
                }
            }
        }
        _ => {}
    }
    let pk_type = match &pk_type_opt {
        Some(pk_type) => pk_type,
        None => {
            return syn::Error::new(input.span(), "No primary key found")
                .to_compile_error()
                .into()
        }
    };

    let extractor_column = match &extractor_column_opt {
        None => {
            return syn::Error::new(input.span(), "No primary key column found")
                .to_compile_error()
                .into()
        }
        Some(ec) => ec,
    };

    let indexes = quote! { vec![#(#index_registrations),*]};

    let computed_columns = quote! { vec![#(#computed_column_registrations),*]};

    let expanded = quote! {
        impl ::hamondb::Table for #name {
            type PrimaryKey = #pk_type;
        }

        impl #name {
          #(#column_definitions)*

          #(#index_definitions)*
        }

        impl ::hamondb::TableExtractor<#name> for #name {
            type PrimaryKeyExtractor = ::hamondb::Column<#name, #pk_type>;
            fn extract_primary_key() -> Self::PrimaryKeyExtractor {
                #name::#extractor_column
            }
            fn extract_indexes() -> Vec<Box<dyn ::hamondb::IndexMaintainer<#name>>> {
                #indexes
            }
            fn extract_computed_columns() -> Vec<Box<dyn ::hamondb::AnyComputedColumn<#name>>> {
                #computed_columns
            }

            fn extract_regular_columns() -> Vec<Box<dyn ::hamondb::AnyColumn<#name>>> {
                vec![#(#regular_column_registrations),*]
            }
        }
    };
    expanded.into()
}

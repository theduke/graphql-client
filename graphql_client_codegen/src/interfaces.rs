use failure;
use objects::GqlObjectField;
use proc_macro2::{Ident, Span, TokenStream};
use query::QueryContext;
use selection::{Selection, SelectionField, SelectionFragmentSpread, SelectionItem};
use shared::*;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashSet;
use unions::union_variants;

/// Represents an Interface type extracted from the schema.
#[derive(Debug, Clone, PartialEq)]
pub struct GqlInterface {
    /// The documentation for the interface. Extracted from the schema.
    pub description: Option<String>,
    /// The set of object types implementing this interface.
    pub implemented_by: HashSet<String>,
    /// The name of the interface. Should match 1-to-1 to its name in the GraphQL schema.
    pub name: String,
    /// The interface's fields. Analogous to object fields.
    pub fields: Vec<GqlObjectField>,
    pub is_required: Cell<bool>,
}

impl GqlInterface {
    /// filters the selection to keep only the fields that refer to the interface's own.
    ///
    /// This does not include the __typename field because it is translated into the `on` enum.
    fn object_selection(&self, selection: &Selection, query_context: &QueryContext) -> Selection {
        Selection(
            selection
                .0
                .iter()
                // Only keep what we can handle
                .filter(|f| match f {
                    SelectionItem::Field(f) => f.name != "__typename",
                    SelectionItem::FragmentSpread(SelectionFragmentSpread { fragment_name }) => {
                        // only if the fragment refers to the interface’s own fields (to take into account type-refining fragments)
                        let fragment = query_context
                            .fragments
                            .get(fragment_name)
                            .ok_or_else(|| format_err!("Unknown fragment: {}", &fragment_name))
                            // TODO: fix this
                            .unwrap();

                        fragment.on == self.name
                    }
                    SelectionItem::InlineFragment(_) => false,
                })
                .map(|a| (*a).clone())
                .collect(),
        )
    }

    fn union_selection(&self, selection: &Selection, query_context: &QueryContext) -> Selection {
        Selection(
            selection
                .0
                .iter()
                // Only keep what we can handle
                .filter(|f| match f {
                    SelectionItem::InlineFragment(_) => true,
                    SelectionItem::FragmentSpread(SelectionFragmentSpread { fragment_name }) => {
                        let fragment = query_context
                            .fragments
                            .get(fragment_name)
                            .ok_or_else(|| format_err!("Unknown fragment: {}", &fragment_name))
                            // TODO: fix this
                            .unwrap();

                        // only the fragments _not_ on the interface
                        fragment.on != self.name
                    }
                    SelectionItem::Field(SelectionField { name, .. }) => name == "__typename",
                })
                .map(|a| (*a).clone())
                .collect(),
        )
    }

    /// Create an empty interface. This needs to be mutated before it is useful.
    pub(crate) fn new(name: Cow<str>, description: Option<&str>) -> GqlInterface {
        GqlInterface {
            description: description.map(|d| d.to_owned()),
            name: name.into_owned(),
            implemented_by: HashSet::new(),
            fields: vec![],
            is_required: false.into(),
        }
    }

    /// The generated code for each of the selected field's types. See [shared::field_impls_for_selection].
    pub(crate) fn field_impls_for_selection(
        &self,
        context: &QueryContext,
        selection: &Selection,
        prefix: &str,
    ) -> Result<Vec<TokenStream>, failure::Error> {
        ::shared::field_impls_for_selection(
            &self.fields,
            context,
            &self.object_selection(selection, context),
            prefix,
        )
    }

    /// The code for the interface's corresponding struct's fields.
    pub(crate) fn response_fields_for_selection(
        &self,
        context: &QueryContext,
        selection: &Selection,
        prefix: &str,
    ) -> Result<Vec<TokenStream>, failure::Error> {
        response_fields_for_selection(
            &self.name,
            &self.fields,
            context,
            &self.object_selection(selection, context),
            prefix,
        )
    }

    /// Generate all the code for the interface.
    pub(crate) fn response_for_selection(
        &self,
        query_context: &QueryContext,
        selection: &Selection,
        prefix: &str,
    ) -> Result<TokenStream, failure::Error> {
        let name = Ident::new(&prefix, Span::call_site());
        let derives = query_context.response_derives();

        selection.extract_typename().ok_or_else(|| {
            format_err!(
                "Missing __typename in selection for the {} interface (type: {})",
                prefix,
                self.name
            )
        })?;

        let object_fields =
            self.response_fields_for_selection(query_context, &selection, prefix)?;

        let object_children = self.field_impls_for_selection(query_context, &selection, prefix)?;

        let union_selection = self.union_selection(&selection, &query_context);

        let (mut union_variants, union_children, used_variants) =
            union_variants(&union_selection, query_context, prefix)?;

        union_variants.extend(
            self.implemented_by
                .iter()
                .filter(|obj| used_variants.iter().find(|v| v == obj).is_none())
                .map(|v| {
                    let v = Ident::new(v, Span::call_site());
                    quote!(#v)
                }),
        );

        let attached_enum_name = Ident::new(&format!("{}On", name), Span::call_site());
        let (attached_enum, last_object_field) = if !union_variants.is_empty() {
            let attached_enum = quote! {
                #derives
                #[serde(tag = "__typename")]
                pub enum #attached_enum_name {
                    #(#union_variants,)*
                }
            };
            let last_object_field = quote!(#[serde(flatten)] pub on: #attached_enum_name,);
            (attached_enum, last_object_field)
        } else {
            (quote!(), quote!())
        };

        Ok(quote! {

            #(#object_children)*

            #(#union_children)*

            #attached_enum

            #derives
            pub struct #name {
                #(#object_fields,)*
                #last_object_field
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // to be improved
    #[test]
    fn union_selection_works() {
        let iface = GqlInterface {
            description: None,
            implemented_by: HashSet::new(),
            name: "MyInterface".into(),
            fields: vec![],
            is_required: Cell::new(true),
        };

        let context = QueryContext::new_empty();

        let typename_field = ::selection::SelectionItem::Field(::selection::SelectionField {
            alias: None,
            name: "__typename".to_string(),
            fields: Selection(vec![]),
        });
        let selection = Selection(vec![typename_field.clone()]);

        assert_eq!(
            iface.union_selection(&selection, &context),
            Selection(vec![typename_field])
        );
    }

    // to be improved
    #[test]
    fn object_selection_works() {
        let iface = GqlInterface {
            description: None,
            implemented_by: HashSet::new(),
            name: "MyInterface".into(),
            fields: vec![],
            is_required: Cell::new(true),
        };

        let context = QueryContext::new_empty();

        let typename_field = ::selection::SelectionItem::Field(::selection::SelectionField {
            alias: None,
            name: "__typename".to_string(),
            fields: Selection(vec![]),
        });
        let selection = Selection(vec![typename_field]);

        assert_eq!(
            iface.object_selection(&selection, &context),
            Selection(vec![])
        );
    }
}

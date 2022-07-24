use proc_macro2::Span;
use syn::spanned::Spanned;

use super::{parse_rust_type, RustType};
use crate::schema::{types::InputValue, types::TypeRef};

#[derive(Debug, PartialEq, Eq)]
pub enum CheckMode {
    OutputTypes,
    Flattening,
    Recursing,
    Spreading,
}

pub fn check_types_are_compatible<'a, T>(
    gql_type: &TypeRef<'a, T>,
    rust_type: &syn::Type,
    mode: CheckMode,
) -> Result<(), syn::Error> {
    match mode {
        CheckMode::Flattening => output_type_check(gql_type, rust_type, true)?,
        CheckMode::OutputTypes => output_type_check(gql_type, rust_type, false)?,
        CheckMode::Recursing => recursing_check(gql_type, rust_type)?,
        CheckMode::Spreading => {
            panic!("check_types_are_compatible shouldnt be called with CheckMode::Spreading")
        }
    }

    Ok(())
}

pub fn check_input_types_are_compatible<'a>(
    gql_type: &InputValue<'a>,
    rust_type: &syn::Type,
) -> Result<(), syn::Error> {
    input_type_check(&gql_type.value_type, gql_type.has_default, rust_type)?;

    Ok(())
}

pub fn check_spread_type(rust_type: &syn::Type) -> Result<(), syn::Error> {
    let parsed_type = parse_rust_type(rust_type);

    match parsed_type {
        RustType::Unknown => {
            // If we can't parse the type just ignore it - the compiler will still tell us if it's
            // wrong.
            Ok(())
        }
        RustType::Box(inner) => {
            // Box is a transparent container for the purposes of checking compatability
            // so just recurse
            check_spread_type(inner)
        }
        RustType::Optional(_) => Err(TypeValidationError::SpreadOnOption {
            span: rust_type.span(),
        }
        .into()),
        RustType::List(_) => Err(TypeValidationError::SpreadOnVec {
            span: rust_type.span(),
        }
        .into()),
        RustType::SimpleType => {
            // No way to tell if the given type is actually compatible,
            // but the rust compiler should help us with that.
            Ok(())
        }
    }
}

/// Returns the type inside `Option` if the type is `Option`.
/// Otherwise returns None
pub fn outer_type_is_option(rust_type: &syn::Type) -> Option<&syn::Type> {
    match parse_rust_type(rust_type) {
        RustType::Optional(inner) => Some(inner),
        RustType::List(_) => None,
        RustType::Box(inner) => outer_type_is_option(inner),
        RustType::SimpleType => None,
        RustType::Unknown => None,
    }
}

fn output_type_check<'a, T>(
    gql_type: &TypeRef<'a, T>,
    rust_type: &syn::Type,
    flattening: bool,
) -> Result<(), TypeValidationError> {
    match (&gql_type, parse_rust_type(rust_type)) {
        (_, RustType::Box(inner)) => {
            // Box is a transparent container for the purposes of checking compatability
            // so just recurse
            output_type_check(gql_type, inner, flattening)
        }
        (TypeRef::Nullable(inner_gql), RustType::Optional(inner)) => {
            output_type_check(inner_gql, inner, flattening)
        }
        (TypeRef::Nullable(_), RustType::Unknown) => Err(TypeValidationError::UnknownType {
            span: rust_type.span(),
        }),
        (TypeRef::Nullable(inner_gql), _) if flattening => {
            // If we're flattening then we should still check the inner types line up...
            output_type_check(inner_gql, rust_type, flattening)
        }
        (TypeRef::Nullable(_), _) => Err(TypeValidationError::FieldIsOptional {
            provided_type: rust_type.to_string(),

            span: rust_type.span(),
        }),
        (gql_type, RustType::Optional(inner)) => {
            // It should be fine for an output field to be `Option` if the schema
            // type isn't nullable.  it's pointless, but won't crash so
            // we just need to check the inner types
            output_type_check(gql_type, inner, flattening)
        }
        (TypeRef::List(item_type), RustType::List(inner)) => {
            output_type_check(item_type.as_ref(), inner, flattening)
        }
        (TypeRef::List(_), _) => {
            // If the server is going to return a list we can't not have a Vec here...
            Err(TypeValidationError::FieldIsList {
                provided_type: rust_type.to_string(),
                span: rust_type.span(),
            })
        }
        (_, RustType::List(inner)) => Err(TypeValidationError::FieldIsNotList {
            provided_type: inner.to_string(),
            span: rust_type.span(),
        }),
        (TypeRef::Named(_, _, _), RustType::SimpleType) => Ok(()),
        (TypeRef::Named(_, _, _), RustType::Unknown) => {
            // This is probably some type with generic params.
            // But we've satisfied any list/nullable requirements by here
            // so should probably just allow it
            Ok(())
        }
    }
}

fn input_type_check<'a, T>(
    gql_type: &TypeRef<'a, T>,
    has_default: bool,
    rust_type: &syn::Type,
) -> Result<(), TypeValidationError> {
    let parsed_type = parse_rust_type(rust_type);

    match (&gql_type, parsed_type) {
        (gql_type, RustType::Box(inner)) => {
            // Box is a transparent container for the purposes of checking compatability
            // so just recurse
            input_type_check(gql_type, has_default, inner)
        }
        (TypeRef::Nullable(inner_gql), RustType::Optional(inner)) => {
            input_type_check(inner_gql, false, inner)
        }
        (TypeRef::Nullable(_), RustType::Unknown) => Err(TypeValidationError::UnknownType {
            span: rust_type.span(),
        }),
        (TypeRef::Nullable(inner_gql), _) => {
            // For input types its fine if a field isn't actually optional.
            // We just need to check that the inner types line up.
            input_type_check(inner_gql, false, rust_type)
        }
        (_, RustType::Optional(inner)) if has_default => {
            // If an input type is required but has a default then
            // it's ok for it to be wrapped in option.
            input_type_check(gql_type, false, inner)
        }
        (_, RustType::Optional(inner)) => Err(TypeValidationError::FieldIsRequired {
            provided_type: inner.to_string(),
            span: rust_type.span(),
        }),
        (TypeRef::List(item_type), RustType::List(inner)) => {
            input_type_check(item_type.as_ref(), false, inner)
        }
        (TypeRef::List(item_type), _) => {
            // For input types its fine to provide a single item instead of a list.
            // We just need to check that the inner types line up.
            input_type_check(item_type, false, rust_type)
        }
        (_, RustType::List(inner)) => Err(TypeValidationError::FieldIsNotList {
            provided_type: inner.to_string(),
            span: rust_type.span(),
        }),
        (TypeRef::Named(_, _, _), RustType::SimpleType) => Ok(()),
        (TypeRef::Named(_, _, _), RustType::Unknown) => {
            // This is probably some type with generic params.
            // But we've satisfied any list/nullable requirements by here
            // so should probably just allow it
            Ok(())
        }
    }
}

fn recursing_check<'a, T>(
    gql_type: &TypeRef<'a, T>,
    rust_type: &syn::Type,
) -> Result<(), TypeValidationError> {
    let parsed_type = parse_rust_type(rust_type);

    if let RustType::Unknown = parsed_type {
        return Err(TypeValidationError::UnknownType {
            span: rust_type.span(),
        });
    };

    if let TypeRef::Nullable(_) = gql_type {
        // If the field is nullable then we just defer to the normal checks.
        return output_type_check(gql_type, rust_type, false);
    };

    if let RustType::Optional(inner_rust_type) = parsed_type {
        output_type_check(gql_type, inner_rust_type, false)
    } else {
        Err(TypeValidationError::RecursiveFieldWithoutOption {
            provided_type: rust_type.to_string(),
            span: rust_type.span(),
        })
    }
}

#[derive(Debug)]
enum TypeValidationError {
    FieldIsOptional { provided_type: String, span: Span },
    FieldIsRequired { provided_type: String, span: Span },
    FieldIsList { provided_type: String, span: Span },
    FieldIsNotList { provided_type: String, span: Span },
    UnknownType { span: Span },
    RecursiveFieldWithoutOption { provided_type: String, span: Span },
    SpreadOnOption { span: Span },
    SpreadOnVec { span: Span },
}

impl From<TypeValidationError> for syn::Error {
    fn from(err: TypeValidationError) -> Self {
        let span = err.span();
        let message = match err {
            TypeValidationError::FieldIsOptional { provided_type, .. } =>
                format!("This field is nullable but you're not wrapping the type in Option.  Did you mean Option<{}>", provided_type),
            TypeValidationError::FieldIsRequired { provided_type, .. } =>
                format!("This field is not nullable but you're wrapping the type in Option.  Did you mean {}", provided_type),
            TypeValidationError::FieldIsList { provided_type, .. } => {
                format!("This field is a list but you're not wrapping the type in Vec.  Did you mean Vec<{}>", provided_type)
            },
            TypeValidationError::FieldIsNotList { provided_type, .. } => {
                format!("This field is not a list but you're wrapping the type in Vec.  Did you mean {}", provided_type)
            },
            TypeValidationError::UnknownType { .. } => {
                "Cynic does not understand this type. Only un-parameterised types, Vecs, Options & Box are accepted currently.".to_string()
            },
            TypeValidationError::RecursiveFieldWithoutOption { provided_type, .. } => {
                format!("Recursive types must be wrapped in Option.  Did you mean Option<{}>", provided_type)
            }
            TypeValidationError::SpreadOnOption { .. } => "You can't spread on an optional field".to_string(),
            TypeValidationError::SpreadOnVec { .. } => "You can't spread on a list field".to_string(),
        };

        syn::Error::new(span, message)
    }
}

impl From<TypeValidationError> for crate::Errors {
    fn from(err: TypeValidationError) -> Self {
        crate::Errors::from(syn::Error::from(err))
    }
}

impl TypeValidationError {
    fn span(&self) -> Span {
        match self {
            TypeValidationError::FieldIsOptional { span, .. } => *span,
            TypeValidationError::FieldIsRequired { span, .. } => *span,
            TypeValidationError::FieldIsList { span, .. } => *span,
            TypeValidationError::FieldIsNotList { span, .. } => *span,
            TypeValidationError::UnknownType { span } => *span,
            TypeValidationError::RecursiveFieldWithoutOption { span, .. } => *span,
            TypeValidationError::SpreadOnOption { span } => *span,
            TypeValidationError::SpreadOnVec { span } => *span,
        }
    }
}

trait SynTypeExt {
    fn to_string(&self) -> String;
}

impl SynTypeExt for syn::Type {
    fn to_string(&self) -> String {
        quote::quote! { #self }.to_string().replace(' ', "")
    }
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use super::*;
    use crate::schema::TypeIndex;

    use assert_matches::assert_matches;
    use quote::quote;
    use rstest::rstest;
    use syn::parse_quote;

    type TypeRef<'a> = super::TypeRef<'a, ()>;

    #[test]
    fn test_output_type_check() {
        let required_field = TypeRef::Named("test", TypeIndex::empty(), PhantomData);
        let optional_field = TypeRef::Nullable(Box::new(required_field.clone()));

        assert_matches!(
            output_type_check(
                &required_field,
                &syn::parse2(quote! { i32 }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &required_field,
                &syn::parse2(quote! { DateTime<Utc> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &optional_field,
                &syn::parse2(quote! { Option<i32> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &optional_field,
                &syn::parse2(quote! { i32 }).unwrap(),
                false
            ),
            Err(TypeValidationError::FieldIsOptional {
                provided_type,
                ..
            }) => {
                assert_eq!(provided_type, "i32")
            }
        );
        assert_matches!(
            output_type_check(
                &required_field,
                &syn::parse2(quote! { Option<i32> }).unwrap(),
                false
            ),
            Ok(())
        );
    }

    #[test]
    fn test_output_type_list_validation() {
        let named = TypeRef::Named("test", TypeIndex::empty(), PhantomData);
        let list = TypeRef::List(Box::new(named.clone()));
        let optional_list = TypeRef::Nullable(Box::new(TypeRef::List(Box::new(named.clone()))));
        let option_list_option = TypeRef::Nullable(Box::new(TypeRef::List(Box::new(
            TypeRef::Nullable(Box::new(named.clone())),
        ))));

        assert_matches!(
            output_type_check(&list, &syn::parse2(quote! { Vec<i32> }).unwrap(), false),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &optional_list,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &option_list_option,
                &syn::parse2(quote! { Option<Vec<Option<i32>>> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &list,
                &syn::parse2(quote! { Vec<DateTime<Utc>> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &optional_list,
                &syn::parse2(quote! { Option<Vec<DateTime<Utc>>> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &option_list_option,
                &syn::parse2(quote! { Option<Vec<Option<DateTime<Utc>>>> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &list,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
                false
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(&list, &syn::parse2(quote! { i32 }).unwrap(), false),
            Err(TypeValidationError::FieldIsList { provided_type, .. }) => {
                assert_eq!(provided_type, "i32")
            }
        );
        assert_matches!(
            output_type_check(
                &optional_list,
                &syn::parse2(quote! { Vec<i32> }).unwrap(),
                false
            ),
            Err(TypeValidationError::FieldIsOptional { provided_type, .. }) => {
                assert_eq!(provided_type, "Vec<i32>")
            }
        );
        assert_matches!(
            output_type_check(
                &option_list_option,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
                false
            ),
            Err(TypeValidationError::FieldIsOptional { provided_type, .. }) => {
                assert_eq!(provided_type, "i32")
            }
        );
        assert_matches!(
            output_type_check(
                &option_list_option,
                &syn::parse2(quote! { Option<DateTime<Vec<Option<i32>>>> }).unwrap(),
                false
            ),
            Err(_)
        );
    }

    #[test]
    fn test_validation_when_flattening() {
        let named = TypeRef::Named("test", TypeIndex::empty(), PhantomData);
        let list = TypeRef::List(Box::new(named.clone()));
        let optional_list = TypeRef::Nullable(Box::new(TypeRef::List(Box::new(named.clone()))));
        let option_list_option = TypeRef::Nullable(Box::new(TypeRef::List(Box::new(
            TypeRef::Nullable(Box::new(named.clone())),
        ))));

        assert_matches!(
            output_type_check(
                &option_list_option,
                &syn::parse2(quote! { Vec<i32> }).unwrap(),
                true
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &option_list_option,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
                true
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &optional_list,
                &syn::parse2(quote! { Vec<i32> }).unwrap(),
                true
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &list,
                &syn::parse2(quote! { Vec<Option<i32>> }).unwrap(),
                true
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(
                &list,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
                true
            ),
            Ok(())
        );
        assert_matches!(
            output_type_check(&list, &syn::parse2(quote! { Option<i32> }).unwrap(), true),
            Err(TypeValidationError::FieldIsList { provided_type, .. }) => {
                assert_eq!(provided_type, "i32")
            }
        );
        assert_matches!(
            output_type_check(
                &optional_list,
                &syn::parse2(quote! { DateTime<Vec<i32>> }).unwrap(),
                true
            ),
            Err(TypeValidationError::UnknownType { .. })
        );
    }

    #[test]
    fn test_input_type_validation() {
        let required_field = TypeRef::Named("test", TypeIndex::empty(), PhantomData);
        let optional_field = TypeRef::Nullable(Box::new(required_field.clone()));

        assert_matches!(
            input_type_check(
                &required_field,
                false,
                &syn::parse2(quote! { i32 }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &required_field,
                false,
                &syn::parse2(quote! { DateTime<Utc> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &optional_field,
                false,
                &syn::parse2(quote! { Option<i32> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &optional_field,
                false,
                &syn::parse2(quote! { i32 }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &required_field,
                false,
                &syn::parse2(quote! { Option<i32> }).unwrap(),
            ),
            Err(TypeValidationError::FieldIsRequired {provided_type, ..}) => {
                assert_eq!(provided_type, "i32")
            }
        );
    }

    #[test]
    fn test_input_type_validation_with_default() {
        let required_field = TypeRef::Named("test", TypeIndex::empty(), PhantomData);
        let optional_field = TypeRef::Nullable(Box::new(required_field.clone()));

        assert_matches!(
            input_type_check(&required_field, true, &syn::parse2(quote! { i32 }).unwrap(),),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &required_field,
                true,
                &syn::parse2(quote! { DateTime<Utc> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &optional_field,
                true,
                &syn::parse2(quote! { Option<i32> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(&optional_field, true, &syn::parse2(quote! { i32 }).unwrap(),),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &required_field,
                true,
                &syn::parse2(quote! { Option<i32> }).unwrap(),
            ),
            Ok(())
        );
    }

    #[test]
    fn test_input_type_list_validation() {
        let named = TypeRef::Named("test", TypeIndex::empty(), PhantomData);
        let list = TypeRef::List(Box::new(named.clone()));
        let optional_list = TypeRef::Nullable(Box::new(TypeRef::List(Box::new(named.clone()))));
        let option_list_option = TypeRef::Nullable(Box::new(TypeRef::List(Box::new(
            TypeRef::Nullable(Box::new(named.clone())),
        ))));

        assert_matches!(
            input_type_check(&list, false, &syn::parse2(quote! { Vec<i32> }).unwrap(),),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &optional_list,
                false,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &option_list_option,
                false,
                &syn::parse2(quote! { Option<Vec<Option<i32>>> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &list,
                false,
                &syn::parse2(quote! { Vec<DateTime<Utc>> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &optional_list,
                false,
                &syn::parse2(quote! { Option<Vec<DateTime<Utc>>> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &option_list_option,
                false,
                &syn::parse2(quote! { Option<Vec<Option<DateTime<Utc>>>> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(&list, false, &syn::parse2(quote! { i32 }).unwrap(),),
            Ok(())
        );
        assert_matches!(
            input_type_check(&optional_list, false, &syn::parse2(quote! { i32 }).unwrap(),),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &optional_list,
                false,
                &syn::parse2(quote! { Vec<i32> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &option_list_option,
                false,
                &syn::parse2(quote! { Option<Vec<i32>> }).unwrap(),
            ),
            Ok(())
        );
        assert_matches!(
            input_type_check(
                &option_list_option,
                false,
                &syn::parse2(quote! { Option<DateTime<Vec<Option<i32>>>> }).unwrap(),
            ),
            Err(TypeValidationError::UnknownType { .. })
        );
    }

    #[rstest(graphql_field, rust_field,
        case::required_t(
            TypeRef::Named("T", TypeIndex::empty(), PhantomData),
            parse_quote! { Option<Box<T>> }
        ),

        case::optional_t(
            TypeRef::Nullable(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData))),
            parse_quote! { Option<T> }
        ),

        case::option_vec_required_t(
            TypeRef::Nullable(Box::new(
                TypeRef::List(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData)))
            )),
            parse_quote! { Option<Vec<T>> }
        ),

        case::required_vec_required_t(
            TypeRef::List(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData))),
            parse_quote! { Option<Vec<T>> }
        ),
    )]
    fn test_recurse_validation_ok(graphql_field: TypeRef<'_>, rust_field: syn::Type) {
        assert_matches!(
            check_types_are_compatible(&graphql_field, &rust_field, CheckMode::Recursing),
            Ok(())
        );
    }

    #[rstest(graphql_field, rust_field,
        case::required_t_box(
            TypeRef::Named("T", TypeIndex::empty(), PhantomData),
            parse_quote! { Box<T> }
        ),
        case::required_t_standalone(
            TypeRef::Named("T", TypeIndex::empty(), PhantomData),
            parse_quote! { T }
        ),

        case::optional_t_standalone(
            TypeRef::Nullable(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData))),
            parse_quote! { T }
        ),
        case::optional_t_box(
            TypeRef::Nullable(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData))),
            parse_quote! { Box<T> }
        ),

        case::option_vec_required_t(
            TypeRef::Nullable(Box::new(
                TypeRef::List(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData)))
            )),
            parse_quote! { Vec<T> }
        ),
        case::option_vec_required_t(
            TypeRef::Nullable(Box::new(
                TypeRef::List(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData)))
            )),
            parse_quote! { Vec<Option<T>> }
        ),

        case::required_vec_required_t(
            TypeRef::List(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData))),
            parse_quote! { Vec<T> }
        ),
        case::required_vec_required_t_no_vec(
            TypeRef::List(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData))),
            parse_quote! { T }
        ),

        case::required_vec_optional_t_no_vec(
            TypeRef::List(Box::new(
                TypeRef::Nullable(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData)))
            )),
            parse_quote! { Option<T> }
        ),
        case::required_vec_optional_t_wrong_nesting(
            TypeRef::List(Box::new(
                TypeRef::Nullable(Box::new(TypeRef::Named("T", TypeIndex::empty(), PhantomData)))
            )),
            parse_quote! { Option<Vec<T>> }
        ),
    )]
    fn test_recurse_validation_fail(graphql_field: TypeRef<'_>, rust_field: syn::Type) {
        assert_matches!(
            check_types_are_compatible(&graphql_field, &rust_field, CheckMode::Recursing),
            Err(_)
        );
    }
}
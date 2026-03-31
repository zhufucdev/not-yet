use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{LitInt, parse_macro_input};

/// Generates `Authenticator` impls on `PriorityAuthenticator<(A0, A1, ...)>`
/// for every arity from 2 up to and including `max`.
///
/// Usage (in priority.rs):
/// ```ignore
/// impl_priority_authenticators!(8);
/// ```
#[proc_macro]
pub fn impl_priority_authenticators(input: TokenStream) -> TokenStream {
    let n = parse_macro_input!(input as LitInt)
        .base10_parse::<usize>()
        .expect("expected a usize literal");

    assert!(n >= 2, "max arity must be at least 2");

    let output: TokenStream = generate_impl(n).into();
    output
}

fn generate_impl(n: usize) -> proc_macro2::TokenStream {
    // Produce type idents: A0, A1, …, A(n-1)
    let types: Vec<syn::Ident> = (0..n)
        .map(|i| syn::Ident::new(&format!("A{i}"), Span::call_site()))
        .collect();

    let try_types = &types[..n - 1]; // all but last
    let last_type = &types[n - 1]; // fallback

    let try_indices: Vec<syn::Index> = (0..n - 1).map(syn::Index::from).collect();
    let last_index = syn::Index::from(n - 1);

    quote! {
        impl<#(#types,)*> From<(#(#types,)*)> for PriorityAuthenticator<(#(#types,)*)> {
            fn from(members: (#(#types,)*)) -> Self {
                Self(members)
            }
        }

        impl<UserId, Level, #(#types),*>
            crate::authenticator::Authenticator
            for PriorityAuthenticator<(#(#types,)*)>
        where
            UserId: Send + Sync + Clone + 'static,
            Level:  Send + Sync + Clone + 'static,
            #(
                #try_types: crate::authenticator::Authenticator<
                    UserId = UserId,
                    Level  = Level,
                > + Send + Sync + 'static,
                <#try_types as crate::authenticator::Authenticator>::Error:
                    Into<anyhow::Error>,
            )*
            #last_type: crate::authenticator::Authenticator<
                UserId = UserId,
                Level  = Level,
            > + Send + Sync + 'static,
            <#last_type as crate::authenticator::Authenticator>::Error:
                Into<anyhow::Error>,
        {
            type UserId = UserId;
            type Level  = Level;
            type Error  = anyhow::Error;

            async fn get_access(
                &self,
                user_id: &Self::UserId,
            ) -> Result<crate::authenticator::Access<Self::Level>, Self::Error> {
                #(
                    {
                        let member: &#try_types = &self.0.#try_indices;
                        if let Ok(crate::authenticator::Access::Granted(level)) = member.get_access(user_id).await {
                            return Ok(crate::authenticator::Access::Granted(level));
                        }
                    }
                )*
                let last: &#last_type = &self.0.#last_index;
                last.get_access(user_id).await.map_err(Into::into)
            }

            async fn grant(
                &self,
                user_id: Self::UserId,
                level: Self::Level,
            ) -> Result<(), Self::Error> {
                #(
                    {
                        let member: &#try_types = &self.0.#try_indices;
                        if let Ok(()) = member.grant(user_id.clone(), level.clone()).await {
                            return Ok(());
                        }
                    }
                )*
                let last: &#last_type = &self.0.#last_index;
                last.grant(user_id, level).await.map_err(Into::into)
            }
        }
    }
}

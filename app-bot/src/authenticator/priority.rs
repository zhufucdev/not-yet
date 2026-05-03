use priority_authenticator::impl_priority_authenticators;

/// A static, tuple-backed authenticator that tries each member in order.
/// No heap allocation or dynamic dispatch — the full type is encoded in `T`.
///
/// Construct via `From`:
/// ```ignore
/// let auth = PriorityAuthenticator::from((
///     JwtAuthenticator::new(…),
///     DbAuthenticator::new(…),
///     FallbackAuthenticator::new(…),
/// ));
/// ```

pub struct PriorityAuthenticator<T>(T);
// Generates Authenticator impls for arities 2–8. Raise the ceiling as needed.
impl_priority_authenticators!(2);

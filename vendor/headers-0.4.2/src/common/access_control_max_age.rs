use crate::time::Duration;

use crate::util::Seconds;

/// `Access-Control-Max-Age` header, part of
/// [CORS](http://www.w3.org/TR/cors/#access-control-max-age-response-header)
///
/// The `Access-Control-Max-Age` header indicates how long the results of a
/// preflight request can be cached in a preflight result cache.
///
/// # ABNF
///
/// ```text
/// Access-Control-Max-Age = \"Access-Control-Max-Age\" \":\" delta-seconds
/// ```
///
/// # Example values
///
/// * `531`
///
/// # Examples
///
/// ```
/// use crate::time::Duration;
/// use headers::AccessControlMaxAge;
///
/// let max_age = AccessControlMaxAge::from(Duration::from_secs(531));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccessControlMaxAge(Seconds);

derive_header! {
    AccessControlMaxAge(_),
    name: ACCESS_CONTROL_MAX_AGE
}

impl From<Duration> for AccessControlMaxAge {
    fn from(dur: Duration) -> AccessControlMaxAge {
        AccessControlMaxAge(dur.into())
    }
}

impl From<AccessControlMaxAge> for Duration {
    fn from(acma: AccessControlMaxAge) -> Duration {
        acma.0.into()
    }
}

#[cfg(target_os = "trueos")]
#[allow(unused_imports)]
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};

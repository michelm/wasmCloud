use tracing::warn;

/// Feature flags to enable experimental functionality in the host. Flags are disabled
/// by default and must be explicitly enabled.
#[derive(Copy, Clone, Debug, Default)]
pub struct Features {
    /// Enable the built-in HTTP server capability provider
    /// that can be started with the reference wasmcloud+builtin://http-server
    pub(crate) builtin_http: bool,
    /// Enable the built-in NATS Messaging capability provider
    /// that can be started with the reference wasmcloud+builtin://messaging-nats
    pub(crate) builtin_messaging: bool,
}

impl Features {
    /// Enable the built-in HTTP server capability provider
    pub fn with_builtin_http() -> Self {
        Self {
            builtin_http: true,
            ..Default::default()
        }
    }

    /// Enable the built-in NATS messaging capability provider
    pub fn with_builtin_messaging() -> Self {
        Self {
            builtin_messaging: true,
            ..Default::default()
        }
    }
}

/// This enables unioning feature flags together
impl std::ops::BitOr for Features {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self {
            builtin_http: self.builtin_http || rhs.builtin_http,
            builtin_messaging: self.builtin_messaging || rhs.builtin_messaging,
        }
    }
}

/// Allow for summing over a collection of feature flags
impl std::iter::Sum for Features {
    fn sum<I: Iterator<Item = Self>>(mut iter: I) -> Self {
        // Grab the first set of flags, fall back on defaults (all disabled)
        let first = iter.next().unwrap_or_default();
        iter.fold(first, |a, b| a | b)
    }
}

/// Parse a feature flag from a string, enabling the feature if the string matches
impl From<&str> for Features {
    fn from(s: &str) -> Self {
        match &*s.to_ascii_lowercase() {
            "builtin-http" | "builtin_http" => Self::with_builtin_http(),
            "builtin-messaging" | "builtin_messaging" => Self::with_builtin_messaging(),
            _ => {
                warn!(%s, "unknown feature flag");
                Features::default()
            }
        }
    }
}

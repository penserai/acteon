use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! newtype_string {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
        #[cfg_attr(feature = "openapi", schema(value_type = String))]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Create a new instance from a string value.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Return the inner string as a str slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

newtype_string!(Namespace, "A logical namespace for grouping actions.");
newtype_string!(TenantId, "A tenant identifier for multi-tenant isolation.");
newtype_string!(ActionId, "A unique action identifier.");
newtype_string!(ProviderId, "Identifies an action provider.");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtype_from_str() {
        let ns = Namespace::from("notifications");
        assert_eq!(ns.as_str(), "notifications");
        assert_eq!(&*ns, "notifications");
    }

    #[test]
    fn newtype_from_string() {
        let tenant = TenantId::from("tenant-42".to_string());
        assert_eq!(tenant.to_string(), "tenant-42");
    }

    #[test]
    fn newtype_serde_roundtrip() {
        let id = ActionId::new("act-123");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"act-123\"");
        let back: ActionId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn newtype_display() {
        let p = ProviderId::new("email-provider");
        assert_eq!(format!("{p}"), "email-provider");
    }
}

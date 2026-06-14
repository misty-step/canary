//! Typed Canary identifiers.
//!
//! The Elixir service used string IDs with stable prefixes such as `ERR-` and
//! `INC-`. The Rust rewrite makes those prefixes part of the type system so an
//! agent cannot accidentally pass a monitor ID where an error ID is required.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

const ALPHABET: [char; 36] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i',
    'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
];
const BODY_LEN: usize = 12;

/// Known Canary ID prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// Error row ID, `ERR-*`.
    Error,
    /// Incident row ID, `INC-*`.
    Incident,
    /// Timeline/service event row ID, `EVT-*`.
    Event,
    /// HTTP health target row ID, `TGT-*`.
    Target,
    /// Non-HTTP monitor row ID, `MON-*`.
    Monitor,
    /// Monitor check-in row ID, `CHK-*`.
    CheckIn,
    /// Webhook row ID, `WHK-*`.
    Webhook,
    /// API key row ID, `KEY-*`.
    Key,
    /// Annotation row ID, `ANN-*`.
    Annotation,
    /// Remediation claim row ID, `CLM-*`.
    Claim,
    /// Webhook delivery ID, `DLV-*`.
    Delivery,
}

impl Prefix {
    /// Return the wire prefix used by the existing API and database rows.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERR",
            Self::Incident => "INC",
            Self::Event => "EVT",
            Self::Target => "TGT",
            Self::Monitor => "MON",
            Self::CheckIn => "CHK",
            Self::Webhook => "WHK",
            Self::Key => "KEY",
            Self::Annotation => "ANN",
            Self::Claim => "CLM",
            Self::Delivery => "DLV",
        }
    }
}

/// Invalid typed ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected}- id, got {actual:?}")]
pub struct IdError {
    expected: &'static str,
    actual: String,
}

/// Define a prefixed newtype ID.
macro_rules! prefixed_id {
    ($name:ident, $prefix:expr) => {
        #[doc = concat!("Typed `", stringify!($name), "` string identifier.")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Generate a new ID with the type's prefix.
            pub fn generate() -> Self {
                let body = nanoid::nanoid!(BODY_LEN, &ALPHABET);
                Self(format!("{}-{body}", $prefix.as_str()))
            }

            /// Borrow the wire/database representation.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the typed ID into its wire/database representation.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let expected = $prefix.as_str();
                if value
                    .strip_prefix(expected)
                    .and_then(|rest| rest.strip_prefix('-'))
                    .is_some_and(|body| {
                        body.len() == BODY_LEN && body.chars().all(|c| ALPHABET.contains(&c))
                    })
                {
                    Ok(Self(value.to_owned()))
                } else {
                    Err(IdError {
                        expected,
                        actual: value.to_owned(),
                    })
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_str(&value).map_err(de::Error::custom)
            }
        }
    };
}

prefixed_id!(ErrorId, Prefix::Error);
prefixed_id!(IncidentId, Prefix::Incident);
prefixed_id!(EventId, Prefix::Event);
prefixed_id!(TargetId, Prefix::Target);
prefixed_id!(MonitorId, Prefix::Monitor);
prefixed_id!(CheckInId, Prefix::CheckIn);
prefixed_id!(WebhookId, Prefix::Webhook);
prefixed_id!(ApiKeyId, Prefix::Key);
prefixed_id!(AnnotationId, Prefix::Annotation);
prefixed_id!(ClaimId, Prefix::Claim);
prefixed_id!(DeliveryId, Prefix::Delivery);

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn generated_ids_have_stable_prefix_and_body_shape() {
        let id = ErrorId::generate();
        assert!(id.as_str().starts_with("ERR-"));
        assert_eq!(id.as_str().len(), "ERR-".len() + BODY_LEN);
    }

    #[test]
    fn parser_rejects_wrong_prefix() {
        let result = ErrorId::from_str("INC-123456789abc");
        assert!(matches!(
            result,
            Err(IdError {
                expected: "ERR",
                ..
            })
        ));
    }

    #[test]
    fn serde_round_trip_preserves_wire_id() {
        let id = WebhookId::generate();
        let encoded = serde_json::to_string(&id);
        assert!(encoded.is_ok());
        let decoded = serde_json::from_str::<WebhookId>(&encoded.unwrap_or_default());
        assert!(decoded.is_ok());
        if let Ok(decoded) = decoded {
            assert_eq!(decoded, id);
        }
    }
}

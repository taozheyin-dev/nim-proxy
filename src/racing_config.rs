//! The validated, opt-in racing policy shared by storage and request dispatch.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Every participant consumes a separately governed upstream request. Policies
/// race at most five copies of the client-selected model and are disabled
/// unless persisted. Retired multi-model policies parse only for migration errors.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RaceStrategy {
    /// Race identical request copies; zero delay launches all copies immediately.
    // All named-variant fields are required. Matching property caps express
    // deny_unknown_fields in utoipa's internally tagged enum schemas.
    #[schema(max_properties = 3)]
    SameModel {
        #[schema(minimum = 1, maximum = 5)]
        attempts: usize,
        #[schema(minimum = 0, maximum = 300000)]
        hedge_delay_ms: u64,
    },
    /// Retired multi-model policy. Replace with `same_model` before enabling.
    #[schema(max_properties = 2)]
    FullParallel { models: Vec<String> },
    /// Retired multi-model policy. Replace with `same_model` before enabling.
    #[schema(max_properties = 4)]
    Smart {
        primary_model: String,
        fallback_models: Vec<String>,
        primary_timeout_ms: u64,
    },
    /// Retired multi-model policy. Replace with `same_model` before enabling.
    #[schema(max_properties = 2)]
    TtftRacing { models: Vec<String> },
    /// No racing. An enabled durable policy may not use this variant.
    #[default]
    None,
}

impl RaceStrategy {
    /// Validate an enabled policy at every settings write and at startup.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::SameModel {
                attempts,
                hedge_delay_ms,
            } => {
                if !(1..=5).contains(attempts) {
                    return Err("racing attempts must be between 1 and 5".into());
                }
                if *hedge_delay_ms > 300_000 {
                    return Err("racing hedge_delay_ms must be between 0 and 300000".into());
                }
                Ok(())
            }
            Self::FullParallel { .. } | Self::Smart { .. } | Self::TtftRacing { .. } => {
                Err(concat!(
                    "legacy multi-model racing policies are retired; replace the policy with ",
                    "{\"type\":\"same_model\",\"attempts\":2,\"hedge_delay_ms\":0} ",
                    "using an explicit attempts value (1-5) and hedge_delay_ms (0-300000), ",
                    "or remove the racing policy to disable it"
                )
                .into())
            }
            Self::None => Err("enabled racing requires a non-none strategy".into()),
        }
    }
}

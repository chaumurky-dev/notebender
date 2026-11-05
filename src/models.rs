//! Model tiers, model ids and list prices.
//!
//! The application never hard-codes a model id outside this module and the user's config:
//! every task asks the [`crate::router`] for a [`Tier`], and the [`crate::config::Config`]
//! maps a tier to a concrete model id (so new model releases are a config change).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Capability/cost tier. Ordered cheapest-first so that `Tier::Haiku < Tier::Opus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Haiku,
    Sonnet,
    Opus,
    Fable,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Haiku, Tier::Sonnet, Tier::Opus, Tier::Fable];

    /// Default model id for the tier (overridable in config).
    pub fn default_model_id(self) -> &'static str {
        match self {
            Tier::Haiku => "claude-haiku-4-5",
            Tier::Sonnet => "claude-sonnet-5-5",
            Tier::Opus => "claude-opus-5-5",
            Tier::Fable => "claude-fable-5-1",
        }
    }

    /// Default list price, USD per million tokens (input, output). Used only for
    /// cost *estimates* in reports; overridable in config.
    pub fn default_price(self) -> Price {
        match self {
            Tier::Haiku => Price {
                input_per_mtok: 1.0,
                output_per_mtok: 5.0,
            },
            Tier::Sonnet => Price {
                input_per_mtok: 2.0,
                output_per_mtok: 10.0,
            },
            Tier::Opus => Price {
                input_per_mtok: 4.0,
                output_per_mtok: 20.0,
            },
            Tier::Fable => Price {
                input_per_mtok: 10.0,
                output_per_mtok: 50.0,
            },
        }
    }

    /// Maximum output tokens the tier's default model can stream.
    pub fn max_output_tokens(self) -> u32 {
        match self {
            Tier::Haiku => 64_000,
            _ => 128_000,
        }
    }

    /// Whether the tier's default model takes the `thinking: {type: "enabled", budget_tokens}`
    /// form (Haiku 4.5) rather than adaptive thinking (every 4.6+ model).
    pub fn uses_thinking_budget(self) -> bool {
        matches!(self, Tier::Haiku)
    }

    /// Whether the model supports `output_config.effort`.
    pub fn supports_effort(self) -> bool {
        !matches!(self, Tier::Haiku)
    }

    /// Whether the model accepts the server-side `fallbacks: "default"` parameter.
    pub fn supports_fallbacks(self) -> bool {
        matches!(self, Tier::Sonnet | Tier::Opus | Tier::Fable)
    }

    pub fn next_cheaper(self) -> Option<Tier> {
        match self {
            Tier::Haiku => None,
            Tier::Sonnet => Some(Tier::Haiku),
            Tier::Opus => Some(Tier::Sonnet),
            Tier::Fable => Some(Tier::Opus),
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Tier::Haiku => "haiku",
            Tier::Sonnet => "sonnet",
            Tier::Opus => "opus",
            Tier::Fable => "fable",
        };
        f.write_str(s)
    }
}

impl FromStr for Tier {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "haiku" => Ok(Tier::Haiku),
            "sonnet" => Ok(Tier::Sonnet),
            "opus" => Ok(Tier::Opus),
            "fable" => Ok(Tier::Fable),
            other => Err(format!(
                "unknown model tier '{other}' (expected haiku|sonnet|opus|fable)"
            )),
        }
    }
}

/// USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl Price {
    pub fn cost_usd(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        (input_tokens as f64 / 1e6) * self.input_per_mtok + (output_tokens as f64 / 1e6) * self.output_per_mtok
    }
}

/// Reasoning effort passed through `output_config.effort` on models that support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
            Effort::Max => "max",
        };
        f.write_str(s)
    }
}

impl FromStr for Effort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Effort::Low),
            "medium" => Ok(Effort::Medium),
            "high" => Ok(Effort::High),
            "xhigh" => Ok(Effort::XHigh),
            "max" => Ok(Effort::Max),
            other => Err(format!("unknown effort '{other}' (expected low|medium|high|xhigh|max)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_order_cheapest_first() {
        assert!(Tier::Haiku < Tier::Sonnet);
        assert!(Tier::Sonnet < Tier::Opus);
        assert!(Tier::Opus < Tier::Fable);
    }

    #[test]
    fn parse_round_trip() {
        for t in Tier::ALL {
            assert_eq!(t.to_string().parse::<Tier>().unwrap(), t);
        }
        assert!("gpt".parse::<Tier>().is_err());
    }

    #[test]
    fn cost_estimate() {
        let p = Price {
            input_per_mtok: 1.0,
            output_per_mtok: 5.0,
        };
        assert!((p.cost_usd(1_000_000, 200_000) - 2.0).abs() < 1e-9);
    }
}

//! Task → model-tier routing.
//!
//! The guiding rule is *cheapest appropriate*: deterministic work runs with no model at all,
//! parsing/summarising runs on Haiku, structured musical judgement (fingering, format
//! conversion) on Sonnet, and open-ended musical decision-making (reading a scanned score,
//! re-arranging for new instruments) on Opus. Fable is never selected by default; users can
//! route any task to it in config or with `--model task=fable`.

use crate::config::Config;
use crate::models::{Effort, Tier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// Every kind of model-backed task the pipeline can issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Parse the user's natural-language request into a structured plan.
    Plan,
    /// Optical music recognition: score image(s)/PDF → MusicXML.
    Omr,
    /// Convert MusicXML to another notation format when no deterministic converter exists.
    Convert,
    /// Add guitar (or other fretted) tablature.
    Tablature,
    /// Re-instrument / arrange for different instruments.
    Arrange,
    /// Free-form edit of the score described in words.
    Edit,
    /// Check a produced artifact against the request (cheap QA pass).
    Review,
}

impl TaskKind {
    pub const ALL: [TaskKind; 7] = [
        TaskKind::Plan,
        TaskKind::Omr,
        TaskKind::Convert,
        TaskKind::Tablature,
        TaskKind::Arrange,
        TaskKind::Edit,
        TaskKind::Review,
    ];

    /// Built-in default tier. See module docs for the rationale.
    pub fn default_tier(self) -> Tier {
        match self {
            TaskKind::Plan => Tier::Haiku,
            TaskKind::Omr => Tier::Opus,
            TaskKind::Convert => Tier::Sonnet,
            TaskKind::Tablature => Tier::Haiku,
            TaskKind::Arrange => Tier::Opus,
            TaskKind::Edit => Tier::Sonnet,
            TaskKind::Review => Tier::Haiku,
        }
    }

    /// Default reasoning effort for the task (only applied on tiers that support it).
    pub fn default_effort(self) -> Effort {
        match self {
            TaskKind::Plan | TaskKind::Review => Effort::Low,
            TaskKind::Convert => Effort::Medium,
            TaskKind::Tablature | TaskKind::Edit => Effort::Medium,
            TaskKind::Omr => Effort::High,
            TaskKind::Arrange => Effort::High,
        }
    }

    /// Generous output ceiling; the client streams, so large values are safe.
    pub fn default_max_tokens(self) -> u32 {
        match self {
            TaskKind::Plan => 4_000,
            TaskKind::Review => 4_000,
            TaskKind::Omr | TaskKind::Convert | TaskKind::Tablature | TaskKind::Arrange | TaskKind::Edit => 64_000,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            TaskKind::Plan => "parse the request into a plan",
            TaskKind::Omr => "read the score (PDF/images) into MusicXML",
            TaskKind::Convert => "convert notation formats (LLM fallback)",
            TaskKind::Tablature => "add fretted-instrument tablature",
            TaskKind::Arrange => "arrange for different instruments",
            TaskKind::Edit => "apply a free-form edit to the score",
            TaskKind::Review => "QA the result against the request",
        }
    }
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TaskKind::Plan => "plan",
            TaskKind::Omr => "omr",
            TaskKind::Convert => "convert",
            TaskKind::Tablature => "tablature",
            TaskKind::Arrange => "arrange",
            TaskKind::Edit => "edit",
            TaskKind::Review => "review",
        };
        f.write_str(s)
    }
}

impl FromStr for TaskKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "plan" => Ok(TaskKind::Plan),
            "omr" => Ok(TaskKind::Omr),
            "convert" => Ok(TaskKind::Convert),
            "tablature" | "tab" => Ok(TaskKind::Tablature),
            "arrange" => Ok(TaskKind::Arrange),
            "edit" => Ok(TaskKind::Edit),
            "review" => Ok(TaskKind::Review),
            other => Err(format!("unknown task '{other}'")),
        }
    }
}

/// The resolved choice for one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub tier: Tier,
    pub effort: Effort,
    pub max_tokens: u32,
}

/// Resolves tasks to model tiers using config defaults plus per-run overrides.
#[derive(Debug, Clone, Default)]
pub struct Router {
    routing: BTreeMap<TaskKind, Tier>,
    effort: BTreeMap<TaskKind, Effort>,
    overrides: BTreeMap<TaskKind, Tier>,
}

impl Router {
    pub fn from_config(cfg: &Config) -> Self {
        Router {
            routing: cfg.routing.clone(),
            effort: cfg.effort.clone(),
            overrides: BTreeMap::new(),
        }
    }

    /// Apply a `task=tier` override (e.g. from `--model omr=fable`).
    pub fn with_override(mut self, task: TaskKind, tier: Tier) -> Self {
        self.overrides.insert(task, tier);
        self
    }

    /// Parse and apply a list of `task=tier` strings.
    pub fn with_override_specs<'a>(mut self, specs: impl IntoIterator<Item = &'a str>) -> Result<Self, String> {
        for spec in specs {
            let (task, tier) = spec
                .split_once('=')
                .ok_or_else(|| format!("override '{spec}' must look like task=tier, e.g. omr=fable"))?;
            let task: TaskKind = task.parse()?;
            let tier: Tier = tier.parse()?;
            self.overrides.insert(task, tier);
        }
        Ok(self)
    }

    pub fn route(&self, task: TaskKind) -> Route {
        let tier = self
            .overrides
            .get(&task)
            .or_else(|| self.routing.get(&task))
            .copied()
            .unwrap_or_else(|| task.default_tier());
        let effort = self.effort.get(&task).copied().unwrap_or_else(|| task.default_effort());
        let max_tokens = task.default_max_tokens().min(tier.max_output_tokens());
        Route {
            tier,
            effort,
            max_tokens,
        }
    }

    /// Full table, for `notebender models`.
    pub fn table(&self) -> Vec<(TaskKind, Route)> {
        TaskKind::ALL.iter().map(|&t| (t, self.route(t))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_cheapest_appropriate() {
        let r = Router::default();
        assert_eq!(r.route(TaskKind::Plan).tier, Tier::Haiku);
        assert_eq!(r.route(TaskKind::Omr).tier, Tier::Opus);
        assert_eq!(r.route(TaskKind::Tablature).tier, Tier::Haiku);
        assert_eq!(r.route(TaskKind::Arrange).tier, Tier::Opus);
        // Fable is never a default.
        for (_, route) in r.table() {
            assert_ne!(route.tier, Tier::Fable);
        }
    }

    #[test]
    fn overrides_win() {
        let r = Router::default()
            .with_override_specs(["omr=fable", "plan=sonnet"])
            .unwrap();
        assert_eq!(r.route(TaskKind::Omr).tier, Tier::Fable);
        assert_eq!(r.route(TaskKind::Plan).tier, Tier::Sonnet);
        assert!(Router::default().with_override_specs(["nonsense"]).is_err());
    }

    #[test]
    fn haiku_caps_max_tokens() {
        let r = Router::default().with_override(TaskKind::Omr, Tier::Haiku);
        assert_eq!(r.route(TaskKind::Omr).max_tokens, 64_000);
    }
}

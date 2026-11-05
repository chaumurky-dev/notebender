//! Job model and orchestration.
//!
//! A [`JobRequest`] (input score + free-text instructions) becomes a [`Plan`] (via the planner
//! agent), which [`run_job`] executes step by step, emitting [`ProgressEvent`]s and collecting
//! [`Artifact`]s into a [`JobResult`]. Both the CLI and the web UI call [`run_job`].

use crate::config::Config;
use crate::models::Tier;
use crate::router::{Router, TaskKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Notation formats notebender can emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Format {
    #[serde(rename = "musicxml")]
    MusicXml,
    #[serde(rename = "lilypond")]
    LilyPond,
    #[serde(rename = "humdrum")]
    Humdrum,
    /// Engraved PDF (requires LilyPond).
    #[serde(rename = "pdf")]
    Pdf,
    /// ABC notation.
    #[serde(rename = "abc")]
    Abc,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::MusicXml,
        Format::LilyPond,
        Format::Humdrum,
        Format::Pdf,
        Format::Abc,
    ];

    /// Serde/CLI name.
    pub fn id(self) -> &'static str {
        match self {
            Format::MusicXml => "musicxml",
            Format::LilyPond => "lilypond",
            Format::Humdrum => "humdrum",
            Format::Pdf => "pdf",
            Format::Abc => "abc",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::MusicXml => "musicxml",
            Format::LilyPond => "ly",
            Format::Humdrum => "krn",
            Format::Pdf => "pdf",
            Format::Abc => "abc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Format::MusicXml => "MusicXML",
            Format::LilyPond => "LilyPond",
            Format::Humdrum => "Humdrum (**kern)",
            Format::Pdf => "PDF",
            Format::Abc => "ABC",
        }
    }

    pub fn parse(s: &str) -> Option<Format> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "musicxml" | "xml" | "mxl" => Some(Format::MusicXml),
            "lilypond" | "ly" => Some(Format::LilyPond),
            "humdrum" | "kern" | "krn" => Some(Format::Humdrum),
            "pdf" => Some(Format::Pdf),
            "abc" => Some(Format::Abc),
            _ => None,
        }
    }
}

/// One transformation in a plan. Produced by the planner agent (structured output) or
/// constructed directly by CLI subcommands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    /// Transpose by an interval or between keys. Exactly one of `semitones` or (`from_key`,`to_key`)
    /// is expected; `to_key` alone means "detect the source key from the score".
    Transpose {
        #[serde(default)]
        semitones: Option<i32>,
        #[serde(default)]
        from_key: Option<String>,
        #[serde(default)]
        to_key: Option<String>,
    },
    /// Add tablature for a fretted instrument.
    Tablature {
        /// e.g. "guitar", "bass", "ukulele".
        #[serde(default = "default_guitar")]
        instrument: String,
        /// e.g. "standard", "drop D", "DADGAD". Free text, interpreted by the model.
        #[serde(default)]
        tuning: Option<String>,
        /// Capo fret, if any.
        #[serde(default)]
        capo: Option<u8>,
    },
    /// Arrange / re-instrument the piece.
    Arrange {
        /// Target instruments in score order, e.g. ["cello", "guitar"].
        instruments: Vec<String>,
        /// Extra guidance from the user (difficulty, style, what to keep).
        #[serde(default)]
        notes: Option<String>,
    },
    /// Any other edit expressed in words (e.g. "simplify the left hand", "remove the repeats").
    Edit { instruction: String },
}

fn default_guitar() -> String {
    "guitar".into()
}

/// What the user asked for, as understood by the planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Plan {
    /// One-sentence restatement of the request, shown to the user for confirmation.
    pub summary: String,
    /// Steps in execution order (OMR is implicit and always first for PDF input).
    pub steps: Vec<Step>,
    /// Output formats to deliver. MusicXML is always included.
    pub deliverables: Vec<Format>,
    /// Anything the planner could not map onto a step; surfaced as a warning.
    #[serde(default)]
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRequest {
    /// Path to the input: a PDF, or an existing MusicXML (`.musicxml`/`.xml`) to skip OMR.
    pub input: PathBuf,
    /// Free-text instructions from the user.
    pub instructions: String,
    /// Directory to write artifacts into.
    pub out_dir: PathBuf,
    /// Skip the planner and use this plan directly (CLI subcommands, tests).
    #[serde(default)]
    pub plan: Option<Plan>,
    /// Run an extra model-backed QA pass on the final artifacts.
    #[serde(default)]
    pub review: bool,
    /// Don't call the API; produce the plan (if given) and describe what would run.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub path: PathBuf,
    pub format: Option<Format>,
    pub description: String,
}

/// Per-step accounting line for the final report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport {
    pub label: String,
    /// `None` for deterministic steps (no model).
    pub task: Option<TaskKind>,
    pub tier: Option<Tier>,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    pub seconds: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobResult {
    pub plan: Plan,
    pub artifacts: Vec<Artifact>,
    pub steps: Vec<StepReport>,
    pub warnings: Vec<String>,
    /// Human-readable summary (markdown).
    pub summary: String,
    pub total_cost_usd: f64,
}

/// Streaming progress for UIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ProgressEvent {
    Planned {
        plan: Plan,
    },
    StepStarted {
        label: String,
        task: Option<TaskKind>,
        tier: Option<Tier>,
        model: Option<String>,
    },
    StepFinished {
        report: StepReport,
    },
    Warning {
        message: String,
    },
    Artifact {
        artifact: Artifact,
    },
    Log {
        message: String,
    },
}

/// Callback type for progress; implementations must be cheap and non-blocking.
pub type ProgressSink = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

pub fn noop_sink() -> ProgressSink {
    Arc::new(|_| {})
}

/// Everything a job needs that is not part of the request itself.
#[derive(Clone)]
pub struct Context {
    pub config: Arc<Config>,
    pub router: Arc<Router>,
    pub client: Option<Arc<crate::claude::Client>>,
    pub tools: Arc<crate::tools::Toolbox>,
}

/// Run a job end-to-end. Implemented in `pipeline_run.rs` (kept separate so this file stays a
/// pure data model).
pub use crate::pipeline_run::run_job;

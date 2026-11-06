//! Arranging for a new set of instruments, in three passes.
//!
//! 1. **Strategy** (strong model, one small structured answer): target key, which instrument
//!    carries what in each section, how to thin the texture.
//! 2. **Realisation** (cheaper model, concurrent chunks of measures): the notes of every new
//!    instrument for those measures, as a compact event list at concert pitch.
//! 3. **Assembly** (Rust): the MusicXML score is built from the realised events and the
//!    source's measure skeleton (time/key changes, barlines, repeats, tempo and dynamics),
//!    with instrument clefs, transpositions and range checks applied deterministically.
//!
//! * [`instruments`] — names, ranges, clefs, transpositions.
//! * [`digest`] — the compact view of the source a model reads.
//! * [`assemble`] — realised events → MusicXML.

pub mod assemble;
pub mod digest;
pub mod instruments;

pub use instruments::ArrInstrument;
use serde::{Deserialize, Serialize};

/// The arranging plan produced by the strategy pass.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Strategy {
    /// Why this key, this distribution, this texture.
    pub rationale: String,
    /// Semitones to transpose the whole piece before realising (0 = keep the key).
    pub transpose_semitones: i32,
    /// Human name of the target key, e.g. "D major" (informational).
    pub target_key: Option<String>,
    pub instruments: Vec<InstrumentPlan>,
    pub sections: Vec<SectionPlan>,
    /// Things the user should double-check.
    pub cautions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstrumentPlan {
    /// As requested by the user, e.g. "cello".
    pub name: String,
    /// "melody", "bass", "accompaniment", "mixed", …
    pub role: String,
    /// Free-form guidance for the realisation (register, idiom, what to drop).
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SectionPlan {
    /// Printed measure numbers, inclusive.
    pub first_measure: String,
    pub last_measure: String,
    pub label: String,
    /// Who plays what here, and how the original texture is reduced.
    pub plan: String,
}

impl Strategy {
    pub fn describe(&self) -> String {
        let mut s = String::new();
        if let Some(k) = &self.target_key {
            s.push_str(&format!("Key: {k}"));
            if self.transpose_semitones != 0 {
                s.push_str(&format!(" (transposed {:+} semitones)", self.transpose_semitones));
            }
            s.push_str(". ");
        }
        s.push_str(&self.rationale);
        for i in &self.instruments {
            s.push_str(&format!("\n- {}: {} — {}", i.name, i.role, i.notes));
        }
        for sec in &self.sections {
            s.push_str(&format!(
                "\n- mm. {}–{} {}: {}",
                sec.first_measure, sec.last_measure, sec.label, sec.plan
            ));
        }
        for c in &self.cautions {
            s.push_str(&format!("\n- check: {c}"));
        }
        s
    }
}

/// One note, chord or rest of a realised instrument line. Pitches are concert (sounding)
/// pitch names such as "C#4", "Bb2"; an empty list is a rest. `onset` and `duration` are in
/// the measure's `<divisions>` units.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealizedNote {
    pub onset: i64,
    pub duration: i64,
    #[serde(default)]
    pub pitches: Vec<String>,
    /// 1-based voice within the instrument's staff.
    #[serde(default = "one")]
    pub voice: u8,
    /// "start", "stop" or "continue" for a tie into/out of this note.
    #[serde(default)]
    pub tie: Option<String>,
    #[serde(default)]
    pub grace: bool,
    /// Tuplet ratio (actual, normal), e.g. (3, 2) for triplets.
    #[serde(default)]
    pub tuplet: Option<(u32, u32)>,
    /// Optional short text such as "pizz.", "arco", "let ring".
    #[serde(default)]
    pub text: Option<String>,
}

fn one() -> u8 {
    1
}

/// The realised content of one measure for one instrument.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealizedMeasure {
    /// Printed measure number (the source's `number` attribute).
    pub measure: String,
    /// Instrument name as given in the strategy.
    pub instrument: String,
    pub notes: Vec<RealizedNote>,
}

//! Tablature for fretted instruments, built deterministically.
//!
//! The notation part is copied into a new `TAB` part; every pitched note receives
//! `<technical><string>/<fret>` computed in Rust from its sounding pitch, so the result is
//! always physically correct and rhythmically identical to the source. A model (or the
//! deterministic [`solver`]) only decides *which string* each note goes on.
//!
//! * [`instrument`] — tunings, string counts, octave conventions.
//! * [`notes`] — extract the pitched events (and fingering hints) of one part.
//! * [`solver`] — deterministic string choice minimising position changes.
//! * [`build`] — write the TAB part back into the document.

pub mod build;
pub mod instrument;
pub mod notes;
pub mod solver;

pub use instrument::Instrument;

/// A pitched note of one part, in document order.
///
/// `idx` counts every `<note>` element of the part (rests and grace notes included), so a
/// builder can find the element again after the choice has been made.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteEvent {
    pub idx: usize,
    /// 0-based position of the measure within the part.
    pub measure_idx: usize,
    /// The measure's `number` attribute.
    pub measure: String,
    /// Offset from the start of the measure, in `<divisions>` units. A grace note shares the
    /// onset of the note it precedes.
    pub onset: i64,
    pub duration: i64,
    /// Sounding pitch as a MIDI number (written pitch adjusted by `<transpose>`).
    pub midi: i32,
    /// `<chord/>`: sounds together with the previous event.
    pub chord: bool,
    pub grace: bool,
    /// Continuation of a tied note: must keep the previous string and fret.
    pub tie_stop: bool,
    pub voice: String,
    /// `<technical><string>` from the source score, 1 = highest-pitched string.
    pub string_hint: Option<u8>,
    /// Left-hand finger from `<technical><fingering>` in the source score.
    pub fingering: Option<String>,
}

/// String and fret for one event. `string` 1 is the highest-pitched string (MusicXML
/// convention); `fret` is relative to the capo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assignment {
    pub idx: usize,
    pub string: u8,
    pub fret: u8,
}

/// A position or barre mark in the source ("C. 7a", "VII", "B5"): from this point the hand
/// is expected to sit at `fret`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionMark {
    pub measure_idx: usize,
    pub onset: i64,
    pub fret: u8,
}

/// Group consecutive events that sound at the same instant (same measure and onset,
/// grace notes excluded) — the unit a fingering decision is made for.
pub fn simultaneous_groups(events: &[NoteEvent]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let joins = !e.grace
            && groups.last().is_some_and(|g| {
                let p = &events[g[0]];
                !p.grace && p.measure_idx == e.measure_idx && p.onset == e.onset
            });
        if joins {
            groups.last_mut().expect("non-empty").push(i);
        } else {
            groups.push(vec![i]);
        }
    }
    groups
}

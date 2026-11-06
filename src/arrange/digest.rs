//! A compact, model-readable view of a score: one line per part and staff per measure, with
//! every note as `pitch:duration` at its onset. Far smaller than the MusicXML, and at sounding
//! pitch, so a model can reason about the music without parsing notation.

use crate::musicxml::{summarize, Summary};
use crate::tab::instrument::pitch_name;
use crate::tab::notes::{self, PartNotes};
use anyhow::Result;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct MeasureDigest {
    /// 0-based index in the score.
    pub idx: usize,
    /// Printed number.
    pub number: String,
    /// Lines, one per (part, staff), e.g. `Piano RH: @0 G4:2 B4:2 @2 D5:4`.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Digest {
    pub summary: Summary,
    pub header: String,
    pub measures: Vec<MeasureDigest>,
}

fn staff_label(part_name: &str, staves: u8, staff: Option<u8>) -> String {
    match (staves, staff) {
        (s, Some(2)) if s >= 2 => format!("{part_name} LH"),
        (s, Some(1)) if s >= 2 => format!("{part_name} RH"),
        (s, Some(n)) if s >= 2 => format!("{part_name} staff {n}"),
        _ => part_name.to_string(),
    }
}

/// Build the digest of `xml`.
pub fn digest(xml: &str) -> Result<Digest> {
    let summary = summarize(xml)?;
    let mut parts: Vec<(PartNotes, u8)> = Vec::new();
    for p in &summary.parts {
        let lower = p.name.to_ascii_lowercase();
        let default_octave = if lower.contains("guitar") || lower.contains("bass") {
            -1
        } else {
            0
        };
        let notes = notes::extract(xml, &p.id, default_octave)?;
        parts.push((notes, p.staves));
    }
    let n_measures = parts.iter().map(|(p, _)| p.measure_numbers.len()).max().unwrap_or(0);
    let mut measures = Vec::with_capacity(n_measures);
    for idx in 0..n_measures {
        let number = parts
            .iter()
            .find_map(|(p, _)| p.measure_numbers.get(idx).cloned())
            .unwrap_or_else(|| idx.to_string());
        let mut lines = Vec::new();
        for (p, staves) in &parts {
            // group by staff, then by onset
            let mut by_staff: BTreeMap<u8, BTreeMap<i64, Vec<String>>> = BTreeMap::new();
            for e in p.events.iter().filter(|e| e.measure_idx == idx) {
                let mut tok = pitch_name(e.midi);
                if e.grace {
                    tok = format!("g{tok}");
                } else {
                    tok.push(':');
                    tok.push_str(&e.duration.to_string());
                }
                if e.tie_stop {
                    tok.insert(0, '~');
                }
                by_staff
                    .entry(e.staff.unwrap_or(1))
                    .or_default()
                    .entry(e.onset)
                    .or_default()
                    .push(tok);
            }
            if by_staff.is_empty() {
                lines.push(format!("{}: rest", p.part_name));
                continue;
            }
            for (staff, onsets) in by_staff {
                let mut line = format!("{}:", staff_label(&p.part_name, *staves, Some(staff)));
                for (onset, toks) in onsets {
                    line.push_str(&format!(" @{onset} {}", toks.join("+")));
                }
                lines.push(line);
            }
        }
        measures.push(MeasureDigest { idx, number, lines });
    }
    let header = {
        let mut h = String::new();
        if let Some(t) = &summary.title {
            h.push_str(&format!("Title: {t}. "));
        }
        if let Some(c) = &summary.composer {
            h.push_str(&format!("Composer: {c}. "));
        }
        h.push_str(&summary.describe());
        h.push_str(". Parts: ");
        let names: Vec<String> = parts
            .iter()
            .map(|(p, staves)| {
                let (lo, hi) = p
                    .events
                    .iter()
                    .fold((i32::MAX, i32::MIN), |(lo, hi), e| (lo.min(e.midi), hi.max(e.midi)));
                if lo == i32::MAX {
                    format!("{} (no notes)", p.part_name)
                } else {
                    format!(
                        "{} ({} staff{}, sounding {}–{})",
                        p.part_name,
                        staves,
                        if *staves == 1 { "" } else { "s" },
                        pitch_name(lo),
                        pitch_name(hi)
                    )
                }
            })
            .collect();
        h.push_str(&names.join("; "));
        h.push_str(". Durations are in each measure's <divisions> units; @n is the onset within the measure; '+' joins simultaneous notes; '~' marks a tied-over note; 'g' a grace note.");
        h
    };
    Ok(Digest {
        summary,
        header,
        measures,
    })
}

impl Digest {
    /// Divisions in force for measure `idx` (from the first part), if known.
    pub fn divisions_text(&self, _idx: usize) -> Option<String> {
        None
    }

    /// Text for measures `lo..hi` (indexes, `hi` exclusive); the whole score when `None`.
    pub fn text(&self, range: Option<(usize, usize)>) -> String {
        let (lo, hi) = range.unwrap_or((0, self.measures.len()));
        let mut out = String::new();
        for m in self.measures.iter().filter(|m| (lo..hi).contains(&m.idx)) {
            out.push_str(&format!("m{}\n", m.number));
            for l in &m.lines {
                out.push_str("  ");
                out.push_str(l);
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/simple.musicxml");
    const GUITAR: &str = include_str!("../../tests/fixtures/guitar.musicxml");

    #[test]
    fn digests_measures_and_notes() {
        let d = digest(FIXTURE).unwrap();
        assert_eq!(d.measures.len(), d.summary.measures);
        assert!(d.header.contains("Piano"));
        let text = d.text(None);
        assert!(text.starts_with("m1\n"), "{text}");
        assert!(text.contains("B4:"), "{text}");
        assert!(d.text(Some((0, 1))).lines().count() < text.lines().count() || d.measures.len() == 1);
    }

    #[test]
    fn guitar_parts_are_at_sounding_pitch_with_chords_and_grace() {
        let d = digest(GUITAR).unwrap();
        let text = d.text(None);
        // written A3 sounds A2; the open A chord at onset 2 is joined with '+'
        assert!(text.contains("@0 A2:2"), "{text}");
        assert!(text.contains("@2 E3:2+A3:2+C#4:2"), "{text}");
        assert!(text.contains("gG#4"), "{text}");
        assert!(text.contains("~E5:4"), "{text}");
        assert!(d.header.contains("sounding A2–E5"), "{}", d.header);
    }
}

//! Deterministic repairs for the mistakes models make in MusicXML they write themselves.
//!
//! * [`fix_missing_backups`] — a second voice that starts without `<backup>` lands after the
//!   first voice instead of alongside it, overflowing the measure and knocking every later
//!   bar line out of alignment. When a voice not yet seen in the measure begins while the
//!   cursor is past the start, a `<backup>` to the measure start is inserted.
//! * [`beams_are_sparse`] — OMR output often beams only some of the eighth notes; when
//!   most are unbeamed it is better to let LilyPond beam automatically.

use super::{join, matching_end, tokenize, Kind, Token};
use anyhow::Result;
use std::collections::HashSet;

fn child_text(t: &[Token], s: usize, e: usize, name: &str) -> Option<String> {
    let mut i = s + 1;
    while i < e {
        if let Kind::Start(n) = &t[i].kind {
            let end = matching_end(t, i)?;
            if n == name && end == i + 2 && t[i + 1].kind == Kind::Text {
                return Some(t[i + 1].raw.trim().to_string());
            }
            i = end;
        }
        i += 1;
    }
    None
}

fn has_empty_child(t: &[Token], s: usize, e: usize, name: &str) -> bool {
    (s + 1..e).any(|i| matches!(&t[i].kind, Kind::Empty(n) if n == name))
        || (s + 1..e).any(|i| matches!(&t[i].kind, Kind::Start(n) if n == name && matching_end(t, i) == Some(i + 1)))
}

/// Insert `<backup>` before a voice that starts mid-measure without one. Returns the repaired
/// document (byte-identical when nothing was wrong) and one note per inserted backup.
pub fn fix_missing_backups(xml: &str) -> Result<(String, Vec<String>)> {
    let t = tokenize(xml)?;
    let mut out: Vec<Token> = Vec::with_capacity(t.len());
    let mut notes = Vec::new();
    let mut i = 0;
    while i < t.len() {
        match &t[i].kind {
            Kind::Start(n) if n == "measure" => {
                let end = matching_end(&t, i).unwrap_or(t.len() - 1);
                let number = t[i]
                    .raw
                    .split("number=\"")
                    .nth(1)
                    .and_then(|r| r.split('"').next())
                    .unwrap_or("?")
                    .to_string();
                let (fixed, inserted) = repair_measure(&t[i..=end]);
                for pos in inserted {
                    notes.push(format!(
                        "measure {number}: inserted a missing <backup> before voice {pos}"
                    ));
                }
                out.extend(fixed);
                i = end + 1;
            }
            _ => {
                out.push(t[i].clone());
                i += 1;
            }
        }
    }
    Ok((join(&out), notes))
}

fn repair_measure(m: &[Token]) -> (Vec<Token>, Vec<String>) {
    let mut out = Vec::with_capacity(m.len() + 4);
    let mut inserted = Vec::new();
    let mut cursor: i64 = 0;
    let mut seen: HashSet<String> = HashSet::new();
    let mut current_voice: Option<String> = None;
    let mut last_was_backup = false;
    let mut i = 0;
    while i < m.len() {
        match &m[i].kind {
            Kind::Start(n) if n == "note" => {
                let end = matching_end(m, i).unwrap_or(m.len() - 1);
                let voice = child_text(m, i, end, "voice").unwrap_or_else(|| "1".into());
                let duration: i64 = child_text(m, i, end, "duration")
                    .and_then(|d| d.parse::<f64>().ok())
                    .map(|d| d.round() as i64)
                    .unwrap_or(0);
                let chord = has_empty_child(m, i, end, "chord");
                let grace = has_empty_child(m, i, end, "grace");
                let new_voice = current_voice.as_deref() != Some(voice.as_str());
                if new_voice && !seen.contains(&voice) && cursor > 0 && !last_was_backup && !chord {
                    out.push(Token {
                        kind: Kind::Other,
                        raw: format!("<backup>\n<duration>{cursor}</duration>\n</backup>\n"),
                    });
                    inserted.push(voice.clone());
                    cursor = 0;
                }
                seen.insert(voice.clone());
                current_voice = Some(voice);
                if !chord && !grace {
                    cursor += duration;
                }
                out.extend_from_slice(&m[i..=end]);
                last_was_backup = false;
                i = end + 1;
                continue;
            }
            Kind::Start(n) if n == "backup" || n == "forward" => {
                let end = matching_end(m, i).unwrap_or(m.len() - 1);
                let d: i64 = child_text(m, i, end, "duration")
                    .and_then(|d| d.parse::<f64>().ok())
                    .map(|d| d.round() as i64)
                    .unwrap_or(0);
                if n == "backup" {
                    cursor -= d;
                    last_was_backup = true;
                } else {
                    cursor += d;
                }
                out.extend_from_slice(&m[i..=end]);
                i = end + 1;
                continue;
            }
            Kind::Text | Kind::Other => {}
            _ => last_was_backup = false,
        }
        out.push(m[i].clone());
        i += 1;
    }
    (out, inserted)
}

/// True when fewer than 80% of the eighth-or-shorter notes carry `<beam>` information, in
/// which case LilyPond's automatic beaming gives a better result than the file's own beams.
pub fn beams_are_sparse(xml: &str) -> bool {
    let short = [
        "<type>eighth</type>",
        "<type>16th</type>",
        "<type>32nd</type>",
        "<type>64th</type>",
    ]
    .iter()
    .map(|t| xml.matches(t).count())
    .sum::<usize>();
    if short < 8 {
        return false;
    }
    let beams = xml.matches("<beam").count();
    beams * 5 < short * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    const BROKEN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
<part-list><score-part id="P1"><part-name>Guitar</part-name></score-part></part-list>
<part id="P1">
<measure number="32">
<attributes><divisions>2</divisions><time><beats>3</beats><beat-type>4</beat-type></time></attributes>
<note><pitch><step>A</step><octave>4</octave></pitch><duration>2</duration><voice>1</voice><type>quarter</type></note>
<note><chord/><pitch><step>C</step><alter>1</alter><octave>5</octave></pitch><duration>2</duration><voice>1</voice><type>quarter</type></note>
<note><pitch><step>E</step><octave>6</octave></pitch><duration>1</duration><voice>1</voice><type>eighth</type></note>
<note><pitch><step>D</step><octave>6</octave></pitch><duration>1</duration><voice>1</voice><type>eighth</type></note>
<note><grace/><pitch><step>F</step><octave>6</octave></pitch><voice>1</voice><type>16th</type></note>
<note><pitch><step>F</step><octave>6</octave></pitch><duration>2</duration><voice>1</voice><type>quarter</type></note>
<note><pitch><step>A</step><octave>3</octave></pitch><duration>2</duration><voice>3</voice><type>quarter</type></note>
<note><rest/><duration>4</duration><voice>3</voice><type>half</type></note>
</measure>
<measure number="33">
<note><pitch><step>A</step><octave>4</octave></pitch><duration>6</duration><voice>1</voice><type>half</type><dot/></note>
<backup><duration>6</duration></backup>
<note><pitch><step>A</step><octave>3</octave></pitch><duration>6</duration><voice>3</voice><type>half</type><dot/></note>
</measure>
</part>
</score-partwise>"#;

    #[test]
    fn inserts_backup_before_a_voice_that_starts_without_one() {
        let (fixed, notes) = fix_missing_backups(BROKEN).unwrap();
        assert_eq!(notes, vec!["measure 32: inserted a missing <backup> before voice 3"]);
        let m32 = &fixed[fixed.find("number=\"32\"").unwrap()..fixed.find("number=\"33\"").unwrap()];
        assert_eq!(m32.matches("<backup>").count(), 1);
        assert!(
            m32.contains("<backup>\n<duration>6</duration>\n</backup>\n<note><pitch><step>A</step><octave>3</octave>")
        );
        // measure 33 already had its backup: untouched
        let m33 = &fixed[fixed.find("number=\"33\"").unwrap()..];
        assert_eq!(m33.matches("<backup>").count(), 1);
        // idempotent
        let (again, notes2) = fix_missing_backups(&fixed).unwrap();
        assert!(notes2.is_empty());
        assert_eq!(again, fixed);
    }

    #[test]
    fn leaves_correct_documents_byte_identical() {
        let ok = include_str!("../../tests/fixtures/simple.musicxml");
        let (fixed, notes) = fix_missing_backups(ok).unwrap();
        assert!(notes.is_empty());
        assert_eq!(fixed, ok);
        let guitar = include_str!("../../tests/fixtures/guitar.musicxml");
        assert_eq!(fix_missing_backups(guitar).unwrap().0, guitar);
    }

    #[test]
    fn sparse_beams_are_detected() {
        let mut xml = String::new();
        for _ in 0..10 {
            xml.push_str("<note><type>eighth</type></note>");
        }
        assert!(beams_are_sparse(&xml));
        for _ in 0..9 {
            xml.push_str("<beam number=\"1\">begin</beam>");
        }
        assert!(!beams_are_sparse(&xml));
        assert!(!beams_are_sparse("<note><type>eighth</type></note>"));
    }
}

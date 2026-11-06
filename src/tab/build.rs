//! Write a TAB part into a MusicXML document.
//!
//! The notation part is copied token by token (so rhythm, voices, ties and tuplets are
//! identical by construction); directions, lyrics and harmony are dropped from the copy, the
//! clef becomes `TAB`, `<staff-details>` carries the tuning, and every pitched note that has an
//! [`Assignment`] receives `<technical><string>/<fret>`. The TAB part's pitches are written at
//! sounding pitch with no `<transpose>`: LilyPond derives fret numbers from pitch and string, so
//! a written-an-octave-up guitar pitch would come out twelve frets too high.

use super::{Assignment, Instrument};
use crate::musicxml::{join, matching_end, tokenize, Kind, Token};
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;

/// Id given to the generated part: `<source id>-TAB`.
pub fn tab_part_id(src: &str) -> String {
    format!("{src}-TAB")
}

/// Add a TAB part for `src_part_id`, placed right after it in both `<part-list>` and the
/// document. Any previous part with the same generated id is replaced. When the source part
/// declares no `<transpose>` and the instrument is written at an octave, the `<transpose>` is
/// also added to the source part so playback matches.
pub fn add_tab_part(
    xml: &str,
    src_part_id: &str,
    part_name: &str,
    inst: &Instrument,
    capo: u8,
    assignments: &[Assignment],
) -> Result<String> {
    let tab_id = tab_part_id(src_part_id);
    let xml = remove_part(xml, &tab_id)?;
    let t = tokenize(&xml)?;
    let by_idx: HashMap<usize, Assignment> = assignments.iter().map(|a| (a.idx, *a)).collect();

    // --- part-list ------------------------------------------------------------------------
    let (ls, le) = find_first(&t, 0, t.len(), "part-list").ok_or_else(|| anyhow!("no <part-list>"))?;
    let (_, sp_end) = find_score_part(&t, ls, le, src_part_id)
        .ok_or_else(|| anyhow!("no <score-part id=\"{src_part_id}\"> in part-list"))?;
    let score_part = format!(
        "\n<score-part id=\"{tab_id}\">\n<part-name>{}</part-name>\n<part-abbreviation>TAB</part-abbreviation>\n</score-part>",
        escape(part_name)
    );

    // --- source part --------------------------------------------------------------------
    let (ps, pe) = find_part(&t, src_part_id).ok_or_else(|| anyhow!("no <part id=\"{src_part_id}\">"))?;
    let mut src_tokens: Vec<Token> = t[ps..=pe].to_vec();
    if has_multiple_staves(&src_tokens) {
        bail!("part {src_part_id} has more than one staff; tablature needs a single-staff part");
    }
    if inst.octave_change != 0 && !has_transpose(&src_tokens) {
        src_tokens = insert_transpose(&src_tokens, inst.octave_change)?;
    }
    let tab_tokens = build_tab_copy(&src_tokens, &tab_id, inst, capo, &by_idx)?;

    let mut out = String::new();
    out.push_str(&join(&t[..=sp_end]));
    out.push_str(&score_part);
    out.push_str(&join(&t[sp_end + 1..ps]));
    out.push_str(&join(&src_tokens));
    out.push('\n');
    out.push_str(&join(&tab_tokens));
    out.push_str(&join(&t[pe + 1..]));
    Ok(out)
}

/// Remove `<score-part id>` and `<part id>` for `part_id` if present (byte-identical otherwise).
pub fn remove_part(xml: &str, part_id: &str) -> Result<String> {
    let t = tokenize(xml)?;
    let mut drop: Vec<(usize, usize)> = Vec::new();
    if let Some((ls, le)) = find_first(&t, 0, t.len(), "part-list") {
        if let Some(r) = find_score_part(&t, ls, le, part_id) {
            drop.push(r);
        }
    }
    if let Some(r) = find_part(&t, part_id) {
        drop.push(r);
    }
    if drop.is_empty() {
        return Ok(xml.to_string());
    }
    let mut out = String::new();
    let mut i = 0;
    while i < t.len() {
        if let Some((_, e)) = drop.iter().find(|(s, _)| *s == i) {
            i = e + 1;
            // swallow the whitespace that preceded the removed element
            continue;
        }
        out.push_str(&t[i].raw);
        i += 1;
    }
    Ok(out)
}

/// First `<name>` element (at any depth) in token range [s, e): (start, end).
fn find_first(t: &[Token], s: usize, e: usize, name: &str) -> Option<(usize, usize)> {
    (s..e).find_map(|i| match &t[i].kind {
        Kind::Start(n) if n == name => matching_end(t, i).map(|end| (i, end)),
        _ => None,
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn id_attr(raw: &str) -> Option<&str> {
    let i = raw.find("id=")? + 3;
    let q = raw[i..].chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let rest = &raw[i + 1..];
    let end = rest.find(q)?;
    Some(&rest[..end])
}

fn find_score_part(t: &[Token], s: usize, e: usize, id: &str) -> Option<(usize, usize)> {
    let mut i = s + 1;
    while i < e {
        if let Kind::Start(n) = &t[i].kind {
            let end = matching_end(t, i)?;
            if n == "score-part" && id_attr(&t[i].raw) == Some(id) {
                return Some((i, end));
            }
            i = end;
        }
        i += 1;
    }
    None
}

fn find_part(t: &[Token], id: &str) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < t.len() {
        if let Kind::Start(n) = &t[i].kind {
            if n == "part" && id_attr(&t[i].raw) == Some(id) {
                return Some((i, matching_end(t, i)?));
            }
        }
        i += 1;
    }
    None
}

fn has_multiple_staves(t: &[Token]) -> bool {
    t.iter().enumerate().any(|(i, tok)| {
        tok.kind == Kind::Start("staves".into())
            && t.get(i + 1)
                .is_some_and(|x| x.raw.trim().parse::<u32>().is_ok_and(|n| n > 1))
    })
}

fn has_transpose(t: &[Token]) -> bool {
    t.iter().any(|tok| tok.kind == Kind::Start("transpose".into()))
}

fn transpose_xml(octave_change: i8) -> String {
    format!(
        "<transpose>\n<diatonic>0</diatonic>\n<chromatic>0</chromatic>\n<octave-change>{octave_change}</octave-change>\n</transpose>\n"
    )
}

/// Insert `<transpose>` at the end of the first `<attributes>` of the part.
fn insert_transpose(t: &[Token], octave_change: i8) -> Result<Vec<Token>> {
    let (_, ae) = find_first(t, 0, t.len(), "attributes").ok_or_else(|| anyhow!("part has no <attributes>"))?;
    let mut out = join(&t[..ae]);
    out.push_str(&transpose_xml(octave_change));
    out.push_str(&join(&t[ae..]));
    tokenize(&out)
}

fn staff_details_xml(inst: &Instrument, capo: u8) -> String {
    let mut s = format!("<staff-details>\n<staff-lines>{}</staff-lines>\n", inst.strings());
    // staff-tuning line 1 is the bottom line of the TAB staff: the string nearest the player.
    for (i, midi) in inst.tuning.iter().enumerate() {
        let (step, alter, octave) = spell(*midi);
        s.push_str(&format!(
            "<staff-tuning line=\"{}\">\n<tuning-step>{step}</tuning-step>\n",
            i + 1
        ));
        if alter != 0 {
            s.push_str(&format!("<tuning-alter>{alter}</tuning-alter>\n"));
        }
        s.push_str(&format!("<tuning-octave>{octave}</tuning-octave>\n</staff-tuning>\n"));
    }
    if capo > 0 {
        s.push_str(&format!("<capo>{capo}</capo>\n"));
    }
    s.push_str("</staff-details>\n");
    s
}

fn spell(midi: i32) -> (char, i32, i32) {
    const NAMES: [(char, i32); 12] = [
        ('C', 0),
        ('C', 1),
        ('D', 0),
        ('E', -1),
        ('E', 0),
        ('F', 0),
        ('F', 1),
        ('G', 0),
        ('A', -1),
        ('A', 0),
        ('B', -1),
        ('B', 0),
    ];
    let (step, alter) = NAMES[midi.rem_euclid(12) as usize];
    (step, alter, midi.div_euclid(12) - 1)
}

/// Produce the TAB part's tokens from the (possibly transpose-augmented) source part tokens.
fn build_tab_copy(
    src: &[Token],
    tab_id: &str,
    inst: &Instrument,
    capo: u8,
    by_idx: &HashMap<usize, Assignment>,
) -> Result<Vec<Token>> {
    let mut out: Vec<Token> = Vec::with_capacity(src.len() + 64);
    let mut i = 0;
    let mut note_idx = 0usize;
    let mut first_attributes = true;
    while i < src.len() {
        let tok = &src[i];
        match &tok.kind {
            Kind::Start(name) if i == 0 && name == "part" => {
                out.push(Token {
                    kind: tok.kind.clone(),
                    raw: format!("<part id=\"{tab_id}\">"),
                });
            }
            Kind::Start(name) if matches!(name.as_str(), "direction" | "harmony" | "print" | "lyric") => {
                i = matching_end(src, i).ok_or_else(|| anyhow!("unbalanced <{name}>"))?;
            }
            Kind::Empty(name) if matches!(name.as_str(), "direction" | "harmony" | "print" | "lyric") => {}
            Kind::Start(name) if name == "attributes" => {
                let end = matching_end(src, i).ok_or_else(|| anyhow!("unbalanced <attributes>"))?;
                out.extend(rewrite_attributes(&src[i..=end], inst, capo, first_attributes));
                first_attributes = false;
                i = end;
            }
            Kind::Start(name) if name == "note" => {
                let end = matching_end(src, i).ok_or_else(|| anyhow!("unbalanced <note>"))?;
                let assignment = by_idx.get(&note_idx).copied();
                note_idx += 1;
                out.extend(rewrite_note(&src[i..=end], assignment, inst.octave_change));
                i = end;
            }
            _ => out.push(tok.clone()),
        }
        i += 1;
    }
    Ok(out)
}

/// Keep divisions/key/time (and anything that is not a clef, staff-details, transpose or
/// staves); on the first attributes append the TAB clef, tuning and transpose.
fn rewrite_attributes(a: &[Token], inst: &Instrument, capo: u8, first: bool) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    let last = a.len() - 1;
    while i < last {
        let tok = &a[i];
        match &tok.kind {
            Kind::Start(name) if matches!(name.as_str(), "clef" | "staff-details" | "transpose" | "staves") => {
                if let Some(end) = matching_end(a, i) {
                    i = end + 1;
                    continue;
                }
            }
            _ => {}
        }
        out.push(tok.clone());
        i += 1;
    }
    if first {
        let mut extra = String::from("<clef>\n<sign>TAB</sign>\n<line>5</line>\n</clef>\n");
        extra.push_str(&staff_details_xml(inst, capo));
        out.push(Token {
            kind: Kind::Other,
            raw: extra,
        });
    }
    out.push(a[last].clone());
    out
}

/// Copy a note, dropping any existing `<technical>` and `<lyric>`, shifting the written octave
/// to sounding pitch, then add string/fret.
fn rewrite_note(n: &[Token], assignment: Option<Assignment>, octave_shift: i8) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    let last = n.len() - 1;
    while i < last {
        let tok = &n[i];
        if let Kind::Start(name) = &tok.kind {
            if name == "technical" || name == "lyric" {
                if let Some(end) = matching_end(n, i) {
                    i = end + 1;
                    continue;
                }
            }
            if name == "octave" && octave_shift != 0 && i + 2 < n.len() && n[i + 1].kind == Kind::Text {
                if let Ok(o) = n[i + 1].raw.trim().parse::<i32>() {
                    out.push(tok.clone());
                    out.push(Token {
                        kind: Kind::Text,
                        raw: (o + octave_shift as i32).to_string(),
                    });
                    i += 2;
                    continue;
                }
            }
        }
        out.push(tok.clone());
        i += 1;
    }
    if let Some(a) = assignment {
        out.push(Token {
            kind: Kind::Other,
            raw: format!(
                "<notations>\n<technical>\n<string>{}</string>\n<fret>{}</fret>\n</technical>\n</notations>\n",
                a.string, a.fret
            ),
        });
    }
    out.push(n[last].clone());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicxml::{summarize, validate};

    fn guitar() -> Instrument {
        Instrument::resolve("guitar", None).unwrap()
    }

    const FIXTURE: &str = include_str!("../../tests/fixtures/guitar.musicxml");

    #[test]
    fn adds_a_tab_part_after_the_source() {
        let assignments = vec![
            Assignment {
                idx: 0,
                string: 5,
                fret: 0,
            },
            Assignment {
                idx: 1,
                string: 4,
                fret: 2,
            },
        ];
        let out = add_tab_part(FIXTURE, "P1", "Guitar TAB", &guitar(), 0, &assignments).unwrap();
        validate(&out).unwrap();
        let s = summarize(&out).unwrap();
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[1].id, "P1-TAB");
        assert_eq!(s.parts[1].name, "Guitar TAB");
        assert_eq!(s.parts[1].measures, s.parts[0].measures);
        assert_eq!(s.parts[1].clefs, vec!["TAB"]);
        assert!(s.has_tablature);
        assert!(out.contains("<staff-lines>6</staff-lines>"));
        assert!(
            out.contains("<staff-tuning line=\"1\">\n<tuning-step>E</tuning-step>\n<tuning-octave>2</tuning-octave>")
        );
        assert!(out.contains("<string>5</string>\n<fret>0</fret>"));
        assert!(out.contains("<string>4</string>\n<fret>2</fret>"));
        // notation part untouched (fingering hint kept, direction kept)
        let notation = &out[..out.find("<part id=\"P1-TAB\">").unwrap()];
        assert!(notation.contains("<fingering>4</fingering>"));
        assert_eq!(notation.matches("C. 2a").count(), 1);
        // tab copy drops directions and the source technical, is at sounding pitch with no transpose
        let tab = &out[out.find("<part id=\"P1-TAB\">").unwrap()..];
        assert!(!tab.contains("C. 2a"));
        assert!(!tab.contains("<fingering>"));
        assert_eq!(tab.matches("<transpose>").count(), 0);
        assert_eq!(tab.matches("<clef>").count(), 1);
        // written A3 (first note) sounds A2 in the TAB part
        assert!(tab.contains("<step>A</step>\n<octave>2</octave>"), "{tab}");
        assert!(!tab.contains("<octave>6</octave>"), "{tab}");
    }

    #[test]
    fn replaces_an_existing_tab_part_and_adds_transpose_when_missing() {
        let no_transpose = FIXTURE.replace(
            "<transpose>\n<diatonic>0</diatonic>\n<chromatic>0</chromatic>\n<octave-change>-1</octave-change>\n</transpose>\n",
            "",
        );
        assert!(!no_transpose.contains("<transpose>"));
        let once = add_tab_part(&no_transpose, "P1", "Guitar TAB", &guitar(), 2, &[]).unwrap();
        assert_eq!(once.matches("<transpose>").count(), 1);
        assert!(once.contains("<capo>2</capo>"));
        let twice = add_tab_part(&once, "P1", "Guitar TAB", &guitar(), 2, &[]).unwrap();
        assert_eq!(summarize(&twice).unwrap().parts.len(), 2);
        assert_eq!(twice.matches("<part id=\"P1-TAB\">").count(), 1);
    }

    #[test]
    fn refuses_multi_staff_parts() {
        let piano = FIXTURE.replace(
            "<divisions>2</divisions>",
            "<divisions>2</divisions>\n<staves>2</staves>",
        );
        let err = add_tab_part(&piano, "P1", "Guitar TAB", &guitar(), 0, &[]).unwrap_err();
        assert!(err.to_string().contains("more than one staff"));
    }

    #[test]
    fn spells_tunings() {
        assert_eq!(spell(40), ('E', 0, 2));
        assert_eq!(spell(39), ('E', -1, 2));
        assert_eq!(spell(61), ('C', 1, 4));
        assert_eq!(spell(60), ('C', 0, 4));
    }
}

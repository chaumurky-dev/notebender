//! Extract the pitched events of one part, with any fingering hints the engraver left.

use super::{NoteEvent, PositionMark};
use crate::musicxml::parse_doc;
use anyhow::{anyhow, Context, Result};
use roxmltree::Node;

/// Everything the string chooser needs to know about one part.
#[derive(Debug, Clone, Default)]
pub struct PartNotes {
    pub part_id: String,
    pub part_name: String,
    /// `<transpose><octave-change>` found in the part (0 when absent).
    pub octave_change: i8,
    /// `<transpose><chromatic>` found in the part (0 when absent).
    pub chromatic: i32,
    /// Whether the part declares a `<transpose>` at all.
    pub has_transpose: bool,
    /// Number of `<note>` elements in the part (rests included): the `idx` space.
    pub note_count: usize,
    pub measure_numbers: Vec<String>,
    pub events: Vec<NoteEvent>,
    pub marks: Vec<PositionMark>,
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.has_tag_name(name))
}

fn text_of(n: Node, name: &'static str) -> Option<String> {
    children(n, name)
        .next()
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn int_of(n: Node, name: &'static str) -> Option<i64> {
    text_of(n, name)
        .and_then(|t| t.parse::<f64>().ok())
        .map(|f| f.round() as i64)
}

/// Written pitch of a `<pitch>` element as MIDI (C4 = 60).
pub fn written_midi(pitch: Node) -> Option<i32> {
    let step = text_of(pitch, "step")?;
    let semis = match step.chars().next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let octave = int_of(pitch, "octave")? as i32;
    let alter = text_of(pitch, "alter")
        .and_then(|a| a.parse::<f64>().ok())
        .map(|a| a.round() as i32)
        .unwrap_or(0);
    Some((octave + 1) * 12 + semis + alter)
}

/// Find the part to tab: the one whose name mentions `instrument` (e.g. "Guitar"), or the
/// only part when there is just one. Returns `(id, name)`.
pub fn find_part(xml: &str, instrument: &str) -> Result<Option<(String, String)>> {
    let doc = parse_doc(xml).context("not well-formed XML")?;
    let root = doc.root_element();
    let list = children(root, "part-list")
        .next()
        .ok_or_else(|| anyhow!("no <part-list>"))?;
    let parts: Vec<(String, String)> = children(list, "score-part")
        .filter_map(|p| {
            let id = p.attribute("id")?.to_string();
            let mut name = text_of(p, "part-name").unwrap_or_default();
            if let Some(i) = p.descendants().find(|n| n.has_tag_name("instrument-name")) {
                if let Some(t) = i.text() {
                    name.push(' ');
                    name.push_str(t.trim());
                }
            }
            Some((id, name))
        })
        .collect();
    let needle = instrument.to_ascii_lowercase();
    let first_word = needle.split_whitespace().last().unwrap_or(&needle).to_string();
    let hit = parts
        .iter()
        .find(|(_, name)| {
            let n = name.to_ascii_lowercase();
            !n.contains("tab") && (n.contains(&needle) || n.contains(&first_word))
        })
        .cloned();
    if hit.is_some() {
        return Ok(hit);
    }
    let non_tab: Vec<_> = parts
        .iter()
        .filter(|(_, n)| !n.to_ascii_lowercase().contains("tab"))
        .collect();
    Ok(if non_tab.len() == 1 {
        Some(non_tab[0].clone())
    } else {
        None
    })
}

/// Parse a position or barre mark such as "C. 7a", "C7", "CVII", "B. V", "Barré IX", "VII".
pub fn parse_position_mark(text: &str) -> Option<u8> {
    let mut s = text.trim().to_uppercase();
    for prefix in ["CEJILLA", "CEJA", "BARRÉ", "BARRE", "POS.", "POS"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    if s.len() > 1 && (s.starts_with('C') || s.starts_with('B')) && !s.starts_with("CI") && !s.starts_with("CV") {
        s = s[1..].to_string();
    } else if let Some(rest) = s.strip_prefix('C').or_else(|| s.strip_prefix('B')) {
        // "CVII" / "CIII": a letter followed directly by a roman numeral.
        if rest.chars().all(|c| matches!(c, 'I' | 'V' | 'X')) {
            s = rest.to_string();
        }
    }
    let s: String = s
        .trim_matches(|c: char| c == '.' || c == ':' || c.is_whitespace())
        .trim_end_matches(['A', 'ª', 'º', '.', ' '])
        .to_string();
    if s.is_empty() {
        return None;
    }
    let value = if s.chars().all(|c| c.is_ascii_digit()) {
        s.parse::<u8>().ok()?
    } else if s.chars().all(|c| matches!(c, 'I' | 'V' | 'X')) {
        roman(&s)?
    } else {
        return None;
    };
    (1..=20).contains(&value).then_some(value)
}

fn roman(s: &str) -> Option<u8> {
    let mut total = 0i32;
    let vals: Vec<i32> = s
        .chars()
        .map(|c| match c {
            'I' => Some(1),
            'V' => Some(5),
            'X' => Some(10),
            _ => None,
        })
        .collect::<Option<_>>()?;
    for (i, v) in vals.iter().enumerate() {
        if vals.get(i + 1).is_some_and(|n| n > v) {
            total -= v;
        } else {
            total += v;
        }
    }
    u8::try_from(total).ok().filter(|v| *v > 0)
}

/// Extract the pitched events of part `part_id`. When the part declares no `<transpose>`,
/// `default_octave_change` is assumed (guitar notation sounds an octave below written).
pub fn extract(xml: &str, part_id: &str, default_octave_change: i8) -> Result<PartNotes> {
    let doc = parse_doc(xml).context("not well-formed XML")?;
    let root = doc.root_element();
    let part = children(root, "part")
        .find(|p| p.attribute("id") == Some(part_id))
        .ok_or_else(|| anyhow!("no <part id=\"{part_id}\">"))?;
    let mut out = PartNotes {
        part_id: part_id.to_string(),
        ..Default::default()
    };
    if let Some(list) = children(root, "part-list").next() {
        if let Some(sp) = children(list, "score-part").find(|p| p.attribute("id") == Some(part_id)) {
            out.part_name = text_of(sp, "part-name").unwrap_or_default();
        }
    }
    // Pass 1: transposition convention.
    if let Some(t) = part.descendants().find(|n| n.has_tag_name("transpose")) {
        out.has_transpose = true;
        out.octave_change = int_of(t, "octave-change").unwrap_or(0) as i8;
        out.chromatic = int_of(t, "chromatic").unwrap_or(0) as i32;
    } else {
        out.octave_change = default_octave_change;
    }
    let shift = out.chromatic + 12 * out.octave_change as i32;

    let mut idx = 0usize;
    for (measure_idx, m) in children(part, "measure").enumerate() {
        let number = m.attribute("number").unwrap_or("").to_string();
        out.measure_numbers.push(number.clone());
        let mut pos: i64 = 0;
        let mut last_onset: i64 = 0;
        for el in m.children().filter(|c| c.is_element()) {
            match el.tag_name().name() {
                "backup" => pos -= int_of(el, "duration").unwrap_or(0),
                "forward" => pos += int_of(el, "duration").unwrap_or(0),
                "direction" => {
                    for w in el.descendants().filter(|n| n.has_tag_name("words")) {
                        if let Some(fret) = w.text().and_then(parse_position_mark) {
                            out.marks.push(PositionMark {
                                measure_idx,
                                onset: pos.max(0),
                                fret,
                            });
                        }
                    }
                }
                "note" => {
                    let this_idx = idx;
                    idx += 1;
                    let chord = children(el, "chord").next().is_some();
                    let grace = children(el, "grace").next().is_some();
                    let duration = int_of(el, "duration").unwrap_or(0);
                    let onset = if chord { last_onset } else { pos };
                    if !chord && !grace {
                        last_onset = pos;
                        pos += duration;
                    }
                    let Some(pitch) = children(el, "pitch").next() else {
                        continue; // rest or unpitched
                    };
                    let Some(written) = written_midi(pitch) else {
                        continue;
                    };
                    let tie_stop = children(el, "tie").any(|t| t.attribute("type") == Some("stop"));
                    let technical = children(el, "notations").flat_map(|n| children(n, "technical")).next();
                    let string_hint = technical
                        .and_then(|t| text_of(t, "string"))
                        .and_then(|s| s.parse::<u8>().ok())
                        .filter(|s| *s > 0);
                    let fingering = technical.and_then(|t| text_of(t, "fingering"));
                    out.events.push(NoteEvent {
                        idx: this_idx,
                        measure_idx,
                        measure: number.clone(),
                        onset: onset.max(0),
                        duration,
                        midi: written + shift,
                        chord,
                        grace,
                        tie_stop,
                        voice: text_of(el, "voice").unwrap_or_else(|| "1".into()),
                        string_hint,
                        fingering,
                    });
                }
                _ => {}
            }
        }
    }
    out.note_count = idx;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
<part-list><score-part id="P1"><part-name>Guitar</part-name></score-part>
<score-part id="P2"><part-name>Cello</part-name></score-part></part-list>
<part id="P1">
<measure number="1">
<attributes><divisions>2</divisions><transpose><diatonic>0</diatonic><chromatic>0</chromatic><octave-change>-1</octave-change></transpose></attributes>
<direction placement="above"><direction-type><words>C. 7a</words></direction-type></direction>
<note><grace/><pitch><step>C</step><alter>1</alter><octave>5</octave></pitch><voice>1</voice><type>eighth</type></note>
<note><pitch><step>E</step><octave>5</octave></pitch><duration>2</duration><voice>1</voice><type>quarter</type>
<notations><technical><string>1</string><fingering>4</fingering></technical></notations></note>
<note><pitch><step>E</step><octave>3</octave></pitch><duration>4</duration><voice>1</voice><type>half</type><tie type="start"/></note>
<note><chord/><pitch><step>B</step><octave>3</octave></pitch><duration>4</duration><voice>1</voice><type>half</type></note>
<backup><duration>6</duration></backup>
<note><rest/><duration>2</duration><voice>2</voice><type>quarter</type></note>
<note><pitch><step>A</step><octave>2</octave></pitch><duration>4</duration><voice>2</voice><type>half</type></note>
</measure>
<measure number="2">
<note><pitch><step>E</step><octave>3</octave></pitch><duration>2</duration><voice>1</voice><type>quarter</type><tie type="stop"/></note>
</measure>
</part>
<part id="P2"><measure number="1"><note><rest/><duration>6</duration></note></measure><measure number="2"><note><rest/><duration>2</duration></note></measure></part>
</score-partwise>"#;

    #[test]
    fn extracts_events_with_onsets_and_hints() {
        let p = extract(XML, "P1", -1).unwrap();
        assert_eq!(p.part_name, "Guitar");
        assert!(p.has_transpose);
        assert_eq!(p.octave_change, -1);
        assert_eq!(p.note_count, 7);
        assert_eq!(p.measure_numbers, vec!["1", "2"]);
        assert_eq!(
            p.marks,
            vec![PositionMark {
                measure_idx: 0,
                onset: 0,
                fret: 7
            }]
        );
        let e = &p.events;
        assert_eq!(e.len(), 6);
        // grace C#5 written -> sounding C#4 = 61, shares onset 0
        assert!(e[0].grace);
        assert_eq!((e[0].idx, e[0].onset, e[0].midi), (0, 0, 61));
        // E5 written -> E4 = 64 with hints
        assert_eq!((e[1].idx, e[1].onset, e[1].midi), (1, 0, 64));
        assert_eq!(e[1].string_hint, Some(1));
        assert_eq!(e[1].fingering.as_deref(), Some("4"));
        // E3 written -> E2 = 40 at onset 2, tie start
        assert_eq!((e[2].idx, e[2].onset, e[2].midi, e[2].tie_stop), (2, 2, 40, false));
        // chord B3 -> B2 = 47 shares onset 2
        assert!(e[3].chord);
        assert_eq!((e[3].idx, e[3].onset, e[3].midi), (3, 2, 47));
        // voice 2 after backup: rest (idx 4) then A2 -> A1 = 33 at onset 2
        assert_eq!((e[4].idx, e[4].onset, e[4].midi, e[4].voice.as_str()), (5, 2, 33, "2"));
        // measure 2 tie stop
        assert_eq!((e[5].idx, e[5].measure_idx, e[5].tie_stop), (6, 1, true));
    }

    #[test]
    fn default_octave_change_applies_without_transpose() {
        let xml = XML.replace(
            "<transpose><diatonic>0</diatonic><chromatic>0</chromatic><octave-change>-1</octave-change></transpose>",
            "",
        );
        let p = extract(&xml, "P1", -1).unwrap();
        assert!(!p.has_transpose);
        assert_eq!(p.events[1].midi, 64);
        let p0 = extract(&xml, "P1", 0).unwrap();
        assert_eq!(p0.events[1].midi, 76);
    }

    #[test]
    fn finds_the_fretted_part() {
        assert_eq!(find_part(XML, "guitar").unwrap(), Some(("P1".into(), "Guitar".into())));
        assert_eq!(
            find_part(XML, "classical guitar").unwrap().map(|p| p.0),
            Some("P1".into())
        );
        assert_eq!(find_part(XML, "ukulele").unwrap(), None);
    }

    #[test]
    fn parses_position_marks() {
        for (text, want) in [
            ("C. 7a", Some(7)),
            ("C.7a", Some(7)),
            ("C 2ª", Some(2)),
            ("C7", Some(7)),
            ("CVII", Some(7)),
            ("C. IX", Some(9)),
            ("B. V", Some(5)),
            ("Barré IX", Some(9)),
            ("VII", Some(7)),
            ("IV", Some(4)),
            ("XII", Some(12)),
            ("ritard.", None),
            ("a tempo", None),
            ("cresc.", None),
            ("C", None),
            ("C. 40a", None),
        ] {
            assert_eq!(parse_position_mark(text), want, "{text}");
        }
    }
}

//! Deterministic assembly of realised note events into a complete MusicXML score.
//!
//! The source score contributes its measure skeleton (divisions, time and key, barlines and
//! directions); the realised events contribute the notes of each new instrument. Clefs,
//! transpositions, rests, ties, tuplets and range corrections are applied here, in code.

use super::instruments::ArrInstrument;
use super::{RealizedMeasure, RealizedNote};
use crate::musicxml::parse_doc;
use crate::tab::instrument::parse_pitch;
use anyhow::{anyhow, Context, Result};
use roxmltree::Node;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

/// Per-measure facts taken from the source score (its first part), used to lay out every new part.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureSkeleton {
    /// The `number` attribute.
    pub number: String,
    /// `implicit="yes"` (pickup).
    pub implicit: bool,
    /// Divisions in force for this measure.
    pub divisions: i64,
    pub beats: u32,
    pub beat_type: u32,
    /// Measure length in divisions.
    pub length: i64,
    /// Some only on the first measure and when it changes.
    pub key_fifths: Option<i8>,
    /// True on the first measure and when the time signature changes.
    pub time_changed: bool,
    /// True on the first measure and when divisions change.
    pub divisions_changed: bool,
    /// Raw `<barline>` elements of the source measure, in order.
    pub barlines: Vec<String>,
    /// Raw tempo / words / rehearsal / segno / coda / dynamics directions, without staff and voice.
    pub directions: Vec<String>,
}

/// The result of [`assemble`].
pub struct Assembled {
    pub musicxml: String,
    pub warnings: Vec<String>,
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn child_text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name).and_then(|c| c.text()).map(str::trim)
}

fn leading_int(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Remove every `<name>…</name>` element (no attributes, no nesting) from a raw XML string.
fn remove_element(s: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut out = s.to_string();
    while let Some(a) = out.find(&open) {
        match out[a..].find(&close) {
            Some(b) => out.replace_range(a..a + b + close.len(), ""),
            None => break,
        }
    }
    out
}

/// Read the measure skeleton from the first part of the source score.
pub fn skeleton(source_xml: &str) -> Result<Vec<MeasureSkeleton>> {
    let doc = parse_doc(source_xml).context("source is not well-formed MusicXML")?;
    let part = child(doc.root_element(), "part").ok_or_else(|| anyhow!("source has no <part>"))?;
    let (mut divisions, mut beats, mut beat_type) = (1i64, 4u32, 4u32);
    let mut fifths: Option<i8> = None;
    let mut out = Vec::new();
    for (idx, m) in part.children().filter(|n| n.has_tag_name("measure")).enumerate() {
        let first = idx == 0;
        let mut time_changed = first;
        let mut divisions_changed = first;
        let mut key_fifths = None;
        for attrs in m.children().filter(|n| n.has_tag_name("attributes")) {
            if let Some(d) = child_text(attrs, "divisions").and_then(|t| t.parse::<i64>().ok()) {
                if d > 0 {
                    if d != divisions {
                        divisions_changed = true;
                    }
                    divisions = d;
                }
            }
            if let Some(k) = child(attrs, "key")
                .and_then(|k| child_text(k, "fifths"))
                .and_then(|t| t.parse::<i8>().ok())
            {
                if first || fifths != Some(k) {
                    key_fifths = Some(k);
                }
                fifths = Some(k);
            }
            if let Some(t) = child(attrs, "time") {
                let b = child_text(t, "beats").and_then(leading_int);
                let bt = child_text(t, "beat-type").and_then(leading_int);
                if let (Some(b), Some(bt)) = (b, bt) {
                    if bt > 0 && (b != beats || bt != beat_type) {
                        time_changed = true;
                    }
                    if bt > 0 {
                        beats = b;
                        beat_type = bt;
                    }
                }
            }
        }
        if first && key_fifths.is_none() {
            key_fifths = Some(fifths.unwrap_or(0));
        }
        let bt = i64::from(beat_type.max(1));
        let length = (divisions * i64::from(beats) * 4 + bt / 2) / bt;
        let barlines = m
            .children()
            .filter(|n| n.has_tag_name("barline"))
            .map(|n| source_xml[n.range()].to_string())
            .collect();
        let directions = m
            .children()
            .filter(|n| n.has_tag_name("direction"))
            .filter(|d| {
                d.children().filter(|t| t.has_tag_name("direction-type")).any(|t| {
                    t.descendants().any(|e| {
                        e.is_element()
                            && matches!(
                                e.tag_name().name(),
                                "metronome" | "words" | "rehearsal" | "segno" | "coda" | "dynamics"
                            )
                    })
                })
            })
            .map(|n| remove_element(&remove_element(&source_xml[n.range()], "staff"), "voice"))
            .collect();
        out.push(MeasureSkeleton {
            number: m.attribute("number").unwrap_or("").to_string(),
            implicit: m.attribute("implicit") == Some("yes"),
            divisions,
            beats,
            beat_type,
            length,
            key_fifths,
            time_changed,
            divisions_changed,
            barlines,
            directions,
        });
    }
    if out.is_empty() {
        return Err(anyhow!("source has no measures"));
    }
    Ok(out)
}

/// (units of a 128th note, name, dots), longest first, one entry per distinct length.
fn type_table() -> Vec<(i64, &'static str, u8)> {
    const BASES: [(&str, i64); 9] = [
        ("breve", 256),
        ("whole", 128),
        ("half", 64),
        ("quarter", 32),
        ("eighth", 16),
        ("16th", 8),
        ("32nd", 4),
        ("64th", 2),
        ("128th", 1),
    ];
    let mut t = Vec::new();
    for (name, base) in BASES {
        for dots in 0u8..=2 {
            let num = base * ((1 << (dots + 1)) - 1);
            if num % (1 << dots) == 0 {
                t.push((num >> dots, name, dots));
            }
        }
    }
    t.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
    t.dedup_by_key(|e| e.0);
    t
}

/// MusicXML note type (and dots) for a duration in divisions. `tuplet` (actual, normal) scales the
/// duration first. None when the duration is not a (dotted) power-of-two note value.
pub fn note_type(duration: i64, divisions: i64, tuplet: Option<(u32, u32)>) -> Option<(&'static str, u8)> {
    let (a, n) = tuplet.unwrap_or((1, 1));
    if duration <= 0 || divisions <= 0 || a == 0 || n == 0 {
        return None;
    }
    let num = 32 * duration * i64::from(a);
    let den = divisions * i64::from(n);
    if num % den != 0 {
        return None;
    }
    let units = num / den;
    type_table().into_iter().find(|e| e.0 == units).map(|e| (e.1, e.2))
}

type Piece = (i64, Option<(&'static str, u8)>);

/// Split a duration into representable pieces (largest first). At most `max` pieces; any remainder
/// is folded into the last piece. The flag is false when the duration was not exactly representable.
fn pieces(d: i64, divisions: i64, tuplet: Option<(u32, u32)>, max: usize) -> (Vec<Piece>, bool) {
    if let Some(t) = note_type(d, divisions, tuplet) {
        return (vec![(d, Some(t))], true);
    }
    let (a, n) = tuplet.unwrap_or((1, 1));
    let cands: Vec<(i64, &'static str, u8)> = type_table()
        .into_iter()
        .filter_map(|(u, name, dots)| {
            let num = u * divisions * i64::from(n);
            let den = 32 * i64::from(a.max(1));
            (num % den == 0 && num / den > 0).then_some((num / den, name, dots))
        })
        .collect();
    let mut rem = d;
    let mut out: Vec<Piece> = Vec::new();
    while rem > 0 && out.len() < max {
        match cands.iter().find(|c| c.0 <= rem) {
            Some(c) => {
                out.push((c.0, Some((c.1, c.2))));
                rem -= c.0;
            }
            None => break,
        }
    }
    let exact = rem == 0;
    if rem > 0 {
        match out.last_mut() {
            Some(l) => l.0 += rem,
            None => out.push((rem, None)),
        }
    }
    (out, exact)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[derive(Debug, Clone)]
struct PitchSpec {
    step: char,
    alter: i32,
    octave: i32,
}

/// Letter, alteration and octave of a name such as "C#4" / "Bb2".
fn parse_spelled(name: &str) -> Option<PitchSpec> {
    let s = name.trim();
    let mut chars = s.chars();
    let step = chars.next()?.to_ascii_uppercase();
    if !"CDEFGAB".contains(step) {
        return None;
    }
    let rest = chars.as_str();
    let alter = rest
        .chars()
        .take_while(|c| matches!(c, '#' | '♯' | 'b' | '♭'))
        .fold(0, |acc, c| match c {
            '#' | '♯' => acc + 1,
            _ => acc - 1,
        });
    let digits: String = rest
        .chars()
        .skip_while(|c| matches!(c, '#' | '♯' | 'b' | '♭'))
        .collect();
    Some(PitchSpec {
        step,
        alter,
        octave: digits.parse().ok()?,
    })
}

fn spell_midi(midi: i32, fifths: i8) -> PitchSpec {
    const SHARP: [(char, i32); 12] = [
        ('C', 0),
        ('C', 1),
        ('D', 0),
        ('D', 1),
        ('E', 0),
        ('F', 0),
        ('F', 1),
        ('G', 0),
        ('G', 1),
        ('A', 0),
        ('A', 1),
        ('B', 0),
    ];
    const FLAT: [(char, i32); 12] = [
        ('C', 0),
        ('D', -1),
        ('D', 0),
        ('E', -1),
        ('E', 0),
        ('F', 0),
        ('G', -1),
        ('G', 0),
        ('A', -1),
        ('A', 0),
        ('B', -1),
        ('B', 0),
    ];
    let table = if fifths < 0 { &FLAT } else { &SHARP };
    let (step, alter) = table[midi.rem_euclid(12) as usize];
    PitchSpec {
        step,
        alter,
        octave: midi.div_euclid(12) - 1,
    }
}

/// Key signature (fifths) as written for a transposing instrument.
fn written_fifths(concert: i8, inst: &ArrInstrument) -> i8 {
    let chromatic = inst.transpose.as_ref().map_or(0, |t| i32::from(t.chromatic));
    if chromatic == 0 {
        return concert;
    }
    let f = i32::from(concert) + 7 * chromatic.rem_euclid(12);
    ((f + 5).rem_euclid(12) - 5) as i8
}

fn match_instrument(instruments: &[ArrInstrument], name: &str) -> Option<usize> {
    let n = name.trim().to_lowercase();
    instruments
        .iter()
        .position(|i| i.name.to_lowercase() == n)
        .or_else(|| {
            n.strip_prefix('p')
                .and_then(|k| k.parse::<usize>().ok())
                .filter(|k| *k >= 1 && *k <= instruments.len())
                .map(|k| k - 1)
        })
        .or_else(|| instruments.iter().position(|i| i.abbreviation.to_lowercase() == n))
}

struct Acc {
    dur: i64,
    grace: bool,
    pitches: Vec<PitchSpec>,
    tie: Option<String>,
    tuplet: Option<(u32, u32)>,
    text: Option<String>,
    mark: Option<&'static str>,
}

enum Item {
    Gap(i64),
    Note(Acc),
}

struct Ctx<'a> {
    inst: &'a ArrInstrument,
    sk: &'a MeasureSkeleton,
    label: String,
    fifths: i8,
    warnings: &'a mut Vec<String>,
}

impl Ctx<'_> {
    fn warn(&mut self, msg: impl std::fmt::Display) {
        self.warnings.push(format!("{}: {msg}", self.label));
    }

    /// Written pitches for sounding pitch names, moved into range; also returns the written MIDI numbers.
    fn resolve(&mut self, names: &[String], written: &mut Vec<i32>) -> Vec<PitchSpec> {
        let mut out = Vec::new();
        let (lo, hi) = self.inst.extended_range;
        let chromatic = self.inst.transpose.as_ref().map_or(0, |t| t.chromatic);
        for name in names {
            let (Some(midi), Some(spec)) = (parse_pitch(name), parse_spelled(name)) else {
                self.warn(format_args!("unreadable pitch {name:?}, ignored"));
                continue;
            };
            let mut shift = 0;
            if midi < lo {
                shift = (lo - midi + 11) / 12;
            } else if midi > hi {
                shift = -((midi - hi + 11) / 12);
            }
            if shift != 0 {
                let dir = if shift > 0 { "up" } else { "down" };
                let amount = match shift.abs() {
                    1 => "an octave".to_string(),
                    n => format!("{n} octaves"),
                };
                self.warn(format_args!("{name} out of range, moved {dir} {amount}"));
            }
            let sounding = midi + 12 * shift;
            let w = self.inst.written_midi(sounding);
            written.push(w);
            out.push(if chromatic == 0 {
                PitchSpec {
                    octave: spec.octave + shift + (w - sounding).div_euclid(12),
                    ..spec
                }
            } else {
                spell_midi(w, self.fifths)
            });
        }
        out
    }

    /// Turn one voice's notes into gaps and notes, dropping what cannot be placed.
    fn items(&mut self, notes: &[&RealizedNote], written: &mut Vec<i32>) -> Vec<Item> {
        let length = self.sk.length;
        let mut sorted = notes.to_vec();
        sorted.sort_by_key(|n| n.onset);
        let mut items = Vec::new();
        let mut cursor = 0i64;
        for n in sorted {
            let mut push = |this: &mut Self, dur: i64, items: &mut Vec<Item>| {
                let pitches = this.resolve(&n.pitches, written);
                items.push(Item::Note(Acc {
                    dur,
                    grace: n.grace,
                    pitches,
                    tie: n.tie.clone(),
                    tuplet: n.tuplet.filter(|t| t.0 > 0 && t.1 > 0),
                    text: n.text.clone(),
                    mark: None,
                }));
            };
            if n.grace {
                push(self, n.duration, &mut items);
                continue;
            }
            if n.duration <= 0 {
                self.warn(format_args!("note at {} has no duration, skipped", n.onset));
                continue;
            }
            if n.onset < cursor {
                self.warn(format_args!("note at {} overlaps the previous note, skipped", n.onset));
                continue;
            }
            if n.onset >= length {
                self.warn(format_args!(
                    "note at {} is beyond the end of the measure, skipped",
                    n.onset
                ));
                continue;
            }
            if n.onset > cursor {
                items.push(Item::Gap(n.onset - cursor));
            }
            let mut dur = n.duration;
            if n.onset + dur > length {
                dur = length - n.onset;
                self.warn(format_args!("note at {} runs past the barline, shortened", n.onset));
            }
            push(self, dur, &mut items);
            cursor = n.onset + dur;
        }
        let tups: Vec<Option<(u32, u32)>> = items
            .iter()
            .map(|i| match i {
                Item::Note(a) if !a.grace => a.tuplet,
                _ => None,
            })
            .collect();
        for (i, item) in items.iter_mut().enumerate() {
            if let (Item::Note(a), Some(t)) = (item, tups[i]) {
                let prev = i > 0 && tups[i - 1] == Some(t);
                let next = tups.get(i + 1) == Some(&Some(t));
                a.mark = match (prev, next) {
                    (false, true) => Some("start"),
                    (true, false) => Some("stop"),
                    _ => None,
                };
            }
        }
        if cursor < length {
            items.push(Item::Gap(length - cursor));
        }
        items
    }
}

#[derive(Default)]
struct NoteEl<'a> {
    chord: bool,
    grace: bool,
    pitch: Option<&'a PitchSpec>,
    duration: Option<i64>,
    tie_stop: bool,
    tie_start: bool,
    voice: u8,
    ty: Option<(&'static str, u8)>,
    tuplet: Option<(u32, u32)>,
    staff: Option<u8>,
    tup_mark: Option<&'static str>,
    measure_rest: bool,
}

fn write_note(out: &mut String, n: &NoteEl) {
    out.push_str("<note>");
    if n.grace {
        out.push_str("<grace/>");
    }
    if n.chord {
        out.push_str("<chord/>");
    }
    match n.pitch {
        Some(p) => {
            let _ = write!(out, "<pitch><step>{}</step>", p.step);
            if p.alter != 0 {
                let _ = write!(out, "<alter>{}</alter>", p.alter);
            }
            let _ = write!(out, "<octave>{}</octave></pitch>", p.octave);
        }
        None if n.measure_rest => out.push_str("<rest measure=\"yes\"/>"),
        None => out.push_str("<rest/>"),
    }
    if let Some(d) = n.duration {
        let _ = write!(out, "<duration>{d}</duration>");
    }
    if n.tie_stop {
        out.push_str("<tie type=\"stop\"/>");
    }
    if n.tie_start {
        out.push_str("<tie type=\"start\"/>");
    }
    let _ = write!(out, "<voice>{}</voice>", n.voice);
    if let Some((name, dots)) = n.ty {
        let _ = write!(out, "<type>{name}</type>");
        for _ in 0..dots {
            out.push_str("<dot/>");
        }
    }
    if let Some((a, b)) = n.tuplet {
        let _ = write!(
            out,
            "<time-modification><actual-notes>{a}</actual-notes><normal-notes>{b}</normal-notes></time-modification>"
        );
    }
    if let Some(s) = n.staff {
        let _ = write!(out, "<staff>{s}</staff>");
    }
    if n.tie_stop || n.tie_start || n.tup_mark.is_some() {
        out.push_str("<notations>");
        if n.tie_stop {
            out.push_str("<tied type=\"stop\"/>");
        }
        if n.tie_start {
            out.push_str("<tied type=\"start\"/>");
        }
        if let Some(m) = n.tup_mark {
            let _ = write!(out, "<tuplet type=\"{m}\"/>");
        }
        out.push_str("</notations>");
    }
    out.push_str("</note>\n");
}

fn emit_gap(out: &mut String, ctx: &mut Ctx, gap: i64, voice: u8, staff: Option<u8>) {
    let (ps, exact) = pieces(gap, ctx.sk.divisions, None, usize::MAX);
    if !exact {
        ctx.warn(format_args!("rest of {gap} divisions is not a plain note value"));
    }
    for (d, ty) in ps {
        write_note(
            out,
            &NoteEl {
                duration: Some(d),
                voice,
                ty,
                staff,
                ..Default::default()
            },
        );
    }
}

fn emit_note(out: &mut String, ctx: &mut Ctx, a: &Acc, voice: u8, staff: Option<u8>) {
    if let Some(text) = &a.text {
        let _ = write!(
            out,
            "<direction placement=\"above\"><direction-type><words>{}</words></direction-type>",
            esc(text)
        );
        if let Some(s) = staff {
            let _ = write!(out, "<staff>{s}</staff>");
        }
        out.push_str("</direction>\n");
    }
    let div = ctx.sk.divisions;
    if a.grace {
        let ty = note_type(a.dur, div, a.tuplet).unwrap_or(("eighth", 0));
        for (i, p) in a.pitches.iter().enumerate() {
            write_note(
                out,
                &NoteEl {
                    chord: i > 0,
                    grace: true,
                    pitch: Some(p),
                    voice,
                    ty: Some(ty),
                    tuplet: a.tuplet,
                    staff,
                    ..Default::default()
                },
            );
        }
        return;
    }
    let pitched = !a.pitches.is_empty();
    let (ps, exact) = pieces(a.dur, div, a.tuplet, if pitched { 3 } else { usize::MAX });
    if !exact {
        ctx.warn(format_args!("duration {} is not a plain note value", a.dur));
    }
    let tie_in = matches!(a.tie.as_deref(), Some("stop" | "continue"));
    let tie_out = matches!(a.tie.as_deref(), Some("start" | "continue"));
    let n = ps.len();
    for (k, (d, ty)) in ps.into_iter().enumerate() {
        let (first, last) = (k == 0, k + 1 == n);
        let tup_mark = match a.mark {
            Some("start") if first => Some("start"),
            Some("stop") if last => Some("stop"),
            _ => None,
        };
        let base = NoteEl {
            duration: Some(d),
            tie_stop: pitched && (!first || tie_in),
            tie_start: pitched && (!last || tie_out),
            voice,
            ty,
            tuplet: a.tuplet,
            staff,
            tup_mark,
            ..Default::default()
        };
        if pitched {
            for (i, p) in a.pitches.iter().enumerate() {
                write_note(
                    out,
                    &NoteEl {
                        chord: i > 0,
                        pitch: Some(p),
                        ..NoteEl {
                            tup_mark: if i == 0 { tup_mark } else { None },
                            ..clone_el(&base)
                        }
                    },
                );
            }
        } else {
            write_note(out, &base);
        }
    }
}

fn clone_el<'a>(n: &NoteEl<'a>) -> NoteEl<'a> {
    NoteEl {
        chord: n.chord,
        grace: n.grace,
        pitch: n.pitch,
        duration: n.duration,
        tie_stop: n.tie_stop,
        tie_start: n.tie_start,
        voice: n.voice,
        ty: n.ty,
        tuplet: n.tuplet,
        staff: n.staff,
        tup_mark: n.tup_mark,
        measure_rest: n.measure_rest,
    }
}

fn clef_xml(c: &super::instruments::ClefSpec, number: Option<u8>) -> String {
    match number {
        Some(n) => format!(
            "<clef number=\"{n}\"><sign>{}</sign><line>{}</line></clef>",
            c.sign, c.line
        ),
        None => format!("<clef><sign>{}</sign><line>{}</line></clef>", c.sign, c.line),
    }
}

fn is_left_barline(raw: &str) -> bool {
    let tag = raw.split('>').next().unwrap_or("");
    tag.contains("location=\"left\"") || tag.contains("location='left'")
}

/// Build a score-partwise document with one part per instrument from the realised measures.
pub fn assemble(
    source_xml: &str,
    skeleton: &[MeasureSkeleton],
    instruments: &[ArrInstrument],
    realized: &[RealizedMeasure],
) -> Result<Assembled> {
    if skeleton.is_empty() {
        return Err(anyhow!("no measures to assemble"));
    }
    if instruments.is_empty() {
        return Err(anyhow!("no instruments to assemble"));
    }
    let mut warnings: Vec<String> = Vec::new();

    let known: HashSet<&str> = skeleton.iter().map(|s| s.number.as_str()).collect();
    let mut by: HashMap<(usize, &str), Vec<&RealizedNote>> = HashMap::new();
    let mut seen_unknown: HashSet<String> = HashSet::new();
    let mut seen_measure: HashSet<(usize, &str)> = HashSet::new();
    for rm in realized {
        match match_instrument(instruments, &rm.instrument) {
            None => {
                if seen_unknown.insert(rm.instrument.to_lowercase()) {
                    warnings.push(format!(
                        "unknown instrument {:?}: its measures were dropped",
                        rm.instrument
                    ));
                }
            }
            Some(i) if !known.contains(rm.measure.as_str()) => {
                if seen_measure.insert((i, rm.measure.as_str())) {
                    warnings.push(format!(
                        "{}: measure {} does not exist in the source, dropped",
                        instruments[i].name, rm.measure
                    ));
                }
            }
            Some(i) => by.entry((i, rm.measure.as_str())).or_default().extend(rm.notes.iter()),
        }
    }

    // Header.
    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str(
        "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \
         \"http://www.musicxml.org/dtds/partwise.dtd\">\n",
    );
    xml.push_str("<score-partwise version=\"4.0\">\n");
    const ENCODING: &str = "<encoding><software>notebender</software></encoding>";
    let mut has_identification = false;
    if let Ok(doc) = parse_doc(source_xml) {
        let root = doc.root_element();
        if let Some(w) = child(root, "work") {
            xml.push_str(&source_xml[w.range()]);
            xml.push('\n');
        }
        if let Some(id) = child(root, "identification") {
            let raw = &source_xml[id.range()];
            let has_encoding = child(id, "encoding").is_some();
            match raw.rfind("</identification>") {
                Some(pos) if !has_encoding => {
                    xml.push_str(&raw[..pos]);
                    xml.push_str(ENCODING);
                    xml.push_str(&raw[pos..]);
                }
                Some(_) => xml.push_str(raw),
                None => {
                    let _ = write!(xml, "<identification>{ENCODING}</identification>");
                }
            }
            xml.push('\n');
            has_identification = true;
        }
    }
    if !has_identification {
        let _ = writeln!(xml, "<identification>{ENCODING}</identification>");
    }

    xml.push_str("<part-list>\n");
    for (i, inst) in instruments.iter().enumerate() {
        let n = i + 1;
        let _ = writeln!(
            xml,
            "<score-part id=\"P{n}\"><part-name>{name}</part-name><part-abbreviation>{abbr}</part-abbreviation>\
             <score-instrument id=\"P{n}-I1\"><instrument-name>{name}</instrument-name></score-instrument>\
             <midi-instrument id=\"P{n}-I1\"><midi-channel>{ch}</midi-channel><midi-program>{prog}</midi-program>\
             </midi-instrument></score-part>",
            name = esc(&inst.name),
            abbr = esc(&inst.abbreviation),
            ch = (i % 16) + 1,
            prog = u32::from(inst.midi_program),
        );
    }
    xml.push_str("</part-list>\n");

    for (pi, inst) in instruments.iter().enumerate() {
        let two_staves = inst.staves == 2;
        let _ = writeln!(xml, "<part id=\"P{}\">", pi + 1);
        let mut clef_cur: Option<(String, u8)> = None;
        let mut fifths = 0i8;
        for (mi, sk) in skeleton.iter().enumerate() {
            if let Some(k) = sk.key_fifths {
                fifths = written_fifths(k, inst);
            }
            let mut ctx = Ctx {
                inst,
                sk,
                label: format!("m{} {}", sk.number, inst.name),
                fifths,
                warnings: &mut warnings,
            };
            let notes: &[&RealizedNote] = by.get(&(pi, sk.number.as_str())).map_or(&[], |v| v.as_slice());
            let mut voices: BTreeMap<u8, Vec<&RealizedNote>> = BTreeMap::new();
            for n in notes {
                let mut v = n.voice.max(1);
                if two_staves && v > 8 {
                    ctx.warn(format_args!("voice {v} does not exist, using voice 8"));
                    v = 8;
                }
                voices.entry(v).or_default().push(n);
            }
            if two_staves {
                if !voices.keys().any(|v| *v <= 4) {
                    voices.insert(1, Vec::new());
                }
                if !voices.keys().any(|v| *v >= 5) {
                    voices.insert(5, Vec::new());
                }
            } else if voices.is_empty() {
                voices.insert(1, Vec::new());
            }
            let mut written = Vec::new();
            let built: Vec<(u8, Option<Vec<Item>>)> = voices
                .iter()
                .map(|(v, ns)| {
                    (
                        *v,
                        if ns.is_empty() {
                            None
                        } else {
                            Some(ctx.items(ns, &mut written))
                        },
                    )
                })
                .collect();

            // Attributes.
            let mut attrs = String::new();
            if sk.divisions_changed {
                let _ = write!(attrs, "<divisions>{}</divisions>", sk.divisions);
            }
            if sk.key_fifths.is_some() {
                let _ = write!(attrs, "<key><fifths>{fifths}</fifths></key>");
            }
            if sk.time_changed {
                let _ = write!(
                    attrs,
                    "<time><beats>{}</beats><beat-type>{}</beat-type></time>",
                    sk.beats, sk.beat_type
                );
            }
            if mi == 0 && two_staves {
                attrs.push_str("<staves>2</staves>");
            }
            if two_staves {
                if mi == 0 {
                    for (n, idx) in [(1u8, 0usize), (2, 1)] {
                        if let Some(c) = inst.clefs.get(idx).or(inst.clefs.first()) {
                            attrs.push_str(&clef_xml(c, Some(n)));
                        }
                    }
                }
            } else {
                let lo = written.iter().min().copied();
                let hi = written.iter().max().copied();
                if clef_cur.is_none() || lo.is_some() {
                    let c = inst.clef_for(lo, hi);
                    let key = (c.sign.to_string(), c.line);
                    if clef_cur.as_ref() != Some(&key) {
                        attrs.push_str(&clef_xml(c, None));
                        clef_cur = Some(key);
                    }
                }
            }
            if mi == 0 {
                if let Some(t) = &inst.transpose {
                    attrs.push_str("<transpose>");
                    if t.diatonic != 0 {
                        let _ = write!(attrs, "<diatonic>{}</diatonic>", t.diatonic);
                    }
                    let _ = write!(attrs, "<chromatic>{}</chromatic>", t.chromatic);
                    if t.octave_change != 0 {
                        let _ = write!(attrs, "<octave-change>{}</octave-change>", t.octave_change);
                    }
                    attrs.push_str("</transpose>");
                }
            }

            let _ = write!(xml, "<measure number=\"{}\"", esc(&sk.number));
            if sk.implicit {
                xml.push_str(" implicit=\"yes\"");
            }
            xml.push_str(">\n");
            if !attrs.is_empty() {
                let _ = writeln!(xml, "<attributes>{attrs}</attributes>");
            }
            if pi == 0 {
                for d in &sk.directions {
                    xml.push_str(d);
                    xml.push('\n');
                }
            }
            for b in sk.barlines.iter().filter(|b| is_left_barline(b)) {
                xml.push_str(b);
                xml.push('\n');
            }

            for (vi, (voice, items)) in built.into_iter().enumerate() {
                if vi > 0 {
                    let _ = writeln!(xml, "<backup><duration>{}</duration></backup>", sk.length);
                }
                let staff = two_staves.then_some(if voice >= 5 { 2 } else { 1 });
                match items {
                    None => write_note(
                        &mut xml,
                        &NoteEl {
                            duration: Some(sk.length),
                            voice,
                            staff,
                            measure_rest: true,
                            ..Default::default()
                        },
                    ),
                    Some(items) => {
                        for item in items {
                            match item {
                                Item::Gap(g) => emit_gap(&mut xml, &mut ctx, g, voice, staff),
                                Item::Note(a) => emit_note(&mut xml, &mut ctx, &a, voice, staff),
                            }
                        }
                    }
                }
            }
            for b in sk.barlines.iter().filter(|b| !is_left_barline(b)) {
                xml.push_str(b);
                xml.push('\n');
            }
            xml.push_str("</measure>\n");
        }
        xml.push_str("</part>\n");
    }
    xml.push_str("</score-partwise>\n");

    crate::musicxml::validate(&xml).context("assembled score is not valid MusicXML")?;
    Ok(Assembled {
        musicxml: xml,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::musicxml::summarize;

    const FIXTURE: &str = include_str!("../../tests/fixtures/simple.musicxml");

    fn rn(onset: i64, duration: i64, pitches: &[&str], voice: u8) -> RealizedNote {
        RealizedNote {
            onset,
            duration,
            pitches: pitches.iter().map(|s| s.to_string()).collect(),
            voice,
            tie: None,
            grace: false,
            tuplet: None,
            text: None,
        }
    }

    fn rm(measure: &str, instrument: &str, notes: Vec<RealizedNote>) -> RealizedMeasure {
        RealizedMeasure {
            measure: measure.into(),
            instrument: instrument.into(),
            notes,
        }
    }

    /// The fixture with divisions 6 (measure length 24).
    fn source6() -> String {
        FIXTURE.replace("<divisions>1</divisions>", "<divisions>6</divisions>")
    }

    fn inst(name: &str) -> ArrInstrument {
        ArrInstrument::resolve(name).unwrap()
    }

    #[test]
    fn skeleton_of_fixture() {
        let sk = skeleton(FIXTURE).unwrap();
        assert_eq!(sk.len(), 2);
        let f = &sk[0];
        assert_eq!(f.number, "1");
        assert_eq!(f.key_fifths, Some(0));
        assert_eq!((f.beats, f.beat_type), (4, 4));
        assert_eq!((f.divisions, f.length), (1, 4));
        assert!(f.time_changed && f.divisions_changed);
        assert!(!sk[1].time_changed && !sk[1].divisions_changed);
        assert_eq!(sk[1].key_fifths, None);
        assert_eq!(skeleton(&source6()).unwrap()[0].length, 24);
    }

    #[test]
    fn note_types() {
        assert_eq!(note_type(4, 4, None), Some(("quarter", 0)));
        assert_eq!(note_type(12, 4, None), Some(("half", 1)));
        assert_eq!(note_type(2, 4, None), Some(("eighth", 0)));
        assert_eq!(note_type(1, 4, None), Some(("16th", 0)));
        assert_eq!(note_type(7, 4, None), Some(("quarter", 2)));
        assert_eq!(note_type(2, 6, Some((3, 2))), Some(("eighth", 0)));
        assert_eq!(note_type(5, 4, None), None);
    }

    #[test]
    fn full_assembly() {
        let src = source6();
        let sk = skeleton(&src).unwrap();
        let insts = [inst("cello"), inst("guitar")];
        let mut c1 = vec![
            rn(0, 6, &["C3"], 1),
            rn(12, 3, &["D3"], 1),
            rn(15, 3, &["C7"], 1),
            rn(21, 3, &["E3"], 1),
        ];
        c1[1].text = Some("pizz. & co".into());
        c1[3].tie = Some("start".into());
        let mut c2 = vec![rn(0, 6, &["E3"], 1)];
        c2[0].tie = Some("stop".into());
        for o in [6, 8, 10] {
            let mut n = rn(o, 2, &["G3"], 1);
            n.tuplet = Some((3, 2));
            c2.push(n);
        }
        let realized = vec![
            rm("1", "Cello", c1),
            rm("2", "cello", c2),
            rm("1", "guitar", vec![rn(0, 24, &["E2", "A2", "C#3"], 1)]),
        ];
        let out = assemble(&src, &sk, &insts, &realized).unwrap();
        let xml = &out.musicxml;
        crate::musicxml::validate(xml).unwrap();
        let s = summarize(xml).unwrap();
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[0].measures, s.parts[1].measures);
        assert!(xml.find("<part-name>Cello").unwrap() < xml.find("<part-name>Guitar").unwrap());
        assert!(
            out.warnings
                .iter()
                .any(|w| w.contains("C7") && w.contains("out of range")),
            "{:?}",
            out.warnings
        );
        assert!(!xml.contains("<step>C</step><octave>7</octave>"));
        assert!(xml.contains("<words>pizz. &amp; co</words>"));
        assert!(xml.contains("<tie type=\"start\"/>") && xml.contains("<tie type=\"stop\"/>"));
        assert!(xml.contains("<tied type=\"start\"/>") && xml.contains("<tied type=\"stop\"/>"));
        assert!(xml.contains("<tuplet type=\"start\"/>") && xml.contains("<tuplet type=\"stop\"/>"));
        assert!(xml.contains("<time-modification>"));
        // Gap between 6 and 12 became a rest.
        assert!(xml.contains("<note><rest/><duration>6</duration>"));
        // Guitar: chord, octave shift, whole-measure rest in m2.
        let guitar = &xml[xml.find("<part id=\"P2\">").unwrap()..];
        assert!(guitar.contains("<octave-change>-1</octave-change>"));
        assert_eq!(guitar.matches("<chord/>").count(), 2);
        assert!(guitar.contains("<note><pitch><step>E</step><octave>3</octave>"));
        assert!(guitar.contains("<note><chord/><pitch><step>C</step><alter>1</alter><octave>4</octave>"));
        assert!(guitar.contains("<rest measure=\"yes\"/>"));
        assert!(!xml.contains("<note><chord/><pitch><step>E</step>"));
    }

    #[test]
    fn clarinet_written_pitch() {
        let src = FIXTURE;
        let sk = skeleton(src).unwrap();
        let realized = vec![rm("1", "clarinet", vec![rn(0, 4, &["C4"], 1)])];
        let out = assemble(src, &sk, &[inst("clarinet")], &realized).unwrap();
        assert!(out.musicxml.contains("<note><pitch><step>D</step><octave>4</octave>"));
        assert!(out
            .musicxml
            .contains("<transpose><diatonic>-1</diatonic><chromatic>-2</chromatic></transpose>"));
    }

    #[test]
    fn two_voices_use_backup() {
        let sk = skeleton(FIXTURE).unwrap();
        let realized = vec![rm("1", "cello", vec![rn(0, 4, &["C3"], 1), rn(0, 2, &["G2"], 2)])];
        let out = assemble(FIXTURE, &sk, &[inst("cello")], &realized).unwrap();
        let x = &out.musicxml;
        assert_eq!(x.matches("<backup><duration>4</duration></backup>").count(), 1);
        // Voice 2 is padded with a half rest.
        assert!(x.contains("<note><rest/><duration>2</duration><voice>2</voice><type>half</type></note>"));
    }

    #[test]
    fn piano_two_staves() {
        let sk = skeleton(FIXTURE).unwrap();
        let realized = vec![rm("1", "piano", vec![rn(0, 4, &["C5"], 1), rn(0, 4, &["C2"], 5)])];
        let out = assemble(FIXTURE, &sk, &[inst("piano")], &realized).unwrap();
        let x = &out.musicxml;
        assert!(x.contains("<staves>2</staves>"));
        assert!(x.contains("<clef number=\"1\">") && x.contains("<clef number=\"2\">"));
        assert!(x.contains("<voice>1</voice><type>whole</type><staff>1</staff>"));
        assert!(x.contains("<voice>5</voice><type>whole</type><staff>2</staff>"));
        assert_eq!(summarize(x).unwrap().parts[0].staves, 2);
    }

    #[test]
    fn unknown_instrument_warns() {
        let sk = skeleton(FIXTURE).unwrap();
        let realized = vec![rm("1", "theremin", vec![rn(0, 4, &["C4"], 1)])];
        let out = assemble(FIXTURE, &sk, &[inst("cello")], &realized).unwrap();
        assert!(out.warnings.iter().any(|w| w.contains("theremin")));
        assert!(out.musicxml.contains("<rest measure=\"yes\"/>"));
    }
}

//! Instruments available to the arranger: ranges, clefs, transpositions, General MIDI programs.
//!
//! Octave-transposing instruments (guitar, double bass, piccolo, ...) use a plain clef plus a
//! `<transpose>` with `octave_change`; nothing in the crate relies on `<clef-octave-change>`.

use crate::tab::instrument::pitch_name;
use anyhow::{anyhow, Result};

/// How an instrument's notation relates to sounding pitch (MusicXML `<transpose>`):
/// sounding = written + chromatic + 12 * octave_change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transposition {
    pub diatonic: i8,
    pub chromatic: i8,
    pub octave_change: i8,
}

impl Transposition {
    /// Semitones to add to a written pitch to get the sounding pitch.
    fn semitones(&self) -> i32 {
        i32::from(self.chromatic) + 12 * i32::from(self.octave_change)
    }
}

/// A clef the instrument may use. `from_midi` is the lowest WRITTEN pitch (MIDI) for which this
/// clef is the right choice; clefs are listed ascending by `from_midi`, the first one is the default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClefSpec {
    pub sign: &'static str,
    pub line: u8,
    pub from_midi: i32,
}

impl ClefSpec {
    fn word(&self) -> &'static str {
        match (self.sign, self.line) {
            ("G", _) => "treble",
            ("F", _) => "bass",
            ("C", 3) => "alto",
            ("C", 4) => "tenor",
            _ => "special",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrInstrument {
    /// Canonical display name, e.g. "Cello".
    pub name: String,
    pub abbreviation: String,
    /// For one-staff instruments: ascending by `from_midi`, first is the default. For two-staff
    /// instruments staff 1 uses `clefs[0]` and staff 2 uses `clefs[1]`.
    pub clefs: Vec<ClefSpec>,
    /// Comfortable sounding range (MIDI) for a competent amateur.
    pub range: (i32, i32),
    /// Absolute sounding limits (MIDI); notes outside are unplayable.
    pub extended_range: (i32, i32),
    pub transpose: Option<Transposition>,
    /// General MIDI program, 1-based.
    pub midi_program: u8,
    pub fretted: bool,
    /// Can sustain several notes at once (guitar, piano, harp, organ, ...).
    pub polyphonic: bool,
    /// Number of staves (2 for piano, harp, organ).
    pub staves: u8,
}

fn treble(from_midi: i32) -> ClefSpec {
    ClefSpec {
        sign: "G",
        line: 2,
        from_midi,
    }
}
fn bass(from_midi: i32) -> ClefSpec {
    ClefSpec {
        sign: "F",
        line: 4,
        from_midi,
    }
}
fn alto(from_midi: i32) -> ClefSpec {
    ClefSpec {
        sign: "C",
        line: 3,
        from_midi,
    }
}
fn tenor(from_midi: i32) -> ClefSpec {
    ClefSpec {
        sign: "C",
        line: 4,
        from_midi,
    }
}

const LOW: i32 = i32::MIN;

/// Start an instrument: treble clef, extended range = range widened by 3 semitones each side.
fn inst(name: &str, abbreviation: &str, range: (i32, i32), midi_program: u8) -> ArrInstrument {
    ArrInstrument {
        name: name.to_string(),
        abbreviation: abbreviation.to_string(),
        clefs: vec![treble(LOW)],
        range,
        extended_range: (range.0 - 3, range.1 + 3),
        transpose: None,
        midi_program,
        fretted: false,
        polyphonic: false,
        staves: 1,
    }
}

impl ArrInstrument {
    fn ext(mut self, lo: i32, hi: i32) -> Self {
        self.extended_range = (lo, hi);
        self
    }
    fn clefs(mut self, clefs: Vec<ClefSpec>) -> Self {
        self.clefs = clefs;
        self
    }
    fn tr(mut self, diatonic: i8, chromatic: i8, octave_change: i8) -> Self {
        self.transpose = Some(Transposition {
            diatonic,
            chromatic,
            octave_change,
        });
        self
    }
    fn fretted(mut self) -> Self {
        self.fretted = true;
        self.polyphonic = true;
        self
    }
    fn poly(mut self) -> Self {
        self.polyphonic = true;
        self
    }
    /// Keyboard-like: two staves (treble + bass), polyphonic, range is a hard limit.
    fn keys(mut self) -> Self {
        self.staves = 2;
        self.clefs = vec![treble(LOW), bass(LOW)];
        self.extended_range = self.range;
        self.poly()
    }
}

/// Every instrument, in listing order.
fn table() -> Vec<ArrInstrument> {
    vec![
        inst("Violin", "Vln.", (55, 96), 41).ext(55, 105),
        inst("Viola", "Vla.", (48, 88), 42)
            .ext(48, 93)
            .clefs(vec![alto(LOW), treble(79)]),
        inst("Cello", "Vc.", (36, 81), 43)
            .ext(36, 91)
            .clefs(vec![bass(LOW), tenor(55), treble(67)]),
        inst("Double bass", "Db.", (28, 60), 44)
            .ext(28, 67)
            .clefs(vec![bass(LOW), tenor(60)])
            .tr(0, 0, -1),
        inst("Guitar", "Gtr.", (40, 79), 25).ext(40, 88).tr(0, 0, -1).fretted(),
        inst("Bass guitar", "Bass", (28, 60), 34)
            .ext(28, 67)
            .clefs(vec![bass(LOW)])
            .tr(0, 0, -1)
            .fretted(),
        inst("Ukulele", "Uke.", (60, 81), 25).ext(55, 84).fretted(),
        inst("Mandolin", "Mand.", (55, 88), 25).ext(55, 93).fretted(),
        inst("Banjo", "Bjo.", (50, 81), 106).ext(50, 86).tr(0, 0, -1).fretted(),
        inst("Flute", "Fl.", (60, 93), 74).ext(60, 96),
        inst("Piccolo", "Picc.", (74, 105), 73).ext(74, 108).tr(0, 0, 1),
        inst("Alto recorder", "Rec.", (65, 91), 75),
        inst("Oboe", "Ob.", (58, 88), 69).ext(58, 93),
        inst("English horn", "E.H.", (52, 83), 70).tr(-4, -7, 0),
        inst("Clarinet", "Cl.", (50, 89), 72).ext(50, 94).tr(-1, -2, 0),
        inst("Bass clarinet", "B.Cl.", (34, 77), 72).tr(-1, -2, -1),
        inst("Bassoon", "Bsn.", (34, 72), 71)
            .ext(34, 75)
            .clefs(vec![bass(LOW), tenor(55)]),
        inst("Soprano sax", "S.Sx.", (56, 88), 65).tr(-1, -2, 0),
        inst("Alto sax", "A.Sx.", (49, 81), 66).tr(-5, -9, 0),
        inst("Tenor sax", "T.Sx.", (44, 76), 67).tr(-1, -2, -1),
        inst("Baritone sax", "B.Sx.", (37, 69), 68).tr(-5, -9, -1),
        inst("Trumpet", "Tpt.", (52, 79), 57).ext(52, 82).tr(-1, -2, 0),
        inst("Horn", "Hn.", (41, 74), 61).ext(35, 77).tr(-4, -7, 0),
        inst("Trombone", "Tbn.", (40, 67), 58)
            .ext(40, 74)
            .clefs(vec![bass(LOW), tenor(55)]),
        inst("Tuba", "Tba.", (26, 60), 59).ext(26, 65).clefs(vec![bass(LOW)]),
        inst("Piano", "Pno.", (21, 108), 1).keys(),
        inst("Harpsichord", "Hpd.", (29, 89), 7).keys(),
        inst("Organ", "Org.", (36, 96), 20).keys(),
        inst("Harp", "Hp.", (24, 103), 47).keys(),
        inst("Marimba", "Mar.", (36, 96), 13)
            .clefs(vec![bass(LOW), treble(60)])
            .poly(),
        inst("Vibraphone", "Vib.", (53, 89), 12).poly(),
        inst("Soprano", "S.", (60, 84), 53),
        inst("Alto", "A.", (53, 77), 53),
        inst("Tenor", "T.", (48, 72), 53).tr(0, 0, -1),
        inst("Baritone", "Bar.", (43, 67), 53).clefs(vec![bass(LOW)]),
        inst("Bass voice", "B.", (40, 64), 53).clefs(vec![bass(LOW)]),
        inst("Voice", "Vo.", (55, 81), 53),
    ]
}

/// Lower-case synonyms (singular, single-spaced) for each canonical name; the canonical name
/// itself is always accepted too.
const SYNONYMS: &[(&str, &str)] = &[
    ("fiddle", "Violin"),
    ("vln", "Violin"),
    ("violoncello", "Cello"),
    ("vc", "Cello"),
    ("vcl", "Cello"),
    ("contrabass", "Double bass"),
    ("upright bass", "Double bass"),
    ("string bass", "Double bass"),
    ("classical guitar", "Guitar"),
    ("acoustic guitar", "Guitar"),
    ("electric guitar", "Guitar"),
    ("nylon guitar", "Guitar"),
    ("electric bass", "Bass guitar"),
    ("bass", "Bass guitar"),
    ("recorder", "Alto recorder"),
    ("bb clarinet", "Clarinet"),
    ("sax", "Alto sax"),
    ("saxophone", "Alto sax"),
    ("alto saxophone", "Alto sax"),
    ("soprano saxophone", "Soprano sax"),
    ("tenor saxophone", "Tenor sax"),
    ("baritone saxophone", "Baritone sax"),
    ("bari sax", "Baritone sax"),
    ("bb trumpet", "Trumpet"),
    ("cornet", "Trumpet"),
    ("french horn", "Horn"),
    ("pianoforte", "Piano"),
    ("keyboard", "Piano"),
    ("mezzo", "Alto"),
    ("mezzo-soprano", "Alto"),
    ("tenor voice", "Tenor"),
    ("baritone voice", "Baritone"),
    ("basso", "Bass voice"),
    ("vocal", "Voice"),
    ("singer", "Voice"),
];

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn lookup(key: &str) -> Option<&'static str> {
    if let Some((_, canon)) = SYNONYMS.iter().find(|(k, _)| *k == key) {
        return Some(canon);
    }
    list().into_iter().find(|n| n.eq_ignore_ascii_case(key))
}

/// Canonical names, for error messages and docs.
pub fn list() -> Vec<&'static str> {
    const NAMES: &[&str] = &[
        "Violin",
        "Viola",
        "Cello",
        "Double bass",
        "Guitar",
        "Bass guitar",
        "Ukulele",
        "Mandolin",
        "Banjo",
        "Flute",
        "Piccolo",
        "Alto recorder",
        "Oboe",
        "English horn",
        "Clarinet",
        "Bass clarinet",
        "Bassoon",
        "Soprano sax",
        "Alto sax",
        "Tenor sax",
        "Baritone sax",
        "Trumpet",
        "Horn",
        "Trombone",
        "Tuba",
        "Piano",
        "Harpsichord",
        "Organ",
        "Harp",
        "Marimba",
        "Vibraphone",
        "Soprano",
        "Alto",
        "Tenor",
        "Baritone",
        "Bass voice",
        "Voice",
    ];
    NAMES.to_vec()
}

impl ArrInstrument {
    /// Case-insensitive lookup with synonyms; unknown -> anyhow error listing `list()`.
    pub fn resolve(name: &str) -> Result<ArrInstrument> {
        let norm = normalize(name);
        let mut candidates = vec![norm.clone()];
        // "clarinet in Bb", "horn in F", ...
        if let Some((base, _)) = norm.split_once(" in ") {
            candidates.push(base.trim().to_string());
        }
        // Plurals.
        for c in candidates.clone() {
            if let Some(stem) = c.strip_suffix('s') {
                candidates.push(stem.to_string());
            }
        }
        for c in &candidates {
            if let Some(canon) = lookup(c) {
                if let Some(i) = table().into_iter().find(|i| i.name == canon) {
                    return Ok(i);
                }
            }
        }
        Err(anyhow!("unknown instrument '{name}'; known: {}", list().join(", ")))
    }

    /// Written MIDI for a sounding MIDI pitch.
    pub fn written_midi(&self, sounding: i32) -> i32 {
        sounding - self.transpose.map_or(0, |t| t.semitones())
    }

    /// Sounding MIDI for a written MIDI pitch.
    pub fn sounding_midi(&self, written: i32) -> i32 {
        written + self.transpose.map_or(0, |t| t.semitones())
    }

    /// Clef to use for a passage whose WRITTEN pitches span `lowest..=highest`: the last clef whose
    /// `from_midi <= lowest`, so a passage that dips into a lower clef's territory stays on it.
    /// With no notes, or on a two-staff instrument, `clefs[0]`.
    pub fn clef_for(&self, lowest: Option<i32>, _highest: Option<i32>) -> &ClefSpec {
        let first = &self.clefs[0];
        let Some(lowest) = lowest else { return first };
        if self.staves > 1 {
            return first;
        }
        self.clefs.iter().rev().find(|c| c.from_midi <= lowest).unwrap_or(first)
    }

    /// Within the absolute sounding limits.
    pub fn in_range(&self, sounding: i32) -> bool {
        (self.extended_range.0..=self.extended_range.1).contains(&sounding)
    }

    /// Within the comfortable range.
    pub fn comfortable(&self, sounding: i32) -> bool {
        (self.range.0..=self.range.1).contains(&sounding)
    }

    /// e.g. "Cello (sounds C2–A5; bass, tenor or treble clef)".
    pub fn describe(&self) -> String {
        let words: Vec<&str> = self.clefs.iter().map(ClefSpec::word).collect();
        let mut clef = match words.as_slice() {
            [] => String::new(),
            [a] => format!("{a} clef"),
            [init @ .., last] => format!("{} or {last} clef", init.join(", ")),
        };
        if let Some(t) = self.transpose {
            let up = -t.semitones();
            if up == 12 {
                clef.push_str(", written an octave up");
            } else if up == -12 {
                clef.push_str(", written an octave down");
            } else if up != 0 {
                let dir = if up > 0 { "up" } else { "down" };
                clef.push_str(&format!(", written {} semitones {dir}", up.abs()));
            }
        }
        let mut s = format!(
            "{} (sounds {}–{}; {clef}",
            self.name,
            pitch_name(self.range.0),
            pitch_name(self.range.1)
        );
        let mut extra = Vec::new();
        if self.fretted {
            extra.push("fretted");
        }
        if self.polyphonic && !self.fretted && self.staves == 1 {
            extra.push("chords");
        }
        if self.fretted {
            extra.push("chords");
        }
        if self.staves > 1 {
            extra.push("two staves");
        }
        if !extra.is_empty() {
            s.push_str("; ");
            s.push_str(&extra.join(", "));
        }
        s.push(')');
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: &str) -> ArrInstrument {
        ArrInstrument::resolve(n).unwrap()
    }

    #[test]
    fn synonyms() {
        assert_eq!(r("fiddle").name, "Violin");
        assert_eq!(r("vc").name, "Cello");
        assert_eq!(r("Clarinet in Bb").name, "Clarinet");
        assert_eq!(r("Horn in F").name, "Horn");
        assert_eq!(r("classical guitar").name, "Guitar");
        assert_eq!(r("  Violas ").name, "Viola");
        assert_eq!(r("bass").name, "Bass guitar");
        assert_eq!(r("Alto   Sax").name, "Alto sax");
    }

    #[test]
    fn transposition() {
        let g = r("guitar");
        assert_eq!(g.written_midi(40), 52);
        assert_eq!(g.sounding_midi(52), 40);
        assert_eq!(r("clarinet").written_midi(60), 62);
        assert_eq!(r("tenor sax").written_midi(60), 74);
        assert_eq!(r("piccolo").written_midi(84), 72);
        assert_eq!(r("flute").written_midi(72), 72);
    }

    #[test]
    fn cello_clefs() {
        let c = r("cello");
        assert_eq!(c.clef_for(Some(40), Some(50)).sign, "F");
        let t = c.clef_for(Some(57), Some(70));
        assert_eq!((t.sign, t.line), ("C", 4));
        assert_eq!(c.clef_for(Some(69), Some(79)).sign, "G");
        assert_eq!(c.clef_for(Some(50), Some(70)).sign, "F");
        assert_eq!(c.clef_for(None, None).sign, "F");
    }

    #[test]
    fn viola_and_piano_clefs() {
        let v = r("viola");
        assert_eq!(v.clef_for(None, None).line, 3);
        assert_eq!(v.clef_for(Some(60), Some(70)).sign, "C");
        assert_eq!(v.clef_for(Some(81), Some(90)).sign, "G");
        let p = r("piano");
        assert_eq!(p.clef_for(Some(30), Some(90)).sign, "G");
        assert_eq!(p.staves, 2);
    }

    #[test]
    fn ranges() {
        let v = r("violin");
        assert!(v.in_range(100));
        assert!(!v.comfortable(100));
        assert!(!v.in_range(50));
        assert!(v.comfortable(60));
    }

    #[test]
    fn describe_text() {
        assert!(r("guitar").describe().contains("written an octave up"));
        assert!(r("cello")
            .describe()
            .starts_with("Cello (sounds C2–A5; bass, tenor or treble clef"));
        assert!(r("piccolo").describe().contains("written an octave down"));
    }

    #[test]
    fn list_and_errors() {
        let names = list();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
        for n in &names {
            assert_eq!(r(n).name, *n);
        }
        assert_eq!(table().len(), names.len());
        let e = ArrInstrument::resolve("kazoo").unwrap_err().to_string();
        assert!(e.contains("known:"), "{e}");
    }
}

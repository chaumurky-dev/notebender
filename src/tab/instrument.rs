//! Fretted instruments: tunings, string counts, octave conventions and fret arithmetic.
//!
//! `Instrument::tuning` is in *physical* order, from the string nearest the player's chin
//! (the bottom line of a TAB staff drawn upside down, i.e. line 6 / 4 / 5) to the string
//! nearest the floor (line 1). `open_midi(1)` is therefore the LAST element. For every
//! instrument except the re-entrant ukulele and the banjo (whose short 5th string is
//! high) this is also lowest-to-highest pitch.

use anyhow::{bail, Result};

/// A fretted instrument: open-string pitches (sounding MIDI, physical order, see the module
/// docs), how its notation is written relative to sounding pitch, and the highest usable fret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// Canonical lower-case name, e.g. "guitar".
    pub name: String,
    /// Open-string sounding pitches as MIDI numbers, from the string nearest the chin to the
    /// string nearest the floor (string 1 = last element).
    pub tuning: Vec<i32>,
    /// MusicXML `<transpose><octave-change>` for the notation of this instrument:
    /// -1 for guitar and bass (written an octave above sounding), 0 otherwise.
    pub octave_change: i8,
    pub max_fret: u8,
    /// Human label for the tuning, e.g. "standard (E2 A2 D3 G3 B3 E4)" or "drop D (D2 A2 D3 G3 B3 E4)".
    pub tuning_label: String,
}

/// A named tuning: display name, lookup keys (normalised), pitches.
type Preset = (&'static str, &'static [&'static str], &'static str);

struct Spec {
    name: &'static str,
    aliases: &'static [&'static str],
    octave_change: i8,
    max_fret: u8,
    standard: &'static str,
    presets: &'static [Preset],
}

const GUITAR_PRESETS: &[Preset] = &[
    ("drop D", &["drop d", "dropped d"], "D2 A2 D3 G3 B3 E4"),
    ("DADGAD", &["dadgad"], "D2 A2 D3 G3 A3 D4"),
    ("open G", &["open g"], "D2 G2 D3 G3 B3 D4"),
    ("open D", &["open d"], "D2 A2 D3 F#3 A3 D4"),
    ("open E", &["open e"], "E2 B2 E3 G#3 B3 E4"),
    (
        "half step down",
        &["half step down", "eb standard", "e flat", "e flat standard"],
        "Eb2 Ab2 Db3 Gb3 Bb3 Eb4",
    ),
    (
        "whole step down",
        &["whole step down", "d standard"],
        "D2 G2 C3 F3 A3 D4",
    ),
    ("drop C", &["drop c"], "C2 G2 C3 F3 A3 D4"),
];

const SPECS: &[Spec] = &[
    Spec {
        name: "guitar",
        aliases: &[
            "guitar",
            "classical guitar",
            "acoustic guitar",
            "electric guitar",
            "6 string guitar",
        ],
        octave_change: -1,
        max_fret: 20,
        standard: "E2 A2 D3 G3 B3 E4",
        presets: GUITAR_PRESETS,
    },
    Spec {
        name: "7-string guitar",
        aliases: &["7 string guitar", "seven string guitar"],
        octave_change: -1,
        max_fret: 20,
        standard: "B1 E2 A2 D3 G3 B3 E4",
        presets: &[],
    },
    Spec {
        name: "bass",
        aliases: &["bass", "bass guitar", "electric bass", "4 string bass"],
        octave_change: -1,
        max_fret: 20,
        standard: "E1 A1 D2 G2",
        presets: &[
            ("5-string", &["5 string", "five string"], "B0 E1 A1 D2 G2"),
            ("drop D", &["drop d", "dropped d"], "D1 A1 D2 G2"),
        ],
    },
    Spec {
        name: "ukulele",
        aliases: &["ukulele", "uke", "soprano ukulele", "concert ukulele", "tenor ukulele"],
        octave_change: 0,
        max_fret: 15,
        standard: "G4 C4 E4 A4",
        presets: &[("low G", &["low g"], "G3 C4 E4 A4")],
    },
    Spec {
        name: "baritone ukulele",
        aliases: &["baritone ukulele", "baritone uke", "bari uke"],
        octave_change: 0,
        max_fret: 15,
        standard: "D3 G3 B3 E4",
        presets: &[],
    },
    Spec {
        name: "mandolin",
        aliases: &["mandolin"],
        octave_change: 0,
        max_fret: 20,
        standard: "G3 D4 A4 E5",
        presets: &[],
    },
    Spec {
        name: "banjo",
        aliases: &["banjo", "5 string banjo", "five string banjo"],
        octave_change: 0,
        max_fret: 22,
        standard: "G4 D3 G3 B3 D4",
        presets: &[("open G", &["open g"], "G4 D3 G3 B3 D4")],
    },
];

/// Lower-case, hyphens/underscores to spaces, whitespace collapsed.
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a space-separated pitch list; unparsable entries make the whole list `None`.
fn pitch_list(s: &str) -> Option<Vec<i32>> {
    s.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(parse_pitch)
        .collect()
}

fn label(name: &str, pitches: &[i32]) -> String {
    let p: Vec<String> = pitches.iter().map(|&m| pitch_name(m)).collect();
    format!("{name} ({})", p.join(" "))
}

/// Parse compact letters ("DADGBE", "BbEbGbDbFBb") against the standard tuning; each letter
/// takes the octave closest (within 6 semitones) to the standard string at that position.
fn parse_compact(s: &str, standard: &[i32]) -> Option<Result<Vec<i32>>> {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != '-')
        .collect();
    if chars.is_empty() || chars.iter().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut pcs = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let base = match chars[i].to_ascii_uppercase() {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        i += 1;
        let mut pc = base;
        match chars.get(i) {
            Some('#' | '♯') => {
                pc += 1;
                i += 1;
            }
            Some('b' | '♭') => {
                pc -= 1;
                i += 1;
            }
            _ => {}
        }
        pcs.push(pc);
    }
    if pcs.len() != standard.len() {
        return Some(Err(anyhow::anyhow!(
            "tuning '{s}' has {} strings but the instrument has {}",
            pcs.len(),
            standard.len()
        )));
    }
    Some(Ok(pcs
        .iter()
        .zip(standard)
        .map(|(&pc, &std)| {
            let mut d = (pc - std).rem_euclid(12);
            if d > 6 {
                d -= 12;
            }
            std + d
        })
        .collect()))
}

impl Instrument {
    /// Resolve an instrument name (case-insensitive, common synonyms accepted) and an optional
    /// tuning: `None`/"standard", a named preset ("drop D", "DADGAD", ...), an explicit pitch list
    /// ("D2 A2 D3 G3 B3 E4") or compact letters ("DADGBE"). All in physical order.
    pub fn resolve(name: &str, tuning: Option<&str>) -> Result<Instrument> {
        let key = normalize(name);
        let Some(spec) = SPECS
            .iter()
            .find(|s| s.name == key || s.aliases.iter().any(|a| normalize(a) == key))
        else {
            let names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
            bail!(
                "unknown instrument '{name}'; supported: {} (also bass guitar, uke, ...)",
                names.join(", ")
            );
        };
        let standard = pitch_list(spec.standard).unwrap_or_default();
        let (pitches, lbl) = match tuning.map(str::trim).filter(|t| !t.is_empty()) {
            None => (standard.clone(), label("standard", &standard)),
            Some(t) => Self::resolve_tuning(spec, &standard, t)?,
        };
        Ok(Instrument {
            name: spec.name.to_string(),
            tuning: pitches,
            octave_change: spec.octave_change,
            max_fret: spec.max_fret,
            tuning_label: lbl,
        })
    }

    fn resolve_tuning(spec: &Spec, standard: &[i32], t: &str) -> Result<(Vec<i32>, String)> {
        let key = normalize(t);
        if key == "standard" {
            return Ok((standard.to_vec(), label("standard", standard)));
        }
        if let Some((display, _, pitches)) = spec.presets.iter().find(|(_, keys, _)| keys.contains(&key.as_str())) {
            let p = pitch_list(pitches).unwrap_or_default();
            return Ok((p.clone(), label(display, &p)));
        }
        let unknown = || {
            anyhow::anyhow!(
                "unknown tuning '{t}' for {}; use a preset (standard, drop D, DADGAD, open G, …), a pitch list like \
                 \"D2 A2 D3 G3 B3 E4\" or letters like DADGBE",
                spec.name
            )
        };
        if t.chars().any(|c| c.is_ascii_digit()) {
            let p = pitch_list(t).ok_or_else(unknown)?;
            if p.len() != standard.len() {
                bail!(
                    "tuning '{t}' has {} strings but the {} has {}",
                    p.len(),
                    spec.name,
                    standard.len()
                );
            }
            return Ok((p.clone(), label("custom", &p)));
        }
        match parse_compact(t, standard) {
            Some(Ok(p)) => Ok((p.clone(), label("custom", &p))),
            Some(Err(e)) => Err(e),
            None => Err(unknown()),
        }
    }

    pub fn strings(&self) -> u8 {
        self.tuning.len().min(u8::MAX as usize) as u8
    }

    /// Open sounding MIDI of `string`, where string 1 is the HIGHEST-pitched string (MusicXML
    /// convention), i.e. the last element of `tuning`. `None` for an out-of-range string number.
    pub fn open_midi(&self, string: u8) -> Option<i32> {
        let s = string as usize;
        if s == 0 || s > self.tuning.len() {
            return None;
        }
        self.tuning.get(self.tuning.len() - s).copied()
    }

    /// Lowest fret at which `string` can be played (other than open): the banjo's short 5th
    /// string cannot be fretted below fret 5.
    fn min_fretted(&self, string: u8) -> u8 {
        if self.name == "banjo" && string == 5 {
            5
        } else {
            1
        }
    }

    /// Fret (relative to the capo) at which `string` sounds `midi` with a capo at `capo`, if possible:
    /// `midi - (open + capo)` must be in `0..=max_fret`. On the banjo's 5th string frets 1-4 are
    /// not playable (the string is short), so those yield `None`.
    pub fn fret_for(&self, string: u8, midi: i32, capo: u8) -> Option<u8> {
        let open = self.open_midi(string)?;
        let fret = midi - (open + capo as i32);
        if fret < 0 || fret > self.max_fret as i32 {
            return None;
        }
        let fret = fret as u8;
        if fret != 0 && fret < self.min_fretted(string) {
            return None;
        }
        Some(fret)
    }

    /// Lowest and highest sounding pitch reachable with the capo (MIDI).
    pub fn range(&self, capo: u8) -> (i32, i32) {
        let lo = self.tuning.iter().copied().min().unwrap_or(0) + capo as i32;
        let hi = self.tuning.iter().copied().max().unwrap_or(0) + capo as i32 + self.max_fret as i32;
        (lo, hi)
    }

    /// e.g. "guitar, standard (E2 A2 D3 G3 B3 E4), capo 2" (capo part omitted when 0).
    pub fn describe(&self, capo: u8) -> String {
        let mut s = format!("{}, {}", self.name, self.tuning_label);
        if capo > 0 {
            s.push_str(&format!(", capo {capo}"));
        }
        s
    }
}

/// Parse a pitch like "E2", "Bb3", "F#4", "C♯4", "Eb2" to MIDI (C4 = 60).
pub fn parse_pitch(s: &str) -> Option<i32> {
    let mut it = s.trim().chars().peekable();
    let base = match it.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let mut acc = 0;
    while let Some(&c) = it.peek() {
        match c {
            '#' | '♯' => acc += 1,
            'b' | '♭' => acc -= 1,
            _ => break,
        }
        it.next();
    }
    let rest: String = it.collect();
    let octave: i32 = rest.parse().ok()?;
    Some((octave + 1) * 12 + base + acc)
}

/// Format MIDI as a pitch name with sharps, e.g. 40 -> "E2", 61 -> "C#4".
pub fn pitch_name(midi: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guitar() -> Instrument {
        Instrument::resolve("guitar", None).unwrap()
    }

    #[test]
    fn standard_guitar() {
        let g = guitar();
        assert_eq!(g.strings(), 6);
        assert_eq!(g.open_midi(1), Some(64));
        assert_eq!(g.open_midi(6), Some(40));
        assert_eq!(g.open_midi(0), None);
        assert_eq!(g.open_midi(7), None);
        assert_eq!(g.octave_change, -1);
        assert_eq!(g.max_fret, 20);
        assert_eq!(g.tuning_label, "standard (E2 A2 D3 G3 B3 E4)");
    }

    #[test]
    fn frets_and_capo() {
        let g = guitar();
        assert_eq!(g.fret_for(6, 40, 0), Some(0));
        assert_eq!(g.fret_for(6, 52, 0), Some(12));
        assert_eq!(g.fret_for(1, 40, 0), None);
        assert_eq!(g.fret_for(6, 60, 0), Some(20));
        assert_eq!(g.fret_for(6, 61, 0), None);
        assert_eq!(g.fret_for(9, 60, 0), None);
        assert_eq!(g.fret_for(6, 42, 2), Some(0));
        assert_eq!(g.fret_for(6, 41, 2), None);
        assert_eq!(g.fret_for(6, 45, 2), Some(3));
    }

    #[test]
    fn ranges() {
        let g = guitar();
        assert_eq!(g.range(0), (40, 84));
        assert_eq!(g.range(2), (42, 86));
    }

    #[test]
    fn describe_strings() {
        let g = guitar();
        assert_eq!(g.describe(0), "guitar, standard (E2 A2 D3 G3 B3 E4)");
        assert_eq!(g.describe(2), "guitar, standard (E2 A2 D3 G3 B3 E4), capo 2");
    }

    #[test]
    fn synonyms() {
        for n in ["Classical Guitar", "acoustic guitar", "ELECTRIC GUITAR", " guitar "] {
            assert_eq!(Instrument::resolve(n, None).unwrap().name, "guitar", "{n}");
        }
        for n in ["bass", "Bass Guitar", "electric bass"] {
            assert_eq!(Instrument::resolve(n, None).unwrap().name, "bass", "{n}");
        }
        assert_eq!(Instrument::resolve("uke", None).unwrap().name, "ukulele");
        assert_eq!(Instrument::resolve("5-string banjo", None).unwrap().name, "banjo");
        assert_eq!(
            Instrument::resolve("baritone ukulele", None).unwrap().tuning,
            vec![50, 55, 59, 64]
        );
        let g7 = Instrument::resolve("7-string guitar", None).unwrap();
        assert_eq!(g7.strings(), 7);
        assert_eq!(g7.open_midi(7), Some(35));
        let m = Instrument::resolve("mandolin", None).unwrap();
        assert_eq!(m.tuning, vec![55, 62, 69, 76]);
        assert_eq!(m.octave_change, 0);
    }

    #[test]
    fn unknown_instrument() {
        let e = Instrument::resolve("sitar", None).unwrap_err().to_string();
        assert!(e.contains("unknown instrument 'sitar'"), "{e}");
        assert!(e.contains("guitar") && e.contains("banjo"), "{e}");
    }

    #[test]
    fn presets() {
        let d = Instrument::resolve("guitar", Some("Drop-D")).unwrap();
        assert_eq!(d.tuning, vec![38, 45, 50, 55, 59, 64]);
        assert_eq!(d.tuning_label, "drop D (D2 A2 D3 G3 B3 E4)");
        let dg = Instrument::resolve("guitar", Some("dadgad")).unwrap();
        assert_eq!(dg.tuning, vec![38, 45, 50, 55, 57, 62]);
        let og = Instrument::resolve("guitar", Some("open  g")).unwrap();
        assert_eq!(og.tuning, vec![38, 43, 50, 55, 59, 62]);
        let eb = Instrument::resolve("guitar", Some("Eb standard")).unwrap();
        assert_eq!(eb.tuning, vec![39, 44, 49, 54, 58, 63]);
        let st = Instrument::resolve("guitar", Some("Standard")).unwrap();
        assert_eq!(st, guitar());
        let b5 = Instrument::resolve("bass", Some("five string")).unwrap();
        assert_eq!(b5.tuning, vec![23, 28, 33, 38, 43]);
        let bd = Instrument::resolve("bass", Some("drop d")).unwrap();
        assert_eq!(bd.tuning, vec![26, 33, 38, 43]);
        // every preset parses fully
        for s in SPECS {
            assert!(!pitch_list(s.standard).unwrap().is_empty());
            for (_, _, p) in s.presets {
                assert!(pitch_list(p).is_some(), "{p}");
            }
        }
        for (_, _, p) in GUITAR_PRESETS {
            assert_eq!(pitch_list(p).unwrap().len(), 6);
        }
    }

    #[test]
    fn compact_letters() {
        let t = Instrument::resolve("guitar", Some("DADGBE")).unwrap();
        assert_eq!(t.tuning, vec![38, 45, 50, 55, 59, 64]);
        assert_eq!(t.tuning_label, "custom (D2 A2 D3 G3 B3 E4)");
        let t = Instrument::resolve("guitar", Some("DADGAD")).unwrap();
        assert_eq!(t.tuning, vec![38, 45, 50, 55, 57, 62]);
        let t = Instrument::resolve("guitar", Some("EbAbDbGbBbEb")).unwrap();
        assert_eq!(t.tuning, vec![39, 44, 49, 54, 58, 63]);
        let t = Instrument::resolve("guitar", Some("DADF#AD")).unwrap();
        assert_eq!(t.tuning, vec![38, 45, 50, 54, 57, 62]);
        let e = Instrument::resolve("guitar", Some("DADG")).unwrap_err().to_string();
        assert!(e.contains("4 strings") && e.contains("has 6"), "{e}");
    }

    #[test]
    fn explicit_list() {
        let t = Instrument::resolve("guitar", Some("D2 A2 D3 G3 B3 E4")).unwrap();
        assert_eq!(t.tuning, vec![38, 45, 50, 55, 59, 64]);
        let t = Instrument::resolve("guitar", Some("D2, G2, D3, G3, B3, D4")).unwrap();
        assert_eq!(t.tuning_label, "custom (D2 G2 D3 G3 B3 D4)");
        let e = Instrument::resolve("guitar", Some("D2 A2 D3")).unwrap_err().to_string();
        assert!(e.contains("3 strings"), "{e}");
        let e = Instrument::resolve("guitar", Some("lute tuning"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown tuning 'lute tuning' for guitar"), "{e}");
        let e = Instrument::resolve("guitar", Some("X2 A2 D3 G3 B3 E4"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown tuning"), "{e}");
    }

    #[test]
    fn pitch_round_trips() {
        assert_eq!(parse_pitch("E2"), Some(40));
        assert_eq!(parse_pitch("C4"), Some(60));
        assert_eq!(parse_pitch("Bb3"), Some(58));
        assert_eq!(parse_pitch("bb3"), Some(58));
        assert_eq!(parse_pitch("F#4"), Some(66));
        assert_eq!(parse_pitch("C♯4"), Some(61));
        assert_eq!(parse_pitch("Eb2"), Some(39));
        assert_eq!(parse_pitch("B0"), Some(23));
        assert_eq!(parse_pitch("C-1"), Some(0));
        assert_eq!(parse_pitch("E"), None);
        assert_eq!(parse_pitch("H2"), None);
        assert_eq!(parse_pitch(""), None);
        assert_eq!(pitch_name(40), "E2");
        assert_eq!(pitch_name(61), "C#4");
        assert_eq!(pitch_name(0), "C-1");
        for m in 0..128 {
            assert_eq!(parse_pitch(&pitch_name(m)), Some(m));
        }
    }

    #[test]
    fn ukulele_reentrant() {
        let u = Instrument::resolve("ukulele", None).unwrap();
        assert_eq!(u.strings(), 4);
        assert_eq!(u.open_midi(4), Some(67));
        assert_eq!(u.open_midi(1), Some(69));
        assert_eq!(u.max_fret, 15);
        assert_eq!(u.fret_for(4, 67, 0), Some(0));
        assert_eq!(u.range(0), (60, 84));
        let low = Instrument::resolve("ukulele", Some("low G")).unwrap();
        assert_eq!(low.open_midi(4), Some(55));
    }

    #[test]
    fn banjo_fifth_string() {
        let b = Instrument::resolve("banjo", None).unwrap();
        assert_eq!(b.strings(), 5);
        assert_eq!(b.open_midi(5), Some(67));
        assert_eq!(b.open_midi(4), Some(50));
        assert_eq!(b.open_midi(1), Some(62));
        assert_eq!(b.fret_for(5, 67, 0), Some(0));
        for m in 68..72 {
            assert_eq!(b.fret_for(5, m, 0), None, "{m}");
        }
        assert_eq!(b.fret_for(5, 72, 0), Some(5));
        // other strings are unaffected
        assert_eq!(b.fret_for(4, 51, 0), Some(1));
        assert_eq!(b.max_fret, 22);
    }
}

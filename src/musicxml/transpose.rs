//! Transposition of score-partwise MusicXML.

use super::{join, matching_end, parse_doc, tokenize, Kind, Token};
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Major,
    Minor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub fifths: i8,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot parse key {input:?}: {reason}")]
pub struct KeyParseError {
    pub input: String,
    pub reason: String,
}

/// Position on the line of fifths of a natural letter (C = 0).
fn letter_pos(c: char) -> Option<i32> {
    Some(match c {
        'F' => -1,
        'C' => 0,
        'G' => 1,
        'D' => 2,
        'A' => 3,
        'E' => 4,
        'B' => 5,
        _ => return None,
    })
}

const LETTERS_BY_FIFTH: &[u8; 7] = b"FCGDAEB";

fn pos_to_step_alter(p: i32) -> (char, i8) {
    let q = p + 1;
    (
        LETTERS_BY_FIFTH[q.rem_euclid(7) as usize] as char,
        q.div_euclid(7) as i8,
    )
}

impl Key {
    pub fn parse(s: &str) -> Result<Key, KeyParseError> {
        let err = |reason: &str| KeyParseError {
            input: s.to_string(),
            reason: reason.to_string(),
        };
        let s_trim = s.trim();
        let mut chars = s_trim.chars();
        let first = chars.next().ok_or_else(|| err("empty"))?;
        let upper = first.to_ascii_uppercase();
        let pos = letter_pos(upper).ok_or_else(|| err("unknown note letter"))?;
        let rest: &str = chars.as_str();
        let mut t = rest.trim_start();
        let mut alter = 0i32;
        let lower = t.to_lowercase();
        if t.starts_with('#') || t.starts_with('♯') {
            alter = 1;
            t = &t[t.chars().next().unwrap().len_utf8()..];
        } else if t.starts_with('♭') {
            alter = -1;
            t = &t[t.chars().next().unwrap().len_utf8()..];
        } else if lower.starts_with("sharp") {
            alter = 1;
            t = &t[5..];
        } else if lower.starts_with("flat") {
            alter = -1;
            t = &t[4..];
        } else if lower.starts_with("-sharp") {
            alter = 1;
            t = &t[6..];
        } else if lower.starts_with("-flat") {
            alter = -1;
            t = &t[5..];
        } else if t.starts_with('b') || (t.starts_with('-') && !lower.starts_with("-m")) {
            alter = -1;
            t = &t[1..];
        }
        let m = t.trim().trim_start_matches('-').trim();
        let mode = match m {
            "" => {
                if first.is_ascii_lowercase() {
                    Mode::Minor
                } else {
                    Mode::Major
                }
            }
            "M" => Mode::Major,
            _ => match m.to_lowercase().as_str() {
                "m" | "min" | "minor" => Mode::Minor,
                "maj" | "major" => Mode::Major,
                _ => return Err(err("only major and minor keys are supported")),
            },
        };
        let tonic = pos + 7 * alter;
        let fifths = match mode {
            Mode::Major => tonic,
            Mode::Minor => tonic - 3,
        };
        if !(-7..=7).contains(&fifths) {
            return Err(err("key signature would need more than 7 accidentals"));
        }
        Ok(Key {
            fifths: fifths as i8,
            mode,
        })
    }

    /// ASCII name such as "Bb major" or "F# minor".
    pub fn name(&self) -> String {
        let (c, a) = self.tonic_step_alter();
        let acc = match a {
            0 => "",
            1 => "#",
            2 => "##",
            -1 => "b",
            _ => "bb",
        };
        let mode = match self.mode {
            Mode::Major => "major",
            Mode::Minor => "minor",
        };
        format!("{c}{acc} {mode}")
    }

    pub fn tonic_step_alter(&self) -> (char, i8) {
        let f = self.fifths as i32;
        pos_to_step_alter(match self.mode {
            Mode::Major => f,
            Mode::Minor => f + 3,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub semitones: i32,
    pub steps: i32,
}

impl Interval {
    pub fn fifths_offset(&self) -> i32 {
        7 * self.semitones - 12 * self.steps
    }

    pub fn from_semitones(semitones: i32, source_key: Option<&Key>) -> Interval {
        let centre = semitones as f64 * 7.0 / 12.0;
        let lo = centre.floor() as i32 - 1;
        let hi = centre.ceil() as i32 + 1;
        let mut best: Option<((i32, i32, i32), Interval)> = None;
        for steps in lo..=hi {
            let iv = Interval { semitones, steps };
            let off = iv.fifths_offset();
            let cost = match source_key {
                Some(k) => {
                    let total = (k.fifths as i32 + off).abs();
                    let rank = if total <= 6 {
                        0
                    } else if total == 7 {
                        1
                    } else {
                        2
                    };
                    (rank, total, off.abs())
                }
                None => (0, off.abs(), 0),
            };
            if best.as_ref().is_none_or(|(c, _)| cost < *c) {
                best = Some((cost, iv));
            }
        }
        best.unwrap().1
    }

    pub fn between_keys(from: &Key, to: &Key) -> Interval {
        let (fl, fa) = from.tonic_step_alter();
        let (tl, ta) = to.tonic_step_alter();
        let idx = |c: char| "CDEFGAB".find(c).unwrap() as i32;
        let pc = |c: char, a: i8| ([0, 2, 4, 5, 7, 9, 11][idx(c) as usize] + a as i32).rem_euclid(12);
        let mut semis = (pc(tl, ta) - pc(fl, fa)).rem_euclid(12);
        if semis > 6 {
            semis -= 12;
        }
        let letter_diff = (idx(tl) - idx(fl)).rem_euclid(7);
        let target = semis as f64 * 7.0 / 12.0;
        let steps = (-2..=2)
            .map(|k| letter_diff + 7 * k)
            .min_by(|a, b| {
                (*a as f64 - target)
                    .abs()
                    .partial_cmp(&(*b as f64 - target).abs())
                    .unwrap()
            })
            .unwrap();
        Interval {
            semitones: semis,
            steps,
        }
    }

    pub fn inverted(&self) -> Interval {
        Interval {
            semitones: -self.semitones,
            steps: -self.steps,
        }
    }

    pub fn describe(&self) -> String {
        if self.semitones == 0 && self.steps == 0 {
            return "unchanged".to_string();
        }
        let dir = if self.semitones < 0 || (self.semitones == 0 && self.steps < 0) {
            "down"
        } else {
            "up"
        };
        let steps = self.steps.abs();
        let octs = steps / 7;
        let st = steps % 7;
        let semis = self.semitones.abs() - 12 * octs;
        let (base, perfect) = match st {
            0 => (0, true),
            1 => (2, false),
            2 => (4, false),
            3 => (5, true),
            4 => (7, true),
            5 => (9, false),
            _ => (11, false),
        };
        let diff = semis - base;
        let quality = if perfect {
            match diff {
                0 => "perfect".to_string(),
                d if d > 0 => aug(d),
                d => dim(-d),
            }
        } else {
            match diff {
                0 => "major".to_string(),
                -1 => "minor".to_string(),
                d if d > 0 => aug(d),
                d => dim(-d - 1),
            }
        };
        let n = steps + 1;
        let ord = match n {
            1 => "unison".to_string(),
            2 => "second".into(),
            3 => "third".into(),
            4 => "fourth".into(),
            5 => "fifth".into(),
            6 => "sixth".into(),
            7 => "seventh".into(),
            8 => "octave".into(),
            n => format!("{n}th"),
        };
        let art = if quality.starts_with("augmented") { "an" } else { "a" };
        format!("{dir} {art} {quality} {ord} ({:+} semitones)", self.semitones)
    }
}

fn aug(d: i32) -> String {
    if d == 1 {
        "augmented".into()
    } else {
        format!("{}x augmented", d)
    }
}
fn dim(d: i32) -> String {
    if d <= 1 {
        "diminished".into()
    } else {
        format!("{}x diminished", d)
    }
}

/// The first `<key>` (fifths/mode) in the document, if any.
pub fn detect_key(xml: &str) -> Option<Key> {
    let doc = parse_doc(xml).ok()?;
    key_of(
        doc.descendants()
            .find(|n| n.has_tag_name("key") && n.children().any(|c| c.has_tag_name("fifths")))?,
    )
}

pub(crate) fn key_of(node: roxmltree::Node) -> Option<Key> {
    let fifths: i8 = node
        .children()
        .find(|c| c.has_tag_name("fifths"))?
        .text()?
        .trim()
        .parse()
        .ok()?;
    if !(-7..=7).contains(&fifths) {
        return None;
    }
    let mode = match node
        .children()
        .find(|c| c.has_tag_name("mode"))
        .and_then(|m| m.text())
        .map(|m| m.trim())
    {
        Some("minor") => Mode::Minor,
        _ => Mode::Major,
    };
    Some(Key { fifths, mode })
}

// ---------------------------------------------------------------------------------------------

struct Child {
    text: usize,
    end: usize,
}

/// Direct child `<name>text</name>` within token range [s, e).
fn find_child(t: &[Token], s: usize, e: usize, name: &str) -> Option<Child> {
    let mut depth = 0usize;
    let mut i = s;
    while i < e {
        match &t[i].kind {
            Kind::Start(n) => {
                if depth == 0
                    && n == name
                    && i + 2 < e
                    && t[i + 1].kind == Kind::Text
                    && t[i + 2].kind == Kind::End(name.to_string())
                {
                    return Some(Child {
                        text: i + 1,
                        end: i + 2,
                    });
                }
                depth += 1;
            }
            Kind::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Direct child container element: returns (start index, end index).
fn find_container(t: &[Token], s: usize, e: usize, name: &str) -> Option<(usize, usize)> {
    let mut i = s;
    while i < e {
        if let Kind::Start(n) = &t[i].kind {
            let end = matching_end(t, i)?;
            if n == name {
                return Some((i, end));
            }
            i = end;
        }
        i += 1;
    }
    None
}

fn replace_trimmed(raw: &str, new: &str) -> String {
    let lead = &raw[..raw.len() - raw.trim_start().len()];
    let trail = &raw[raw.trim_end().len()..];
    format!("{lead}{new}{trail}")
}

fn parse_alter(s: &str) -> Option<i32> {
    let f: f64 = s.trim().parse().ok()?;
    if f.fract() == 0.0 && f.abs() < 100.0 {
        Some(f as i32)
    } else {
        None
    }
}

const BASE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// Transposes a step/alter(/octave) group in token range [s, e). Returns the new alter if changed.
fn transpose_group(
    t: &mut [Token],
    s: usize,
    e: usize,
    names: (&str, &str, Option<&str>),
    iv: &Interval,
) -> anyhow::Result<Option<i32>> {
    let (step_n, alter_n, oct_n) = names;
    let Some(step) = find_child(t, s, e, step_n) else {
        return Ok(None);
    };
    let letter = t[step.text].raw.trim().chars().next().unwrap_or('?');
    let Some(idx) = "CDEFGAB".find(letter) else {
        return Ok(None);
    };
    let alter_child = find_child(t, s, e, alter_n);
    let old_alter = match &alter_child {
        Some(c) => match parse_alter(&t[c.text].raw) {
            Some(a) => a,
            None => return Ok(None),
        },
        None => 0,
    };
    let oct_child = match oct_n {
        Some(n) => match find_child(t, s, e, n) {
            Some(c) => Some(c),
            None => return Ok(None),
        },
        None => None,
    };
    let octave = match &oct_child {
        Some(c) => match t[c.text].raw.trim().parse::<i32>() {
            Ok(o) => o,
            Err(_) => return Ok(None),
        },
        None => 0,
    };
    let idx = idx as i32;
    let li = idx + iv.steps;
    let new_idx = li.rem_euclid(7);
    let new_oct = octave + li.div_euclid(7);
    let old_total = octave * 12 + BASE[idx as usize] + old_alter;
    let new_alter = old_total + iv.semitones - (new_oct * 12 + BASE[new_idx as usize]);
    if new_alter.abs() > 2 {
        bail!(
            "transposing {}{} by {} steps / {} semitones needs a {}-alter; choose a different interval spelling",
            letter,
            old_alter,
            iv.steps,
            iv.semitones,
            new_alter
        );
    }
    let new_letter = "CDEFGAB".chars().nth(new_idx as usize).unwrap();
    t[step.text].raw = replace_trimmed(&t[step.text].raw, &new_letter.to_string());
    match &alter_child {
        Some(c) => t[c.text].raw = replace_trimmed(&t[c.text].raw, &new_alter.to_string()),
        None if new_alter != 0 => {
            let ws = match t.get(step.end + 1) {
                Some(n) if n.kind == Kind::Text && n.raw.trim().is_empty() => n.raw.clone(),
                _ => String::new(),
            };
            t[step.end]
                .raw
                .push_str(&format!("{ws}<{alter_n}>{new_alter}</{alter_n}>"));
        }
        None => {}
    }
    if let Some(c) = &oct_child {
        t[c.text].raw = replace_trimmed(&t[c.text].raw, &new_oct.to_string());
    }
    Ok(Some(new_alter))
}

fn accidental_name(alter: i32) -> Option<&'static str> {
    Some(match alter {
        -2 => "flat-flat",
        -1 => "flat",
        0 => "natural",
        1 => "sharp",
        2 => "double-sharp",
        _ => return None,
    })
}

/// Transpose a score-partwise MusicXML document. Returns new XML text.
pub fn transpose(xml: &str, interval: &Interval) -> anyhow::Result<String> {
    {
        let doc = parse_doc(xml).context("not well-formed XML")?;
        let root = doc.root_element().tag_name().name().to_string();
        if root != "score-partwise" {
            bail!("expected a score-partwise document, found <{root}>");
        }
    }
    let mut t = tokenize(xml)?;
    let mut i = 0;
    while i < t.len() {
        let name = match &t[i].kind {
            Kind::Start(n) => n.clone(),
            _ => {
                i += 1;
                continue;
            }
        };
        match name.as_str() {
            "note" => {
                let e = matching_end(&t, i).context("unbalanced <note>")?;
                if let Some((ps, pe)) = find_container(&t, i + 1, e, "pitch") {
                    let new_alter = transpose_group(&mut t, ps + 1, pe, ("step", "alter", Some("octave")), interval)?;
                    if let (Some(a), Some(acc)) = (new_alter, find_child(&t, i + 1, e, "accidental")) {
                        let cur = t[acc.text].raw.trim().to_string();
                        if matches!(
                            cur.as_str(),
                            "flat-flat" | "flat" | "natural" | "sharp" | "double-sharp"
                        ) {
                            if let Some(n) = accidental_name(a) {
                                t[acc.text].raw = replace_trimmed(&t[acc.text].raw, n);
                            }
                        }
                    }
                }
                i = e;
            }
            "harmony" => {
                let e = matching_end(&t, i).context("unbalanced <harmony>")?;
                if let Some((s, en)) = find_container(&t, i + 1, e, "root") {
                    transpose_group(&mut t, s + 1, en, ("root-step", "root-alter", None), interval)?;
                }
                if let Some((s, en)) = find_container(&t, i + 1, e, "bass") {
                    transpose_group(&mut t, s + 1, en, ("bass-step", "bass-alter", None), interval)?;
                }
                i = e;
            }
            "key" => {
                let e = matching_end(&t, i).context("unbalanced <key>")?;
                if find_container(&t, i + 1, e, "key-step").is_none()
                    && !t[i + 1..e]
                        .iter()
                        .any(|k| matches!(&k.kind, Kind::Start(n) | Kind::Empty(n) if n == "key-step"))
                {
                    if let Some(f) = find_child(&t, i + 1, e, "fifths") {
                        if let Ok(old) = t[f.text].raw.trim().parse::<i32>() {
                            let new = old + interval.fifths_offset();
                            if !(-7..=7).contains(&new) {
                                bail!(
                                    "transposed key signature would have {new} fifths (outside -7..=7); choose a different interval spelling"
                                );
                            }
                            t[f.text].raw = replace_trimmed(&t[f.text].raw, &new.to_string());
                        }
                    }
                }
                i = e;
            }
            _ => {}
        }
        i += 1;
    }
    Ok(join(&t))
}

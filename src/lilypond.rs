//! Deterministic fixes applied to LilyPond source produced by `musicxml2ly`.
//!
//! * [`sync_grace_skips`] — when a `\repeat volta` starts with grace notes in one voice but
//!   not in the others, LilyPond draws the repeat bar line after the grace notes, which
//!   looks like a tiny extra measure. The documented remedy is a matching `\grace { s16 … }`
//!   in every other voice; this pass inserts it.

use regex::Regex;
use std::sync::OnceLock;

fn repeat_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\\repeat\s+volta\s+\d+\s*\{)(\s*)(\\grace\s*\{([^}]*)\})?").expect("regex"))
}

fn duration_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:<[^>]*>|[a-grs](?:is|es|isis|eses)?[',]*)(\d+)\.*").expect("regex"))
}

fn block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)^[A-Za-z]\w* = ").expect("regex"))
}

/// Number of rhythmic events inside a `\grace { … }` body.
fn grace_events(body: &str) -> usize {
    duration_re().find_iter(body).count()
}

/// Insert grace skips at repeat starts so that every voice has the same grace timing as the
/// voice with the most grace notes there. Returns the new source and how many skips were added.
pub fn sync_grace_skips(ly: &str) -> (String, usize) {
    // Voice blocks are top-level assignments such as `PartPOneVoiceTwo = \relative … {`.
    let starts: Vec<usize> = block_re().find_iter(ly).map(|m| m.start()).collect();
    if starts.len() < 2 {
        return (ly.to_string(), 0);
    }
    let mut bounds: Vec<(usize, usize)> = starts.windows(2).map(|w| (w[0], w[1])).collect();
    bounds.push((*starts.last().expect("non-empty"), ly.len()));

    // Reference: for the k-th repeat start, the largest grace group any block has there.
    let mut reference: Vec<usize> = Vec::new();
    for (s, e) in &bounds {
        for (k, cap) in repeat_re().captures_iter(&ly[*s..*e]).enumerate() {
            let n = cap.get(4).map(|m| grace_events(m.as_str())).unwrap_or(0);
            if reference.len() <= k {
                reference.push(0);
            }
            reference[k] = reference[k].max(n);
        }
    }
    if reference.iter().all(|n| *n == 0) {
        return (ly.to_string(), 0);
    }

    let mut out = String::with_capacity(ly.len() + 64);
    out.push_str(&ly[..bounds[0].0]);
    let mut inserted = 0usize;
    for (s, e) in &bounds {
        let chunk = &ly[*s..*e];
        let mut k = 0usize;
        let replaced = repeat_re().replace_all(chunk, |cap: &regex::Captures| {
            let need = reference.get(k).copied().unwrap_or(0);
            k += 1;
            if need > 0 && cap.get(3).is_none() {
                inserted += 1;
                let skips = vec!["s16"; need].join(" ");
                format!("{} \\grace {{ {skips} }}{}", &cap[1], &cap[2])
            } else {
                cap[0].to_string()
            }
        });
        out.push_str(&replaced);
    }
    (out, inserted)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"\version "2.24.0"
PartPOneVoiceOne = \relative c'' {
  \repeat volta 2 {
    c4 d e | % 1
  }
  \key e \major \repeat volta 2 {
    \grace { cis16 b16 } a4 <gis b>4 <e gis b>4 | % 33
  }
}

PartPOneVoiceTwo = \relative a' {
  \repeat volta 2 {
    e,2. | % 1
  }
  \key e \major \repeat volta 2 {
    e,,2. | % 33
  }
}
"#;

    #[test]
    fn adds_grace_skips_to_voices_without_graces() {
        let (out, n) = sync_grace_skips(SRC);
        assert_eq!(n, 1);
        assert!(
            out.contains("\\key e \\major \\repeat volta 2 { \\grace { s16 s16 }\n    e,,2. | % 33"),
            "{out}"
        );
        // the first repeat (no graces anywhere) and the voice that already has graces are untouched
        assert_eq!(out.matches("\\grace").count(), 2);
        assert!(out.contains("\\repeat volta 2 {\n    c4 d e | % 1"));
        // idempotent
        assert_eq!(sync_grace_skips(&out).1, 0);
    }

    #[test]
    fn leaves_sources_without_grace_repeats_alone() {
        let plain = SRC.replace("\\grace { cis16 b16 } ", "");
        let (out, n) = sync_grace_skips(&plain);
        assert_eq!(n, 0);
        assert_eq!(out, plain);
        assert_eq!(sync_grace_skips("\\relative { c4 }").1, 0);
    }

    #[test]
    fn counts_grace_events() {
        assert_eq!(grace_events(" cis16 b16 "), 2);
        assert_eq!(grace_events("<a c>8 "), 1);
        assert_eq!(grace_events("s16 s16 s16"), 3);
        assert_eq!(grace_events("fis'16. g8"), 2);
    }
}

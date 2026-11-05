//! Page layout control for engraved output: staff size, note spacing, system spacing and
//! page/system targets. Applied deterministically by rewriting the LilyPond source before it is
//! engraved, so it costs nothing and works on model-generated `.ly` as well as `musicxml2ly` output.

use serde::{Deserialize, Serialize};

/// LilyPond's default global staff size in points.
pub const DEFAULT_STAFF_SIZE: f32 = 20.0;
/// LilyPond defaults for `SpacingSpanner` and `system-system-spacing`.
const DEFAULT_SPACING_INCREMENT: f32 = 1.2;
const DEFAULT_SHORTEST_DURATION_SPACE: f32 = 2.0;
const DEFAULT_SYSTEM_BASIC_DISTANCE: f32 = 12.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LayoutOptions {
    /// Overall size multiplier (1.0 = LilyPond default). 0.8 fits noticeably more per page;
    /// 1.2 is large print. Ignored when `staff_size` is set.
    pub scale: Option<f32>,
    /// Absolute global staff size in points (LilyPond default 20).
    pub staff_size: Option<f32>,
    /// Horizontal note-spacing multiplier (1.0 = default). <1 packs more notes per line.
    pub spacing: Option<f32>,
    /// Vertical distance multiplier between systems (1.0 = default).
    pub system_spacing: Option<f32>,
    /// Ask LilyPond to fit the piece on exactly this many pages.
    pub page_count: Option<u32>,
    /// Fixed number of systems per page.
    pub systems_per_page: Option<u32>,
    /// Paper size, e.g. "a4", "letter", "a3", "legal".
    pub paper_size: Option<String>,
}

impl LayoutOptions {
    pub fn is_default(&self) -> bool {
        *self == LayoutOptions::default()
    }

    /// Effective staff size in points.
    pub fn effective_staff_size(&self) -> Option<f32> {
        self.staff_size.or_else(|| self.scale.map(|s| DEFAULT_STAFF_SIZE * s))
    }

    /// Validate ranges so a typo cannot produce a blank or gigantic page.
    pub fn validate(&self) -> Result<(), String> {
        let in_range = |name: &str, v: Option<f32>, lo: f32, hi: f32| match v {
            Some(x) if !(lo..=hi).contains(&x) || !x.is_finite() => {
                Err(format!("{name} = {x} is outside the sensible range {lo}..={hi}"))
            }
            _ => Ok(()),
        };
        in_range("scale", self.scale, 0.4, 2.5)?;
        in_range("staff_size", self.staff_size, 8.0, 50.0)?;
        in_range("spacing", self.spacing, 0.4, 3.0)?;
        in_range("system_spacing", self.system_spacing, 0.4, 3.0)?;
        if self.page_count == Some(0) || self.systems_per_page == Some(0) {
            return Err("page_count and systems_per_page must be at least 1".into());
        }
        if let Some(p) = &self.paper_size {
            if !p.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(format!(
                    "paper_size '{p}' must be a plain LilyPond paper name such as a4 or letter"
                ));
            }
        }
        Ok(())
    }

    /// Human-readable description for reports.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(s) = self.effective_staff_size() {
            parts.push(format!("staff size {s:.1}pt (default {DEFAULT_STAFF_SIZE:.0})"));
        }
        if let Some(s) = self.spacing {
            parts.push(format!("note spacing ×{s}"));
        }
        if let Some(s) = self.system_spacing {
            parts.push(format!("system spacing ×{s}"));
        }
        if let Some(n) = self.page_count {
            parts.push(format!("{n} page(s)"));
        }
        if let Some(n) = self.systems_per_page {
            parts.push(format!("{n} systems per page"));
        }
        if let Some(p) = &self.paper_size {
            parts.push(format!("paper {p}"));
        }
        if parts.is_empty() {
            "default layout".into()
        } else {
            parts.join(", ")
        }
    }

    /// The LilyPond snippet implementing these options (top-level, after `\version`).
    pub fn lilypond_snippet(&self) -> String {
        let mut out = String::from("% --- notebender layout ---\n");
        if let Some(s) = self.effective_staff_size() {
            out.push_str(&format!("#(set-global-staff-size {s:.2})\n"));
        }
        let mut paper = Vec::new();
        if let Some(p) = &self.paper_size {
            paper.push(format!("  #(set-paper-size \"{p}\")"));
        }
        if let Some(f) = self.system_spacing {
            paper.push(format!(
                "  system-system-spacing.basic-distance = #{:.2}",
                DEFAULT_SYSTEM_BASIC_DISTANCE * f
            ));
        }
        if let Some(n) = self.page_count {
            paper.push(format!("  page-count = {n}"));
        }
        if let Some(n) = self.systems_per_page {
            paper.push(format!("  systems-per-page = {n}"));
        }
        if self.page_count.is_some() || self.systems_per_page.is_some() {
            paper.push("  ragged-last-bottom = ##f".into());
        }
        if !paper.is_empty() {
            out.push_str("\\paper {\n");
            out.push_str(&paper.join("\n"));
            out.push_str("\n}\n");
        }
        if let Some(f) = self.spacing {
            out.push_str(&format!(
                "\\layout {{\n  \\context {{\n    \\Score\n    \\override SpacingSpanner.spacing-increment = #{:.2}\n    \\override SpacingSpanner.shortest-duration-space = #{:.2}\n  }}\n}}\n",
                DEFAULT_SPACING_INCREMENT * f,
                DEFAULT_SHORTEST_DURATION_SPACE * f
            ));
        }
        out.push_str("% --- end notebender layout ---\n");
        out
    }
}

/// Insert the layout snippet into LilyPond source, right after the `\version` line (or at the
/// top if there is none). Any earlier notebender layout block is replaced, so re-running with
/// new options does not accumulate.
pub fn apply_to_lilypond(ly: &str, opts: &LayoutOptions) -> String {
    let stripped = strip_existing(ly);
    if opts.is_default() {
        return stripped;
    }
    let snippet = opts.lilypond_snippet();
    if let Some(pos) = stripped.find("\\version") {
        let line_end = stripped[pos..]
            .find('\n')
            .map(|i| pos + i + 1)
            .unwrap_or(stripped.len());
        let mut out = String::with_capacity(stripped.len() + snippet.len() + 1);
        out.push_str(&stripped[..line_end]);
        out.push_str(&snippet);
        out.push_str(&stripped[line_end..]);
        out
    } else {
        format!("{snippet}{stripped}")
    }
}

fn strip_existing(ly: &str) -> String {
    const START: &str = "% --- notebender layout ---\n";
    const END: &str = "% --- end notebender layout ---\n";
    match (ly.find(START), ly.find(END)) {
        (Some(s), Some(e)) if e > s => format!("{}{}", &ly[..s], &ly[e + END.len()..]),
        _ => ly.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_noop() {
        let ly = "\\version \"2.26.0\"\n\\score { c }\n";
        assert_eq!(apply_to_lilypond(ly, &LayoutOptions::default()), ly);
    }

    #[test]
    fn inserts_after_version_and_is_idempotent() {
        let ly = "\\version \"2.26.0\"\n\\score { c }\n";
        let opts = LayoutOptions {
            scale: Some(0.8),
            spacing: Some(0.9),
            page_count: Some(2),
            ..Default::default()
        };
        let once = apply_to_lilypond(ly, &opts);
        assert!(once.starts_with("\\version \"2.26.0\"\n% --- notebender layout ---\n#(set-global-staff-size 16.00)\n"));
        assert!(once.contains("page-count = 2"));
        assert!(once.contains("ragged-last-bottom = ##f"));
        assert!(once.contains("spacing-increment = #1.08"));
        assert!(once.ends_with("\\score { c }\n"));
        let bigger = LayoutOptions {
            scale: Some(1.2),
            ..Default::default()
        };
        let twice = apply_to_lilypond(&once, &bigger);
        assert_eq!(
            twice.matches("notebender layout").count(),
            2,
            "old block replaced:\n{twice}"
        );
        assert!(twice.contains("set-global-staff-size 24.00"));
        assert!(!twice.contains("page-count"));
    }

    #[test]
    fn staff_size_beats_scale_and_validation_catches_nonsense() {
        let opts = LayoutOptions {
            scale: Some(0.5),
            staff_size: Some(18.0),
            ..Default::default()
        };
        assert_eq!(opts.effective_staff_size(), Some(18.0));
        assert!(LayoutOptions {
            scale: Some(0.1),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(LayoutOptions {
            page_count: Some(0),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(LayoutOptions {
            paper_size: Some("a4\" x".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn no_version_line_prepends() {
        let out = apply_to_lilypond(
            "\\score { c }\n",
            &LayoutOptions {
                scale: Some(1.0),
                ..Default::default()
            },
        );
        assert!(out.starts_with("% --- notebender layout ---\n"));
    }
}

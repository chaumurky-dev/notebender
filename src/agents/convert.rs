//! Notation-format conversion. Deterministic when a converter is installed (musicxml2ly for
//! LilyPond, LilyPond for PDF); a mid-tier model otherwise, or for formats with no local tool.

use super::{call, extract_fenced, TaskCall};
use crate::claude::ContentBlock;
use crate::pipeline::{Context, Format, ProgressEvent, ProgressSink, StepReport};
use crate::router::TaskKind;
use anyhow::Result;
use std::path::Path;
use std::time::Instant;

const SYSTEM: &str = r#"You are an expert in music notation file formats. Convert the given MusicXML score into the requested format, preserving all musical content: pitches, rhythms, voices, ties, slurs, articulations, dynamics, lyrics, chord symbols, key/time/clef changes, repeats and part names.

Format notes:
- LilyPond: target version 2.24 or later (`\version "2.24.0"`); use `\relative` or absolute pitches consistently, one `\new Staff` per part (a `\new PianoStaff` for grand staff, `\new TabStaff` for tablature parts, `\new ChordNames` for chord symbols, `\addlyrics` for lyrics), `\header { title composer }`, and correct `\key`/`\time`/`\clef`. The file must compile with `lilypond` without errors.
- Humdrum: `**kern` spines, one per staff (lowest staff leftmost), with `*clef`, `*k[...]`, `*M` time signature, `=` barlines with measure numbers, correct durations (4 = quarter, 8 = eighth, 4. dotted, 12 = triplet eighth), ties with [ ] _, chords space-separated in one token, a `**text` spine for lyrics, and a terminating `*-`. Include `!!!OTL:` and `!!!COM:` reference records.
- ABC: ABC 2.1 with X:, T:, C:, M:, L:, K: headers and one V: per part.

Output ONLY the converted file inside a single fenced code block tagged with the format name (```lilypond, ```humdrum or ```abc). No commentary."#;

pub struct ConvertResult {
    pub text: String,
    pub report: StepReport,
}

/// Produce `format` from `musicxml` (also on disk at `musicxml_path` for external tools).
pub async fn convert(
    ctx: &Context,
    musicxml: &str,
    musicxml_path: &Path,
    format: Format,
    workdir: &Path,
    progress: &ProgressSink,
) -> Result<ConvertResult> {
    if format == Format::LilyPond && ctx.tools.musicxml2ly.available() {
        let label = "convert → LilyPond (musicxml2ly)";
        progress(ProgressEvent::StepStarted {
            label: label.into(),
            task: None,
            tier: None,
            model: None,
        });
        let started = Instant::now();
        match ctx.tools.musicxml_to_lilypond(musicxml_path, workdir).await {
            Ok((ly, notes)) => {
                let report = super::deterministic_report(label, started.elapsed().as_secs_f64(), notes);
                progress(ProgressEvent::StepFinished { report: report.clone() });
                return Ok(ConvertResult { text: ly, report });
            }
            Err(e) => {
                progress(ProgressEvent::Warning {
                    message: format!("musicxml2ly failed ({e}); falling back to the model"),
                });
            }
        }
    }
    let (name, tags): (&str, &[&str]) = match format {
        Format::LilyPond => ("LilyPond", &["lilypond", "ly"]),
        Format::Humdrum => ("Humdrum **kern", &["humdrum", "kern", "krn"]),
        Format::Abc => ("ABC notation", &["abc"]),
        Format::MusicXml | Format::Pdf => anyhow::bail!("{} is not produced by the converter agent", format.label()),
    };
    let label = format!("convert → {}", format.label());
    let task = TaskCall {
        task: TaskKind::Convert,
        system: SYSTEM.into(),
        content: vec![ContentBlock::text(format!(
            "Convert this score to {name}.\n\n{musicxml}"
        ))],
        schema: None,
        max_tokens: None,
    };
    let outcome = call(ctx, task, &label, progress).await?;
    let text = extract_fenced(&outcome.text, tags)
        .or_else(|| extract_fenced(&outcome.text, &[]))
        .unwrap_or_else(|| outcome.text.trim().to_string());
    if text.is_empty() {
        anyhow::bail!("{label}: empty conversion result");
    }
    Ok(ConvertResult {
        text,
        report: outcome.report,
    })
}

//! Optical music recognition: score images → MusicXML (strong model).
//!
//! Strategy: when `pdftoppm` is available, each page is transcribed in its own request (pages
//! run concurrently, bounded by `limits.omr_concurrency`) and the per-page documents are merged
//! deterministically. Page 1 goes first so that its part list can be handed to the other pages,
//! keeping part ids consistent. Without `pdftoppm` the whole PDF is sent as one document block.

use super::{call, call_on_tier, extract_musicxml, TaskCall};
use crate::claude::ContentBlock;
use crate::musicxml;
use crate::pdf::{self, ScoreImages};
use crate::pipeline::{Context, ProgressEvent, ProgressSink, StepReport};
use crate::router::TaskKind;
use anyhow::{Context as _, Result};
use std::sync::Arc;
use tokio::sync::Semaphore;

const SYSTEM: &str = r#"You are an expert music engraver performing optical music recognition. You will be shown one page (or a whole short PDF) of printed or handwritten sheet music. Transcribe it into MusicXML 4.0, score-partwise, as faithfully as possible.

Requirements:
- Output ONLY the MusicXML document, starting with `<?xml version="1.0" encoding="UTF-8"?>` and `<!DOCTYPE score-partwise PUBLIC "-//Recordare//DTD MusicXML 4.0 Partwise//EN" "http://www.musicxml.org/dtds/partwise.dtd">`, ending with `</score-partwise>`. No prose, no code fences.
- Write one XML element per line with no indentation (keeps the output short).
- Include <work><work-title> and <identification><creator type="composer"> when printed on the page; include <part-list> with <score-part> ids P1, P2, … and meaningful <part-name>s (e.g. "Piano", "Voice", "Guitar"). A grand staff (piano) is ONE part with <staves>2</staves> and <staff> on every note, not two parts.
- First measure of each part must carry <attributes> with <divisions>, <key><fifths>, <time>, and <clef> (one per staff). Choose <divisions> so that every duration, including tuplets and dotted notes, is an integer (e.g. 24 or 480).
- Use the measure numbers printed on the page when present; otherwise number from 1. A pickup measure is <measure number="0" implicit="yes">.
- Represent chords with <chord/>, multiple voices with <voice> and <backup>/<forward>, ties with <tie> + <notations><tied>, slurs, articulations, dynamics (<direction>), tempo marks, lyrics (<lyric><syllabic><text>), chord symbols (<harmony>), repeats and endings (<barline>), fermatas, grace notes, tuplets (<time-modification> + <notations><tuplet>). Rests are <note><rest/>. Include <type> on every note.
- Spell pitches exactly as written (an accidental applies for the rest of the measure); do not transpose. Transposing instruments: write the pitches as printed and add <transpose> in <attributes>.
- Every measure of every part must be rhythmically complete. If something is genuinely illegible, make the most musical guess and add an XML comment <!-- OMR: uncertain ... --> right after the element.
- Fretted instruments (guitar, lute, ukulele, …): the engraver's fingerings direct any tablature made later, so keep them. Left-hand finger numbers become <notations><technical><fingering>n</fingering></technical>, circled string numbers <technical><string>n</string></technical> (1 = highest string), right-hand p/i/m/a <technical><pluck>, and position or barre marks ("C. 7a", "CVII", "B5", "VII") a <direction><direction-type><words> with the text exactly as printed, placed before the note where the mark begins. Guitar notation sounds an octave lower than written: add <transpose><diatonic>0</diatonic><chromatic>0</chromatic><octave-change>-1</octave-change></transpose> to the guitar part's first <attributes>.
- Do not invent music that is not on the page. Do not stop early: transcribe every system on the page."#;

pub struct OmrResult {
    pub musicxml: String,
    pub reports: Vec<StepReport>,
    pub warnings: Vec<String>,
}

/// Transcribe a prepared score.
pub async fn transcribe(ctx: &Context, images: &ScoreImages, progress: &ProgressSink) -> Result<OmrResult> {
    match images {
        ScoreImages::WholePdf(path) => {
            let b64 = pdf::base64_file(path)?;
            let content = vec![
                ContentBlock::pdf(b64, Some("score".into())),
                ContentBlock::text("Transcribe every page of this PDF into one MusicXML document."),
            ];
            let (xml, report, warnings) = transcribe_one(ctx, content, "omr", progress).await?;
            Ok(OmrResult {
                musicxml: xml,
                reports: vec![report],
                warnings,
            })
        }
        ScoreImages::Pages(pages) if pages.len() == 1 => {
            let content = page_content(&pages[0], 1, None)?;
            let (xml, report, warnings) = transcribe_one(ctx, content, "omr page 1", progress).await?;
            Ok(OmrResult {
                musicxml: xml,
                reports: vec![report],
                warnings,
            })
        }
        ScoreImages::Pages(pages) => {
            // Page 1 first: it establishes the part list and the opening attributes.
            let content = page_content(&pages[0], 1, None)?;
            let (first_xml, first_report, mut warnings) = transcribe_one(ctx, content, "omr page 1", progress).await?;
            let context_text = page_context(&first_xml);
            let context_text = Arc::new(context_text);

            let sem = Arc::new(Semaphore::new(ctx.config.limits.omr_concurrency.max(1)));
            let mut handles = Vec::new();
            for (idx, page) in pages.iter().enumerate().skip(1) {
                let ctx = ctx.clone();
                let page = page.clone();
                let progress = progress.clone();
                let sem = sem.clone();
                let context_text = context_text.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore open");
                    let content = page_content(&page, idx + 1, Some(&context_text))?;
                    transcribe_one(&ctx, content, &format!("omr page {}", idx + 1), &progress).await
                }));
            }
            let mut docs = vec![first_xml];
            let mut reports = vec![first_report];
            for (i, h) in handles.into_iter().enumerate() {
                let (xml, report, w) = h
                    .await
                    .context("omr task panicked")?
                    .with_context(|| format!("page {}", i + 2))?;
                docs.push(xml);
                reports.push(report);
                warnings.extend(w);
            }
            let (merged, merge_warnings) = musicxml::merge_pages(&docs).context("merging per-page transcriptions")?;
            warnings.extend(merge_warnings);
            Ok(OmrResult {
                musicxml: merged,
                reports,
                warnings,
            })
        }
    }
}

fn page_content(page: &std::path::Path, number: usize, context: Option<&str>) -> Result<Vec<ContentBlock>> {
    let b64 = pdf::base64_file(page)?;
    let mut text = format!("This is page {number} of the score. Transcribe this page into MusicXML.");
    if let Some(c) = context {
        text.push_str("\n\nThe piece's part list, as established from page 1, is below. Use EXACTLY these part ids and names, in this order, even if a part has only rests on this page (write whole-measure rests). Begin the first measure of each part on this page with full <attributes> (divisions, key, time, clef) as they apply at the top of this page, since the page is transcribed in isolation. Number measures as printed, or continue from the previous page.\n\n");
        text.push_str(c);
    }
    Ok(vec![ContentBlock::image_png(b64), ContentBlock::text(text)])
}

/// Extract what later pages need from page 1's transcription.
fn page_context(first_xml: &str) -> String {
    let mut out = String::new();
    if let (Some(s), Some(e)) = (first_xml.find("<part-list>"), first_xml.find("</part-list>")) {
        out.push_str(&first_xml[s..e + "</part-list>".len()]);
        out.push('\n');
    }
    if let Ok(summary) = musicxml::summarize(first_xml) {
        out.push_str("Page 1 summary: ");
        out.push_str(&summary.describe());
    }
    out
}

/// One OMR request plus up to one repair round if the XML does not validate.
async fn transcribe_one(
    ctx: &Context,
    content: Vec<ContentBlock>,
    label: &str,
    progress: &ProgressSink,
) -> Result<(String, StepReport, Vec<String>)> {
    let task = TaskCall {
        task: TaskKind::Omr,
        system: SYSTEM.into(),
        content: content.clone(),
        schema: None,
        max_tokens: None,
    };
    let outcome = call(ctx, task, label, progress).await?;
    let mut report = outcome.report;
    let mut warnings = Vec::new();
    let xml = match extract_musicxml(&outcome.text) {
        Some(x) => x,
        None => anyhow::bail!("{label}: model reply contained no MusicXML document"),
    };
    match musicxml::validate(&xml) {
        Ok(()) => Ok((xml, report, warnings)),
        Err(err) => {
            progress(ProgressEvent::Log {
                message: format!("{label}: transcription failed validation ({err}); asking for a repair"),
            });
            let tier = report.tier.expect("model step has a tier");
            let mut repair_content = content;
            repair_content.push(ContentBlock::text(format!(
                "Your previous transcription was rejected by the MusicXML validator with this error:\n{err}\n\nHere is the rejected document:\n{xml}\n\nReturn the complete corrected MusicXML document (no commentary)."
            )));
            let repair = TaskCall {
                task: TaskKind::Omr,
                system: SYSTEM.into(),
                content: repair_content,
                schema: None,
                max_tokens: None,
            };
            let fixed = call_on_tier(ctx, repair, tier, &format!("{label} (repair)"), progress).await?;
            report.input_tokens += fixed.report.input_tokens;
            report.output_tokens += fixed.report.output_tokens;
            report.estimated_cost_usd += fixed.report.estimated_cost_usd;
            report.seconds += fixed.report.seconds;
            report.notes.push("one repair round".into());
            let xml2 = extract_musicxml(&fixed.text).context("repair reply contained no MusicXML")?;
            if let Err(e2) = musicxml::validate(&xml2) {
                warnings.push(format!(
                    "{label}: transcription still fails validation after repair: {e2}"
                ));
            }
            Ok((xml2, report, warnings))
        }
    }
}

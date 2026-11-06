//! Shared driver for agents that take a MusicXML document and return a modified one plus notes.

use super::{call, call_on_tier, extract_musicxml, extract_notes, TaskCall};
use crate::claude::ContentBlock;
use crate::musicxml;
use crate::pipeline::{Context, ProgressEvent, ProgressSink, StepReport};
use crate::router::TaskKind;
use anyhow::{Context as _, Result};

pub const OUTPUT_RULES: &str = r#"Output format:
1. The complete modified MusicXML document (score-partwise, one element per line, no indentation, no code fences). Keep every part, measure, attribute and notation you were not asked to change exactly as it was; keep <divisions> consistent; keep every measure rhythmically complete.
2. Then a line containing exactly `---NOTES---`.
3. Then a concise explanation (markdown, ≤ 200 words) of the musical decisions you made and anything the user should double-check."#;

pub struct XmlTaskResult {
    pub musicxml: String,
    pub notes: Option<String>,
    pub report: StepReport,
    pub warnings: Vec<String>,
}

/// Send `score` with `instruction` under `system`; validate the returned document, with one
/// repair round on validation failure.
pub async fn run_xml_task(
    ctx: &Context,
    task: TaskKind,
    system: &str,
    instruction: &str,
    score: &str,
    label: &str,
    progress: &ProgressSink,
) -> Result<XmlTaskResult> {
    let user = format!("{instruction}\n\nCurrent score (MusicXML):\n{score}");
    let content = vec![ContentBlock::text(user)];
    let outcome = call(
        ctx,
        TaskCall {
            task,
            system: system.into(),
            content: content.clone(),
            schema: None,
            max_tokens: None,
        },
        label,
        progress,
    )
    .await?;
    let mut report = outcome.report;
    let mut warnings = Vec::new();
    let xml = extract_musicxml(&outcome.text).with_context(|| format!("{label}: reply contained no MusicXML"))?;
    let mut notes = extract_notes(&outcome.text);
    let (xml, fixes) = musicxml::fix_missing_backups(&xml)?;
    report.notes.extend(fixes);

    let xml = match musicxml::validate(&xml) {
        Ok(()) => xml,
        Err(err) => {
            progress(ProgressEvent::Log {
                message: format!("{label}: result failed validation ({err}); asking for a repair"),
            });
            let tier = report.tier.expect("model step has a tier");
            let mut repair_content = content;
            repair_content.push(ContentBlock::text(format!(
                "Your previous answer was rejected by the MusicXML validator with this error:\n{err}\n\nRejected document:\n{xml}\n\nReturn the complete corrected document in the same output format."
            )));
            let fixed = call_on_tier(
                ctx,
                TaskCall {
                    task,
                    system: system.into(),
                    content: repair_content,
                    schema: None,
                    max_tokens: None,
                },
                tier,
                &format!("{label} (repair)"),
                progress,
            )
            .await?;
            report.input_tokens += fixed.report.input_tokens;
            report.output_tokens += fixed.report.output_tokens;
            report.estimated_cost_usd += fixed.report.estimated_cost_usd;
            report.seconds += fixed.report.seconds;
            report.notes.push("one repair round".into());
            let xml2 = extract_musicxml(&fixed.text).context("repair reply contained no MusicXML")?;
            if extract_notes(&fixed.text).is_some() {
                notes = extract_notes(&fixed.text);
            }
            if let Err(e2) = musicxml::validate(&xml2) {
                warnings.push(format!("{label}: result still fails validation after repair: {e2}"));
            }
            xml2
        }
    };
    Ok(XmlTaskResult {
        musicxml: xml,
        notes,
        report,
        warnings,
    })
}

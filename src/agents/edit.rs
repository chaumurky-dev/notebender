//! Free-form edits described in words (Sonnet).

use super::xml_task::{run_xml_task, XmlTaskResult, OUTPUT_RULES};
use crate::pipeline::{Context, ProgressSink};
use crate::router::TaskKind;
use anyhow::Result;

const SYSTEM: &str = r#"You are an expert music editor and engraver working on MusicXML scores. Apply the user's requested edit precisely and musically. Interpret musical terms the way a professional would (e.g. "simplify" = fewer notes, same harmony and rhythmic feel; "remove repeats" = write out or drop repeat barlines as the user's intent suggests, say which). If an instruction is ambiguous, pick the most conventional reading and state your assumption in the notes. Never silently drop music that was not part of the edit.
"#;

pub async fn edit(ctx: &Context, score: &str, instruction: &str, progress: &ProgressSink) -> Result<XmlTaskResult> {
    let system = format!("{SYSTEM}\n{OUTPUT_RULES}");
    let instruction = format!("Requested edit: {instruction}");
    run_xml_task(ctx, TaskKind::Edit, &system, &instruction, score, "edit", progress).await
}

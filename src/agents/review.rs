//! Cheap QA pass (Haiku): does the result plausibly satisfy the request?

use super::{call, TaskCall};
use crate::claude::ContentBlock;
use crate::musicxml::Summary;
use crate::pipeline::{Context, Plan, ProgressSink, StepReport};
use crate::router::TaskKind;
use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde_json::json;

const SYSTEM: &str = r#"You are the quality-control reviewer for a sheet-music transformation tool. You receive the user's request, the plan that was executed, a structural summary of the original and final scores, and the notes each step produced. You do NOT see the full score; judge consistency only.

Check things like: does the final key signature match a requested transposition; does the final part list match the requested instruments; is tablature present when requested; did the step notes mention anything the user should verify (octave shifts, simplifications, uncertain OMR); did the measure count change unexpectedly. Be concise and concrete. `ok` is false only if something clearly contradicts the request."#;

#[derive(Debug, Clone, Deserialize)]
pub struct Review {
    pub ok: bool,
    pub findings: Vec<String>,
}

pub async fn review(
    ctx: &Context,
    instructions: &str,
    plan: &Plan,
    before: &Summary,
    after: &Summary,
    step_notes: &[(String, String)],
    progress: &ProgressSink,
) -> Result<(Review, StepReport)> {
    let mut user = format!(
        "User request:\n{instructions}\n\nPlan executed:\n{}\n\nOriginal score:\n{}\n\nFinal score:\n{}\n",
        serde_json::to_string_pretty(plan).unwrap_or_default(),
        before.describe(),
        after.describe()
    );
    if !step_notes.is_empty() {
        user.push_str("\nStep notes:\n");
        for (label, notes) in step_notes {
            user.push_str(&format!("## {label}\n{notes}\n"));
        }
    }
    let schema = json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "findings": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["ok", "findings"],
        "additionalProperties": false
    });
    let task = TaskCall {
        task: TaskKind::Review,
        system: SYSTEM.into(),
        content: vec![ContentBlock::text(user)],
        schema: Some(schema),
        max_tokens: Some(2_000),
    };
    let outcome = call(ctx, task, "review", progress).await?;
    let review: Review = serde_json::from_str(outcome.text.trim()).context("reviewer returned invalid JSON")?;
    Ok((review, outcome.report))
}

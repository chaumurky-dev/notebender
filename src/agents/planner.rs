//! Turn the user's free-text request into a [`Plan`] (Haiku, structured output).

use super::{call, TaskCall};
use crate::claude::ContentBlock;
use crate::musicxml::Summary;
use crate::pipeline::{Context, Format, Plan, ProgressSink, Step, StepReport};
use crate::router::TaskKind;
use anyhow::{Context as _, Result};
use serde_json::{json, Value};

const SYSTEM: &str = r#"You are the planning component of notebender, a sheet-music transformation tool. The user has uploaded a score and written, in plain language, what they want done to it. Map the request onto the tool's supported steps.

Supported steps:
- transpose: move the whole piece by an interval or between keys. Use `semitones` for interval requests ("up a half-step" = 1, "down a whole step" = -2, "up a minor third" = 3, "down a fourth" = -5, "up an octave" = 12). Use `from_key`/`to_key` for key requests ("from G to C": from_key "G", to_key "C"; "into Bb" or "to B-flat": to_key "Bb", from_key null so the tool detects it; the score summary may tell you the current key). Key names: letter, optional # or b, optional "major"/"minor" ("F# minor", "Bb", "a minor"). Never set both semitones and keys.
- tablature: add tablature for a fretted instrument (default "guitar"; also bass, ukulele, mandolin, banjo). Record tuning ("drop D", "DADGAD", "standard") and capo fret when mentioned.
- arrange: re-instrument the piece for a list of instruments in score order (e.g. a piano piece "for cello and guitar" → instruments ["cello", "guitar"]). Put any guidance about difficulty, style, who takes the melody, etc. in `notes`.
- edit: any other musical edit that can be described in words and applied to the score ("simplify the left hand", "remove the repeats", "add fingerings", "double the melody an octave up"). Pass the user's wording through as `instruction`.

Output formats (deliverables): musicxml (always included), lilypond, pdf (engraved from LilyPond), humdrum (**kern), abc. Include a format when the user asks for it by name or by obvious synonym (".ly" → lilypond, "kern" → humdrum, "printable"/"engraved"/"sheet" → pdf). If the user names no format, deliver musicxml and pdf.

Ordering: arrange and edit steps first, then transpose, then tablature (tab must be computed after any pitch changes). If a request has nothing to do (e.g. "just transcribe it"), return no steps. Put anything you cannot map onto these steps into `unsupported`, quoting the user's words, rather than guessing. The `summary` is one sentence restating what will be done."#;

pub fn plan_schema() -> Value {
    let nullable = |t: &str| json!({ "anyOf": [ { "type": t }, { "type": "null" } ] });
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "steps": {
                "type": "array",
                "items": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": {
                                "kind": { "const": "transpose" },
                                "semitones": nullable("integer"),
                                "from_key": nullable("string"),
                                "to_key": nullable("string")
                            },
                            "required": ["kind", "semitones", "from_key", "to_key"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "kind": { "const": "tablature" },
                                "instrument": { "type": "string" },
                                "tuning": nullable("string"),
                                "capo": nullable("integer")
                            },
                            "required": ["kind", "instrument", "tuning", "capo"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "kind": { "const": "arrange" },
                                "instruments": { "type": "array", "items": { "type": "string" } },
                                "notes": nullable("string")
                            },
                            "required": ["kind", "instruments", "notes"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "kind": { "const": "edit" },
                                "instruction": { "type": "string" }
                            },
                            "required": ["kind", "instruction"],
                            "additionalProperties": false
                        }
                    ]
                }
            },
            "deliverables": {
                "type": "array",
                "items": { "type": "string", "enum": Format::ALL.iter().map(|f| f.id()).collect::<Vec<_>>() }
            },
            "unsupported": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["summary", "steps", "deliverables", "unsupported"],
        "additionalProperties": false
    })
}

/// Ask the planner model for a plan. `summary` describes the score if it is already known
/// (MusicXML input, or after OMR) so key-relative requests can be resolved.
pub async fn plan(
    ctx: &Context,
    instructions: &str,
    summary: Option<&Summary>,
    progress: &ProgressSink,
) -> Result<(Plan, StepReport)> {
    let mut user = String::new();
    if let Some(s) = summary {
        user.push_str("Score summary:\n");
        user.push_str(&s.describe());
        user.push_str("\n\n");
    }
    user.push_str("User request:\n");
    user.push_str(instructions.trim());

    let task = TaskCall {
        task: TaskKind::Plan,
        system: SYSTEM.to_string(),
        content: vec![ContentBlock::text(user)],
        schema: Some(plan_schema()),
        max_tokens: Some(4_000),
    };
    let outcome = call(ctx, task, "plan", progress).await?;
    let mut plan: Plan = serde_json::from_str(outcome.text.trim())
        .with_context(|| format!("planner returned invalid JSON: {}", outcome.text))?;
    normalize(&mut plan);
    Ok((plan, outcome.report))
}

/// Enforce invariants regardless of what the model returned.
pub fn normalize(plan: &mut Plan) {
    // MusicXML is always delivered; dedupe and keep a stable order.
    if !plan.deliverables.contains(&Format::MusicXml) {
        plan.deliverables.insert(0, Format::MusicXml);
    }
    plan.deliverables.sort();
    plan.deliverables.dedup();
    // Tablature must run after pitch-changing steps.
    let (tabs, others): (Vec<Step>, Vec<Step>) =
        plan.steps.drain(..).partition(|s| matches!(s, Step::Tablature { .. }));
    plan.steps = others.into_iter().chain(tabs).collect();
    for step in &mut plan.steps {
        if let Step::Transpose {
            semitones,
            from_key,
            to_key,
        } = step
        {
            let blank = |k: &mut Option<String>| {
                if k.as_deref().is_some_and(|s| s.trim().is_empty()) {
                    *k = None;
                }
            };
            blank(from_key);
            blank(to_key);
            if to_key.is_some() {
                *semitones = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_closed_objects() {
        let s = plan_schema();
        assert_eq!(s["additionalProperties"], json!(false));
        assert_eq!(s["properties"]["steps"]["items"]["anyOf"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn normalize_orders_and_dedupes() {
        let mut plan = Plan {
            summary: "x".into(),
            steps: vec![
                Step::Tablature {
                    instrument: "guitar".into(),
                    tuning: None,
                    capo: None,
                },
                Step::Transpose {
                    semitones: Some(2),
                    from_key: Some("".into()),
                    to_key: None,
                },
            ],
            deliverables: vec![Format::Pdf, Format::Pdf],
            unsupported: vec![],
        };
        normalize(&mut plan);
        assert!(matches!(plan.steps[0], Step::Transpose { from_key: None, .. }));
        assert!(matches!(plan.steps[1], Step::Tablature { .. }));
        assert_eq!(plan.deliverables, vec![Format::MusicXml, Format::Pdf]);
    }

    #[test]
    fn plan_json_round_trip() {
        let text = r#"{"summary":"s","steps":[{"kind":"transpose","semitones":null,"from_key":"G","to_key":"C"},{"kind":"arrange","instruments":["cello","guitar"],"notes":null}],"deliverables":["lilypond","pdf"],"unsupported":[]}"#;
        let plan: Plan = serde_json::from_str(text).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.deliverables, vec![Format::LilyPond, Format::Pdf]);
    }
}

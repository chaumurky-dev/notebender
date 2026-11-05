//! Model-backed agents. Each agent is a function: inputs → prompt → one Messages API call
//! (sometimes a few) → parsed output. Routing (which tier) is decided by the caller via
//! [`crate::router::Router`], never inside the agent.
//!
//! [`call`] is the shared entry point: it shapes the request for the chosen model (adaptive
//! thinking vs. thinking budget, effort, structured output), streams the response, continues
//! automatically when the output hits `max_tokens`, and produces a [`StepReport`] with token
//! usage and an estimated cost.

pub mod arrange;
pub mod convert;
pub mod edit;
pub mod omr;
pub mod planner;
pub mod review;
pub mod tablature;
pub mod xml_task;

use crate::claude::client::SendOptions;
use crate::claude::{
    ContentBlock, Message, MessagesRequest, OutputConfig, OutputFormat, Role, SystemBlock, Thinking, Usage,
};
use crate::models::Tier;
use crate::pipeline::{Context, ProgressEvent, ProgressSink, StepReport};
use crate::router::TaskKind;
use anyhow::{Context as _, Result};
use serde_json::Value;
use std::time::Instant;

/// How many "please continue" turns to allow when a long output hits `max_tokens`.
const MAX_CONTINUATIONS: usize = 3;

/// One model task, ready to send.
pub struct TaskCall {
    pub task: TaskKind,
    /// Stable instructions; cached across calls with the same prefix.
    pub system: String,
    /// User-turn content (text, images, documents).
    pub content: Vec<ContentBlock>,
    /// JSON schema for structured output, if the task returns JSON.
    pub schema: Option<Value>,
    /// Override the router's max_tokens (e.g. small for classification).
    pub max_tokens: Option<u32>,
}

pub struct TaskOutcome {
    /// Concatenated text of the response (and of any continuation turns).
    pub text: String,
    pub report: StepReport,
    pub stop_reason: Option<String>,
}

/// Build the request body for a task on a given tier, applying the model-family rules.
pub fn shape_request(ctx: &Context, call: &TaskCall, tier: Tier) -> MessagesRequest {
    let route = ctx.router.route(call.task);
    let model = ctx.config.model_id(tier).to_string();
    let max_tokens = call
        .max_tokens
        .unwrap_or(route.max_tokens)
        .min(tier.max_output_tokens());

    // Haiku 4.5 takes a thinking budget; everything newer takes adaptive thinking and effort.
    let thinking = if tier.uses_thinking_budget() {
        None
    } else {
        Some(Thinking::Adaptive { display: None })
    };
    let mut output_config = OutputConfig::default();
    if tier.supports_effort() {
        output_config.effort = Some(route.effort);
    }
    if let Some(schema) = &call.schema {
        output_config.format = Some(OutputFormat::JsonSchema { schema: schema.clone() });
    }
    let output_config = if output_config == OutputConfig::default() {
        None
    } else {
        Some(output_config)
    };

    MessagesRequest {
        model,
        max_tokens,
        system: vec![SystemBlock::cached(call.system.clone())],
        messages: vec![Message::user(call.content.clone())],
        thinking,
        output_config,
        fallbacks: None,
        temperature: None,
        stream: Some(true),
    }
}

/// Run a task on the tier the router selects for it.
pub async fn call(ctx: &Context, task: TaskCall, label: &str, progress: &ProgressSink) -> Result<TaskOutcome> {
    let tier = ctx.router.route(task.task).tier;
    call_on_tier(ctx, task, tier, label, progress).await
}

/// Run a task on an explicit tier (used for escalation/repair rounds).
pub async fn call_on_tier(
    ctx: &Context,
    task: TaskCall,
    tier: Tier,
    label: &str,
    progress: &ProgressSink,
) -> Result<TaskOutcome> {
    let client = ctx
        .client
        .as_ref()
        .with_context(|| format!("step '{label}' needs the Claude API but no API key is configured"))?;
    let mut req = shape_request(ctx, &task, tier);
    let model = req.model.clone();
    progress(ProgressEvent::StepStarted {
        label: label.to_string(),
        task: Some(task.task),
        tier: Some(tier),
        model: Some(model.clone()),
    });

    let started = Instant::now();
    let opts = SendOptions {
        tier: Some(tier),
        on_text: None,
        extra_betas: vec![],
    };
    let mut usage = Usage::default();
    let mut text = String::new();
    let mut notes = Vec::new();
    let mut stop_reason;

    let mut continuations = 0;
    loop {
        let resp = client
            .send(&req, &opts)
            .await
            .with_context(|| format!("step '{label}' ({model})"))?;
        usage.add(&resp.usage);
        let chunk = resp.text();
        text.push_str(&chunk);
        stop_reason = resp.stop_reason.clone();
        if resp.model != model && !resp.model.is_empty() {
            notes.push(format!("served by fallback model {}", resp.model));
        }
        if !resp.hit_max_tokens() || task.schema.is_some() || continuations >= MAX_CONTINUATIONS {
            if resp.hit_max_tokens() {
                notes.push("output truncated at max_tokens".into());
            }
            break;
        }
        continuations += 1;
        notes.push(format!("continuation {continuations} after max_tokens"));
        progress(ProgressEvent::Log {
            message: format!("{label}: output hit the token limit; asking the model to continue"),
        });
        // Append the partial assistant turn and ask for the remainder. (Not a prefill: the last
        // message is a user turn.)
        req.messages.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::text(chunk)],
        });
        req.messages.push(Message::user_text(
            "Your previous message was cut off by the output limit. Continue EXACTLY from where \
             it stopped, without repeating anything already sent and without any preamble.",
        ));
    }

    let seconds = started.elapsed().as_secs_f64();
    let price = ctx.config.price(tier);
    let report = StepReport {
        label: label.to_string(),
        task: Some(task.task),
        tier: Some(tier),
        model: Some(model),
        input_tokens: usage.total_input(),
        output_tokens: usage.output_tokens,
        estimated_cost_usd: price.cost_usd(usage.total_input(), usage.output_tokens),
        seconds,
        notes,
    };
    progress(ProgressEvent::StepFinished { report: report.clone() });
    Ok(TaskOutcome {
        text,
        report,
        stop_reason,
    })
}

/// Report line for a deterministic (model-free) step.
pub fn deterministic_report(label: &str, seconds: f64, notes: Vec<String>) -> StepReport {
    StepReport {
        label: label.to_string(),
        task: None,
        tier: None,
        model: None,
        input_tokens: 0,
        output_tokens: 0,
        estimated_cost_usd: 0.0,
        seconds,
        notes,
    }
}

/// Pull a MusicXML document out of a model reply: a fenced ```xml block if present, else from
/// the first `<?xml` / `<score-partwise` to the closing `</score-partwise>`.
pub fn extract_musicxml(reply: &str) -> Option<String> {
    let body = extract_fenced(reply, &["xml", "musicxml"]).unwrap_or_else(|| reply.to_string());
    let start = body.find("<?xml").or_else(|| body.find("<score-partwise"))?;
    let end_tag = "</score-partwise>";
    let end = body.rfind(end_tag)? + end_tag.len();
    if end <= start {
        return None;
    }
    Some(body[start..end].trim().to_string())
}

/// Text after a `---NOTES---` marker (used by agents that return XML plus commentary).
pub fn extract_notes(reply: &str) -> Option<String> {
    let idx = reply.rfind("---NOTES---")?;
    let notes = reply[idx + "---NOTES---".len()..].trim();
    if notes.is_empty() {
        None
    } else {
        Some(notes.to_string())
    }
}

/// First fenced code block whose language tag is in `langs` (or any fence if `langs` is empty).
pub fn extract_fenced(reply: &str, langs: &[&str]) -> Option<String> {
    let mut rest = reply;
    while let Some(open) = rest.find("```") {
        let after = &rest[open + 3..];
        let nl = after.find('\n')?;
        let lang = after[..nl].trim().to_ascii_lowercase();
        let body = &after[nl + 1..];
        let close = body.find("```")?;
        if langs.is_empty() || langs.iter().any(|l| *l == lang) {
            return Some(body[..close].trim().to_string());
        }
        rest = &body[close + 3..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_xml() {
        let reply = "Here you go:\n```xml\n<?xml version=\"1.0\"?>\n<score-partwise></score-partwise>\n```\n---NOTES---\nMoved it up.";
        assert_eq!(
            extract_musicxml(reply).unwrap(),
            "<?xml version=\"1.0\"?>\n<score-partwise></score-partwise>"
        );
        assert_eq!(extract_notes(reply).unwrap(), "Moved it up.");
    }

    #[test]
    fn extracts_bare_xml() {
        let reply = "<score-partwise version=\"4.0\"><part-list/></score-partwise>\ntrailing";
        assert_eq!(
            extract_musicxml(reply).unwrap(),
            "<score-partwise version=\"4.0\"><part-list/></score-partwise>"
        );
        assert!(extract_musicxml("nothing here").is_none());
    }

    #[test]
    fn fenced_picks_matching_language() {
        let reply = "```json\n{}\n```\n```lilypond\n\\relative c' { c }\n```";
        assert_eq!(
            extract_fenced(reply, &["lilypond", "ly"]).unwrap(),
            "\\relative c' { c }"
        );
        assert_eq!(extract_fenced(reply, &[]).unwrap(), "{}");
    }
}

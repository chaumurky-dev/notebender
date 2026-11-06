//! Add tablature for a fretted instrument.
//!
//! The TAB part is built deterministically (see [`crate::tab`]): the notation part is copied,
//! and every note gets a string/fret computed from its pitch. The only judgement call, which
//! string to put each note on, is made in small chunks of measures by a cheap model, run
//! concurrently; each answer is physically checked, the engraver's own string numbers and
//! position marks always win, and the deterministic solver fills whatever is left. Without an
//! API key (or in `--dry-run`) the solver decides everything.

use super::{call_on_tier, TaskCall};
use crate::claude::ContentBlock;
use crate::models::Tier;
use crate::pipeline::{Context, ProgressSink, StepReport};
use crate::router::TaskKind;
use crate::tab::instrument::pitch_name;
use crate::tab::notes::{self, PartNotes};
use crate::tab::{build, solver, Assignment, Instrument, NoteEvent};
use anyhow::{anyhow, bail, Context as _, Result};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

/// Measures per model chunk, and measures of context shown on either side.
const CHUNK_MEASURES: usize = 8;
const CONTEXT_MEASURES: usize = 1;

const SYSTEM: &str = r#"You are an expert guitarist and tablature editor. You are given the notes of a short passage for a fretted instrument, and you choose which STRING each note is played on. Frets are computed for you afterwards from the string and the pitch, so only the string matters.

Choose like a competent player: stay in one hand position across a phrase, prefer the open position and open strings in open keys, move up the neck only when the music (or a position mark) calls for it, keep the notes of a chord on adjacent strings with a reachable fret span, let a melody sit on the top strings and the bass on the bottom ones, and keep a tied or repeated note on the same string. Marks such as "position VII" or "barre at fret 5" come from the printed edition and must be followed. A note that already has a string in the source keeps it.

Each line of the passage is one event: measure, beat offset (in divisions), pitch names (sounding), and the note id in parentheses. Simultaneous notes are shown on one line as a chord, lowest first. Lines in the CONTEXT sections are for continuity only; do not answer for them. Answer with JSON only: {"assignments":[{"id":<note id>,"string":<1..N>}]} covering every note id of the PASSAGE section exactly once. String 1 is the highest-pitched string."#;

/// Result of the tablature step.
pub struct TabResult {
    pub musicxml: String,
    pub notes: Option<String>,
    pub reports: Vec<StepReport>,
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
struct ChunkAnswer {
    assignments: Vec<ChunkAssignment>,
}

#[derive(Deserialize)]
struct ChunkAssignment {
    id: usize,
    string: u8,
}

fn answer_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "assignments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "string": { "type": "integer" }
                    },
                    "required": ["id", "string"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["assignments"],
        "additionalProperties": false
    })
}

/// One chunk: the events to decide and the surrounding context.
struct Chunk {
    first_measure: usize,
    last_measure: usize,
    text: String,
    ids: Vec<usize>,
}

fn event_line(events: &[NoteEvent], group: &[usize], marks: &BTreeMap<(usize, i64), u8>) -> String {
    let e0 = &events[group[0]];
    let mut line = format!("m{} @{}", e0.measure, e0.onset);
    if let Some(f) = marks.get(&(e0.measure_idx, e0.onset)) {
        line.push_str(&format!(" [position mark: fret {f}]"));
    }
    if e0.grace {
        line.push_str(" grace");
    }
    if group.len() > 1 {
        line.push_str(" chord");
    }
    let mut notes: Vec<&NoteEvent> = group.iter().map(|&i| &events[i]).collect();
    notes.sort_by_key(|e| e.midi);
    for e in notes {
        line.push_str(&format!(" {}({})", pitch_name(e.midi), e.idx));
        if let Some(s) = e.string_hint {
            line.push_str(&format!("[string {s}]"));
        }
        if let Some(f) = &e.fingering {
            line.push_str(&format!("[finger {f}]"));
        }
        if e.tie_stop {
            line.push_str("[tied]");
        }
    }
    line
}

fn chunks(part: &PartNotes) -> Vec<Chunk> {
    let events = &part.events;
    let groups = crate::tab::simultaneous_groups(events);
    let marks: BTreeMap<(usize, i64), u8> = part.marks.iter().map(|m| ((m.measure_idx, m.onset), m.fret)).collect();
    let n_measures = part.measure_numbers.len();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < n_measures {
        let end = (start + CHUNK_MEASURES).min(n_measures); // exclusive
        let ctx_lo = start.saturating_sub(CONTEXT_MEASURES);
        let ctx_hi = (end + CONTEXT_MEASURES).min(n_measures);
        let mut text = String::new();
        let mut ids = Vec::new();
        let section = |lo: usize, hi: usize, label: &str, text: &mut String| {
            let lines: Vec<String> = groups
                .iter()
                .filter(|g| (lo..hi).contains(&events[g[0]].measure_idx))
                .map(|g| event_line(events, g, &marks))
                .collect();
            if !lines.is_empty() {
                text.push_str(label);
                text.push('\n');
                text.push_str(&lines.join("\n"));
                text.push('\n');
            }
        };
        section(ctx_lo, start, "CONTEXT (before):", &mut text);
        section(start, end, "PASSAGE:", &mut text);
        section(end, ctx_hi, "CONTEXT (after):", &mut text);
        for e in events.iter().filter(|e| (start..end).contains(&e.measure_idx)) {
            ids.push(e.idx);
        }
        if !ids.is_empty() {
            out.push(Chunk {
                first_measure: start,
                last_measure: end - 1,
                text,
                ids,
            });
        }
        start = end;
    }
    out
}

fn parse_answer(text: &str, chunk: &Chunk) -> Result<Vec<Assignment>> {
    let start = text.find('{').ok_or_else(|| anyhow!("no JSON object in reply"))?;
    let end = text.rfind('}').ok_or_else(|| anyhow!("no JSON object in reply"))?;
    let a: ChunkAnswer = serde_json::from_str(&text[start..=end]).context("reply is not the expected JSON")?;
    let wanted: HashMap<usize, ()> = chunk.ids.iter().map(|i| (*i, ())).collect();
    Ok(a.assignments
        .into_iter()
        .filter(|x| wanted.contains_key(&x.id))
        .map(|x| Assignment {
            idx: x.id,
            string: x.string,
            fret: 0,
        })
        .collect())
}

/// Ask the model for one chunk, escalating one tier when the answer is unusable.
async fn decide_chunk(
    ctx: &Context,
    inst: &Instrument,
    capo: u8,
    events: &[NoteEvent],
    chunk: &Chunk,
    progress: &ProgressSink,
) -> (Vec<Assignment>, Vec<StepReport>, Vec<String>) {
    let label = format!(
        "tablature m{}–{}",
        events
            .iter()
            .find(|e| e.measure_idx == chunk.first_measure)
            .map(|e| e.measure.as_str())
            .unwrap_or("?"),
        events
            .iter()
            .rev()
            .find(|e| e.measure_idx == chunk.last_measure)
            .map(|e| e.measure.as_str())
            .unwrap_or("?")
    );
    let system = format!(
        "{SYSTEM}\n\nInstrument: {} ({} strings, string 1 = {}, string {} = {}); maximum fret {}.",
        inst.describe(capo),
        inst.strings(),
        inst.open_midi(1).map(pitch_name).unwrap_or_default(),
        inst.strings(),
        inst.open_midi(inst.strings()).map(pitch_name).unwrap_or_default(),
        inst.max_fret
    );
    let base_tier = ctx.router.route(TaskKind::Tablature).tier;
    let mut reports = Vec::new();
    let mut warnings = Vec::new();
    let mut tier = base_tier;
    for attempt in 0..2 {
        let task = TaskCall {
            task: TaskKind::Tablature,
            system: system.clone(),
            content: vec![ContentBlock::text(chunk.text.clone())],
            schema: Some(answer_schema()),
            max_tokens: Some((chunk.ids.len() as u32 * 16 + 256).min(16_000)),
        };
        let outcome = match call_on_tier(ctx, task, tier, &label, progress).await {
            Ok(o) => o,
            Err(e) => {
                warnings.push(format!(
                    "{label}: model call failed ({e:#}); using the deterministic fingering"
                ));
                return (Vec::new(), reports, warnings);
            }
        };
        reports.push(outcome.report);
        let proposed = match parse_answer(&outcome.text, chunk) {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!("{label}: {e:#}"));
                Vec::new()
            }
        };
        let (checked, bad) = solver::check_assignments(events, inst, capo, &proposed);
        let usable = !chunk.ids.is_empty() && checked.len() * 4 >= chunk.ids.len() * 3;
        if usable || attempt == 1 {
            if !bad.is_empty() {
                warnings.push(format!(
                    "{label}: {} string choice(s) were not playable and were recomputed",
                    bad.len()
                ));
            }
            if !usable {
                warnings.push(format!(
                    "{label}: model answer unusable; using the deterministic fingering"
                ));
            }
            return (checked, reports, warnings);
        }
        let next = match tier {
            Tier::Haiku => Tier::Sonnet,
            Tier::Sonnet => Tier::Opus,
            other => other,
        };
        if next == tier {
            return (checked, reports, warnings);
        }
        warnings.push(format!(
            "{label}: only {}/{} usable string choices from {}; retrying on {}",
            checked.len(),
            chunk.ids.len(),
            tier,
            next
        ));
        tier = next;
    }
    (Vec::new(), reports, warnings)
}

/// Summarise fret usage, and flag tablature that lives unusually high on the neck: most
/// tablature stays in single digits, so a high share of double-digit frets usually means the
/// transcription's octave is wrong (guitar is written an octave above sounding pitch).
fn fret_profile(assignments: &[Assignment]) -> (String, Option<String>) {
    let n = assignments.len();
    if n == 0 {
        return ("no fretted notes".into(), None);
    }
    let pct = |pred: &dyn Fn(u8) -> bool| assignments.iter().filter(|a| pred(a.fret)).count() * 100 / n;
    let open = pct(&|f| f == 0);
    let low = pct(&|f| (1..=4).contains(&f));
    let mid = pct(&|f| (5..=9).contains(&f));
    let high = pct(&|f| (10..=12).contains(&f));
    let very_high = pct(&|f| f >= 13);
    let summary = format!("frets: open {open}%, 1–4 {low}%, 5–9 {mid}%, 10–12 {high}%, 13+ {very_high}%");
    let warning = if high + very_high > 20 || very_high > 5 {
        Some(format!(
            "{}% of the tablature is above fret 9 ({very_high}% above fret 12); most tablature stays in single digits, so check the transcription's octave (guitar is written an octave above sounding pitch), the tuning, and the fingering",
            high + very_high
        ))
    } else {
        None
    };
    (summary, warning)
}

/// Add a TAB part for the `instrument` part of `score`.
pub async fn add_tablature(
    ctx: &Context,
    score: &str,
    instrument: &str,
    tuning: Option<&str>,
    capo: Option<u8>,
    use_model: bool,
    progress: &ProgressSink,
) -> Result<TabResult> {
    let started = Instant::now();
    let mut inst = Instrument::resolve(instrument, tuning)?;
    let capo = capo.unwrap_or(0);
    let Some((part_id, part_name)) = notes::find_part(score, instrument)? else {
        let names: Vec<String> = crate::musicxml::summarize(score)
            .map(|s| s.parts.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        bail!(
            "no {instrument} part to tab (parts: {}); arrange for {instrument} first, e.g. `arrange --for {instrument}`",
            names.join(", ")
        );
    };
    let mut part = notes::extract(score, &part_id, inst.octave_change)?;
    // A capo score is written as played (relative to the capo): the true sounding pitch is
    // a capo's worth higher, which is what the fret computation expects.
    for e in &mut part.events {
        e.midi += capo as i32;
    }
    if part.events.is_empty() {
        bail!("part {part_name:?} has no pitched notes");
    }
    let mut warnings = Vec::new();
    let mut reports = Vec::new();
    let mut notes_out = Vec::new();

    // A guitar part that reaches exactly D2 (one step below the low E) is written for drop D.
    let lowest = part.events.iter().map(|e| e.midi).min().unwrap_or(i32::MAX);
    if tuning.is_none() && inst.name == "guitar" && capo == 0 && lowest == 38 {
        inst = Instrument::resolve("guitar", Some("drop D"))?;
        notes_out.push(
            "the part goes down to D2, so drop D tuning was assumed (pass --tuning standard to override)".to_string(),
        );
    }

    // Engraver's string numbers are hard constraints (checked for playability by the solver).
    let mut fixed: Vec<Assignment> = part
        .events
        .iter()
        .filter_map(|e| {
            e.string_hint.map(|s| Assignment {
                idx: e.idx,
                string: s,
                fret: 0,
            })
        })
        .collect();
    let hinted = fixed.len();

    let mut decided_by_model = 0usize;
    if use_model && ctx.client.is_none() {
        warnings.push("no API key configured: strings chosen by the deterministic solver only".to_string());
    }
    if use_model && ctx.client.is_some() {
        let chunks = chunks(&part);
        let sem = Arc::new(Semaphore::new(ctx.config.limits.omr_concurrency.max(1)));
        let mut handles = Vec::new();
        for chunk in chunks {
            let permit = sem.clone().acquire_owned().await.context("semaphore closed")?;
            let ctx = ctx.clone();
            let inst = inst.clone();
            let events = part.events.clone();
            let progress = progress.clone();
            handles.push(tokio::spawn(async move {
                let r = decide_chunk(&ctx, &inst, capo, &events, &chunk, &progress).await;
                drop(permit);
                r
            }));
        }
        let hinted_ids: HashMap<usize, ()> = fixed.iter().map(|a| (a.idx, ())).collect();
        for h in handles {
            let (assignments, reps, warns) = h.await.context("tablature chunk task panicked")?;
            reports.extend(reps);
            warnings.extend(warns);
            for a in assignments {
                if !hinted_ids.contains_key(&a.idx) {
                    fixed.push(a);
                    decided_by_model += 1;
                }
            }
        }
    }

    let (assignments, solver_warnings) = solver::solve(&part.events, &inst, capo, &part.marks, &fixed);
    warnings.extend(solver_warnings);
    let total = part.events.len();
    notes_out.push(format!(
        "{} for part {part_name:?}: {total} notes; {hinted} string(s) from the engraver, {} position mark(s), {decided_by_model} string choice(s) by the model, {} by the deterministic solver",
        inst.describe(capo),
        part.marks.len(),
        total.saturating_sub(hinted + decided_by_model)
    ));
    let (profile, high_warning) = fret_profile(&assignments);
    notes_out.push(profile);
    warnings.extend(high_warning);
    if !part.has_transpose && inst.octave_change != 0 {
        notes_out.push(format!(
            "the {part_name} part had no <transpose>; assumed it is written an octave above sounding pitch, as is usual for {}",
            inst.name
        ));
    }
    let tab_name = format!("{part_name} TAB");
    let musicxml = build::add_tab_part(score, &part_id, &tab_name, &inst, capo, &assignments)?;
    crate::musicxml::validate(&musicxml).map_err(|e| anyhow!("tablature produced an invalid score: {e}"))?;
    reports.push(super::deterministic_report(
        "tablature (build TAB part)",
        started.elapsed().as_secs_f64(),
        notes_out.clone(),
    ));
    Ok(TabResult {
        musicxml,
        notes: Some(notes_out.join("\n")),
        reports,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::noop_sink;
    use crate::tab::notes;

    const FIXTURE: &str = include_str!("../../tests/fixtures/guitar.musicxml");

    #[test]
    fn chunks_cover_every_note_once() {
        let part = notes::extract(FIXTURE, "P1", -1).unwrap();
        let cs = chunks(&part);
        assert_eq!(cs.len(), 1);
        let ids: Vec<usize> = cs.iter().flat_map(|c| c.ids.clone()).collect();
        let mut expected: Vec<usize> = part.events.iter().map(|e| e.idx).collect();
        expected.sort();
        let mut got = ids.clone();
        got.sort();
        assert_eq!(got, expected);
        let text = &cs[0].text;
        assert!(text.contains("PASSAGE:"));
        assert!(text.contains("[position mark: fret 2]"), "{text}");
        assert!(text.contains("[string 1][finger 4]"), "{text}");
        assert!(text.contains("chord A2(0)") || text.contains("chord"), "{text}");
    }

    #[test]
    fn parses_answers_and_ignores_foreign_ids() {
        let part = notes::extract(FIXTURE, "P1", -1).unwrap();
        let chunk = &chunks(&part)[0];
        let got = parse_answer(r#"{"assignments":[{"id":0,"string":5},{"id":999,"string":1}]}"#, chunk).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].idx, got[0].string), (0, 5));
        assert!(parse_answer("nope", chunk).is_err());
    }

    #[test]
    fn fret_profile_flags_tablature_high_on_the_neck() {
        let mk = |frets: &[u8]| -> Vec<Assignment> {
            frets
                .iter()
                .enumerate()
                .map(|(i, f)| Assignment {
                    idx: i,
                    string: 1,
                    fret: *f,
                })
                .collect()
        };
        let (summary, warn) = fret_profile(&mk(&[0, 0, 2, 2, 4, 7, 9, 5, 3, 12]));
        assert_eq!(summary, "frets: open 20%, 1–4 40%, 5–9 30%, 10–12 10%, 13+ 0%");
        assert!(warn.is_none());
        let (_, warn) = fret_profile(&mk(&[12, 14, 14, 16, 0, 2, 2, 2, 2, 2]));
        assert!(warn.unwrap().contains("40% of the tablature is above fret 9"));
        let (_, warn) = fret_profile(&mk(&[0; 10]));
        assert!(warn.is_none());
        assert!(fret_profile(&[]).1.is_none());
    }

    #[tokio::test]
    async fn deterministic_tablature_without_a_model() {
        let config = crate::config::Config::default();
        let ctx = Context {
            router: Arc::new(crate::router::Router::from_config(&config)),
            tools: Arc::new(crate::tools::Toolbox::detect(&config.tools)),
            config: Arc::new(config),
            client: None,
        };
        let r = add_tablature(&ctx, FIXTURE, "guitar", None, None, false, &noop_sink())
            .await
            .unwrap();
        let s = crate::musicxml::summarize(&r.musicxml).unwrap();
        assert!(s.has_tablature);
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[0].measures, s.parts[1].measures);
        // the engraver's string 1 for A4 is kept: string 1 fret 5
        assert!(
            r.musicxml.contains("<string>1</string>\n<fret>5</fret>"),
            "{}",
            r.musicxml
        );
        // open-position A major chord: low A on string 5 open
        assert!(r.musicxml.contains("<string>5</string>\n<fret>0</fret>"));
        assert!(r.notes.unwrap().contains("by the deterministic solver"));
    }
}

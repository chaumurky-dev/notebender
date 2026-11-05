//! Execution of a [`JobRequest`]: input → (OMR) → plan → steps → deliverables → report.

use crate::agents;
use crate::musicxml::{self, Interval, Key, Summary};
use crate::pdf::{self, InputKind};
use crate::pipeline::{
    Artifact, Context, Format, JobRequest, JobResult, Plan, ProgressEvent, ProgressSink, Step, StepReport,
};
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Run<'a> {
    ctx: &'a Context,
    req: &'a JobRequest,
    progress: ProgressSink,
    result: JobResult,
    /// Per-step commentary from the models, for the report and the reviewer.
    notes: Vec<(String, String)>,
    workdir: PathBuf,
}

impl Run<'_> {
    fn warn(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        (self.progress)(ProgressEvent::Warning { message: msg.clone() });
        self.result.warnings.push(msg);
    }

    fn log(&self, msg: impl Into<String>) {
        (self.progress)(ProgressEvent::Log { message: msg.into() });
    }

    fn record(&mut self, report: StepReport) {
        self.result.total_cost_usd += report.estimated_cost_usd;
        self.result.steps.push(report);
    }

    fn deterministic(&mut self, label: &str, started: Instant, notes: Vec<String>) {
        let report = agents::deterministic_report(label, started.elapsed().as_secs_f64(), notes);
        (self.progress)(ProgressEvent::StepFinished { report: report.clone() });
        self.record(report);
    }

    fn start_deterministic(&self, label: &str) -> Instant {
        (self.progress)(ProgressEvent::StepStarted {
            label: label.into(),
            task: None,
            tier: None,
            model: None,
        });
        Instant::now()
    }

    fn artifact(&mut self, name: &str, bytes: &[u8], format: Option<Format>, description: &str) -> Result<PathBuf> {
        let path = self.req.out_dir.join(name);
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        let artifact = Artifact {
            name: name.into(),
            path: path.clone(),
            format,
            description: description.into(),
        };
        (self.progress)(ProgressEvent::Artifact {
            artifact: artifact.clone(),
        });
        self.result.artifacts.push(artifact);
        Ok(path)
    }
}

pub async fn run_job(ctx: &Context, req: &JobRequest, progress: ProgressSink) -> Result<JobResult> {
    std::fs::create_dir_all(&req.out_dir).with_context(|| format!("creating {}", req.out_dir.display()))?;
    let workdir = req.out_dir.join(".work");
    std::fs::create_dir_all(&workdir)?;
    let mut run = Run {
        ctx,
        req,
        progress,
        result: JobResult::default(),
        notes: Vec::new(),
        workdir,
    };

    // 1. Input → MusicXML.
    let kind = pdf::classify(&req.input)?;
    let mut score = match kind {
        InputKind::MusicXml => {
            let xml = std::fs::read_to_string(&req.input)?;
            musicxml::validate(&xml).context("input MusicXML is not valid")?;
            run.log(format!("input is MusicXML ({} bytes); skipping OMR", xml.len()));
            xml
        }
        InputKind::Pdf => transcribe(&mut run).await?,
    };
    let original_summary = musicxml::summarize(&score)?;
    run.log(format!("score: {}", original_summary.describe()));

    // 2. Plan.
    let mut plan = match &req.plan {
        Some(p) => {
            let mut p = p.clone();
            agents::planner::normalize(&mut p);
            p
        }
        None if req.dry_run => anyhow::bail!(
            "--dry-run makes no API calls, so the planner cannot run; use a subcommand that defines the plan (transpose, tab, arrange, edit, convert, omr) instead of `run`"
        ),
        None => {
            let (plan, report) =
                agents::planner::plan(ctx, &req.instructions, Some(&original_summary), &run.progress).await?;
            run.record(report);
            plan
        }
    };
    if plan.summary.is_empty() {
        plan.summary = describe_plan(&plan);
    }
    for u in &plan.unsupported {
        run.warn(format!("not supported, skipped: {u}"));
    }
    plan.layout.validate().map_err(|e| anyhow::anyhow!("layout: {e}"))?;
    (run.progress)(ProgressEvent::Planned { plan: plan.clone() });
    run.result.plan = plan.clone();

    if req.dry_run {
        run.log("dry run: model-backed steps will be described, not executed");
    }

    // 3. Steps.
    for step in &plan.steps {
        score = apply_step(&mut run, step, score).await?;
    }

    // 4. Deliverables.
    let final_path = run.artifact(
        "score.musicxml",
        score.as_bytes(),
        Some(Format::MusicXml),
        "final score (MusicXML)",
    )?;
    let final_summary = musicxml::summarize(&score)?;
    deliver(&mut run, &plan, &score, &final_path).await?;

    // 5. Optional review.
    if req.review && !req.dry_run {
        match agents::review::review(
            ctx,
            &req.instructions,
            &plan,
            &original_summary,
            &final_summary,
            &run.notes,
            &run.progress,
        )
        .await
        {
            Ok((review, report)) => {
                run.record(report);
                let mut text = String::new();
                for f in &review.findings {
                    text.push_str(&format!("- {f}\n"));
                }
                if !review.ok {
                    run.warn("reviewer flagged the result; see the review notes");
                }
                run.notes.push((
                    "review".into(),
                    if text.is_empty() { "No findings.".into() } else { text },
                ));
            }
            Err(e) => run.warn(format!("review step failed: {e:#}")),
        }
    }

    // 6. Report.
    run.result.summary = report_markdown(&run, &original_summary, &final_summary);
    let summary = run.result.summary.clone();
    run.artifact(
        "report.md",
        summary.as_bytes(),
        None,
        "what was done, by which model, at what cost",
    )?;
    Ok(run.result)
}

async fn transcribe(run: &mut Run<'_>) -> Result<String> {
    let ctx = run.ctx;
    let pages = pdf::page_count(&ctx.tools, &run.req.input).await?;
    if pages > ctx.config.limits.max_pdf_pages {
        anyhow::bail!(
            "PDF has {pages} pages, more than limits.max_pdf_pages = {}",
            ctx.config.limits.max_pdf_pages
        );
    }
    if run.req.dry_run {
        anyhow::bail!("dry run cannot transcribe a PDF; give a MusicXML input or run without --dry-run");
    }
    let started = run.start_deterministic("prepare pages");
    let images = pdf::prepare(&ctx.tools, &run.req.input, &run.workdir, ctx.config.limits.page_dpi).await?;
    let note = match &images {
        pdf::ScoreImages::Pages(p) => format!("{} page image(s) at {} dpi", p.len(), ctx.config.limits.page_dpi),
        pdf::ScoreImages::WholePdf(_) => "pdftoppm not installed: sending the whole PDF in one request".into(),
    };
    run.deterministic("prepare pages", started, vec![note]);

    let omr = agents::omr::transcribe(ctx, &images, &run.progress).await?;
    for r in omr.reports {
        run.record(r);
    }
    for w in omr.warnings {
        run.warn(w);
    }
    run.artifact(
        "transcription.musicxml",
        omr.musicxml.as_bytes(),
        Some(Format::MusicXml),
        "OMR transcription of the input, before any transformation",
    )?;
    Ok(omr.musicxml)
}

async fn apply_step(run: &mut Run<'_>, step: &Step, score: String) -> Result<String> {
    let ctx = run.ctx;
    match step {
        Step::Transpose {
            semitones,
            from_key,
            to_key,
        } => {
            let started = run.start_deterministic("transpose");
            let detected = musicxml::detect_key(&score);
            let interval = resolve_interval(*semitones, from_key.as_deref(), to_key.as_deref(), detected)?;
            let out = musicxml::transpose(&score, &interval)?;
            let mut notes = vec![interval.describe()];
            if let (Some(d), None) = (detected, from_key) {
                notes.push(format!("source key detected as {}", d.name()));
            }
            if let Some(k) = musicxml::detect_key(&out) {
                notes.push(format!("new key signature: {}", k.name()));
            }
            run.deterministic("transpose", started, notes);
            Ok(out)
        }
        Step::Tablature {
            instrument,
            tuning,
            capo,
        } => {
            if run.req.dry_run {
                run.log(format!(
                    "would add {instrument} tablature with {}",
                    ctx.config
                        .model_id(ctx.router.route(crate::router::TaskKind::Tablature).tier)
                ));
                return Ok(score);
            }
            let r = agents::tablature::add_tablature(ctx, &score, instrument, tuning.as_deref(), *capo, &run.progress)
                .await?;
            finish_xml_step(run, "tablature", r)
        }
        Step::Arrange { instruments, notes } => {
            if run.req.dry_run {
                run.log(format!(
                    "would arrange for {} with {}",
                    instruments.join(", "),
                    ctx.config
                        .model_id(ctx.router.route(crate::router::TaskKind::Arrange).tier)
                ));
                return Ok(score);
            }
            let r = agents::arrange::arrange(ctx, &score, instruments, notes.as_deref(), &run.progress).await?;
            finish_xml_step(run, "arrange", r)
        }
        Step::Edit { instruction } => {
            if run.req.dry_run {
                run.log(format!(
                    "would apply edit '{instruction}' with {}",
                    ctx.config
                        .model_id(ctx.router.route(crate::router::TaskKind::Edit).tier)
                ));
                return Ok(score);
            }
            let r = agents::edit::edit(ctx, &score, instruction, &run.progress).await?;
            finish_xml_step(run, "edit", r)
        }
    }
}

fn finish_xml_step(run: &mut Run<'_>, label: &str, r: agents::xml_task::XmlTaskResult) -> Result<String> {
    run.record(r.report);
    for w in r.warnings {
        run.warn(w);
    }
    if let Some(n) = r.notes {
        run.notes.push((label.to_string(), n));
    }
    Ok(r.musicxml)
}

/// Work out the transposition interval from whatever the plan specified.
pub fn resolve_interval(
    semitones: Option<i32>,
    from_key: Option<&str>,
    to_key: Option<&str>,
    detected: Option<Key>,
) -> Result<Interval> {
    match (semitones, to_key) {
        (_, Some(to)) => {
            let to = Key::parse(to).map_err(|e| anyhow::anyhow!("target key '{to}': {e}"))?;
            let from = match from_key {
                Some(f) => Key::parse(f).map_err(|e| anyhow::anyhow!("source key '{f}': {e}"))?,
                None => detected
                    .context("the score has no key signature; please say which key it is in (e.g. 'from G to C')")?,
            };
            Ok(Interval::between_keys(&from, &to))
        }
        (Some(n), None) => Ok(Interval::from_semitones(n, detected.as_ref())),
        (None, None) => anyhow::bail!("transpose step needs semitones or a target key"),
    }
}

async fn deliver(run: &mut Run<'_>, plan: &Plan, score: &str, score_path: &Path) -> Result<()> {
    let ctx = run.ctx;
    let wants_pdf = plan.deliverables.contains(&Format::Pdf);
    let mut ly_path: Option<PathBuf> = None;

    for format in &plan.deliverables {
        match format {
            Format::MusicXml => {}
            Format::Pdf => {}
            Format::LilyPond | Format::Humdrum | Format::Abc => {
                let model_needed = !(*format == Format::LilyPond && ctx.tools.musicxml2ly.available());
                if run.req.dry_run && model_needed {
                    run.log(format!(
                        "would convert to {} with the {} model",
                        format.label(),
                        ctx.router.route(crate::router::TaskKind::Convert).tier
                    ));
                    continue;
                }
                match agents::convert::convert(ctx, score, score_path, *format, &run.workdir, &run.progress).await {
                    Ok(r) => {
                        run.record(r.report);
                        let text = if *format == Format::LilyPond {
                            apply_layout(run, &plan.layout, r.text)
                        } else {
                            r.text
                        };
                        let name = format!("score.{}", format.extension());
                        let p = run.artifact(
                            &name,
                            text.as_bytes(),
                            Some(*format),
                            &format!("{} export", format.label()),
                        )?;
                        if *format == Format::LilyPond {
                            ly_path = Some(p);
                        }
                    }
                    Err(e) => run.warn(format!("{} export failed: {e:#}", format.label())),
                }
            }
        }
    }

    if wants_pdf {
        if !ctx.tools.lilypond.available() {
            run.warn("PDF requested but LilyPond is not installed (brew install lilypond); skipping engraving");
            return Ok(());
        }
        // Need LilyPond source first.
        if ly_path.is_none() {
            if run.req.dry_run && !ctx.tools.musicxml2ly.available() {
                run.log("would convert to LilyPond with a model, then engrave");
                return Ok(());
            }
            match agents::convert::convert(ctx, score, score_path, Format::LilyPond, &run.workdir, &run.progress).await
            {
                Ok(r) => {
                    run.record(r.report);
                    let text = apply_layout(run, &plan.layout, r.text);
                    let p = run.workdir.join("score.ly");
                    std::fs::write(&p, text)?;
                    ly_path = Some(p);
                }
                Err(e) => {
                    run.warn(format!("could not produce LilyPond source for the PDF: {e:#}"));
                    return Ok(());
                }
            }
        }
        let ly = ly_path.expect("set above");
        let started = run.start_deterministic("engrave PDF (lilypond)");
        match ctx.tools.lilypond_to_pdf(&ly, &run.workdir, "score").await {
            Ok(pdf) => {
                let bytes = std::fs::read(&pdf)?;
                run.artifact("score.pdf", &bytes, Some(Format::Pdf), "engraved score (LilyPond)")?;
                run.deterministic("engrave PDF (lilypond)", started, vec![]);
            }
            Err(e) => run.warn(format!("LilyPond engraving failed: {e:#}")),
        }
    }
    Ok(())
}

/// Inject layout options into LilyPond source, recording a deterministic step when they are set.
fn apply_layout(run: &mut Run<'_>, layout: &crate::layout::LayoutOptions, ly: String) -> String {
    if layout.is_default() {
        return ly;
    }
    let started = run.start_deterministic("layout");
    let out = crate::layout::apply_to_lilypond(&ly, layout);
    run.deterministic("layout", started, vec![layout.describe()]);
    out
}

pub fn describe_plan(plan: &Plan) -> String {
    let mut parts: Vec<String> = plan
        .steps
        .iter()
        .map(|s| match s {
            Step::Transpose { semitones: Some(n), .. } => format!("transpose by {n:+} semitone(s)"),
            Step::Transpose { from_key, to_key, .. } => format!(
                "transpose {}to {}",
                from_key.as_ref().map(|f| format!("from {f} ")).unwrap_or_default(),
                to_key.as_deref().unwrap_or("?")
            ),
            Step::Tablature { instrument, .. } => format!("add {instrument} tablature"),
            Step::Arrange { instruments, .. } => format!("arrange for {}", instruments.join(" and ")),
            Step::Edit { instruction } => format!("edit: {instruction}"),
        })
        .collect();
    if parts.is_empty() {
        parts.push("transcribe".into());
    }
    if !plan.layout.is_default() {
        parts.push(format!("layout: {}", plan.layout.describe()));
    }
    let formats: Vec<&str> = plan.deliverables.iter().map(|f| f.label()).collect();
    format!("{}; deliver {}", parts.join(", "), formats.join(", "))
}

fn report_markdown(run: &Run<'_>, before: &Summary, after: &Summary) -> String {
    let r = &run.result;
    let mut md = String::new();
    md.push_str("# notebender report\n\n");
    md.push_str(&format!("**Request:** {}\n\n", run.req.instructions.trim()));
    md.push_str(&format!(
        "**Plan:** {}\n\n",
        if r.plan.summary.is_empty() {
            describe_plan(&r.plan)
        } else {
            r.plan.summary.clone()
        }
    ));
    md.push_str(&format!(
        "**Original score:** {}\n\n**Final score:** {}\n\n",
        before.describe(),
        after.describe()
    ));
    if !r.artifacts.is_empty() {
        md.push_str("## Artifacts\n\n");
        for a in &r.artifacts {
            md.push_str(&format!("- `{}` — {}\n", a.name, a.description));
        }
        md.push('\n');
    }
    md.push_str("## Steps\n\n| step | model | tokens in | tokens out | est. cost (USD) | seconds |\n|---|---|---:|---:|---:|---:|\n");
    for s in &r.steps {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {:.4} | {:.1} |\n",
            s.label,
            s.model.as_deref().unwrap_or("— (deterministic)"),
            s.input_tokens,
            s.output_tokens,
            s.estimated_cost_usd,
            s.seconds
        ));
    }
    md.push_str(&format!("\n**Total estimated cost:** ${:.4}\n\n", r.total_cost_usd));
    let step_notes: Vec<String> = r
        .steps
        .iter()
        .filter(|s| !s.notes.is_empty())
        .map(|s| format!("- {}: {}", s.label, s.notes.join("; ")))
        .collect();
    if !step_notes.is_empty() {
        md.push_str(&step_notes.join("\n"));
        md.push_str("\n\n");
    }
    if !run.notes.is_empty() {
        md.push_str("## Notes from the models\n\n");
        for (label, n) in &run.notes {
            md.push_str(&format!("### {label}\n\n{n}\n\n"));
        }
    }
    if !r.warnings.is_empty() {
        md.push_str("## Warnings\n\n");
        for w in &r.warnings {
            md.push_str(&format!("- {w}\n"));
        }
    }
    md
}

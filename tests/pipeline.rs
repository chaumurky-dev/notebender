//! Offline pipeline tests: no API key, no external tools required.

use notebender::config::Config;
use notebender::pipeline::{self, Context, Format, JobRequest, Plan, ProgressEvent, Step};
use notebender::router::Router;
use notebender::tools::Toolbox;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn offline_context() -> Context {
    let config = Config::default();
    let tools = Toolbox::detect(&config.tools);
    Context {
        router: Arc::new(Router::from_config(&config)),
        config: Arc::new(config),
        client: None,
        tools: Arc::new(tools),
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/simple.musicxml")
}

#[tokio::test]
async fn deterministic_transpose_runs_without_api_key() {
    let ctx = offline_context();
    let out = tempfile::tempdir().unwrap();
    let plan = Plan {
        summary: String::new(),
        steps: vec![Step::Transpose {
            semitones: None,
            from_key: None,
            to_key: Some("D".into()),
        }],
        deliverables: vec![Format::MusicXml],
        unsupported: vec![],
        layout: Default::default(),
    };
    let req = JobRequest {
        input: fixture(),
        instructions: "Transpose to D.".into(),
        out_dir: out.path().join("job"),
        plan: Some(plan),
        review: false,
        dry_run: false,
    };
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let events = events.clone();
        Arc::new(move |e: ProgressEvent| events.lock().unwrap().push(e))
    };
    let result = pipeline::run_job(&ctx, &req, sink).await.expect("job runs");

    assert_eq!(result.total_cost_usd, 0.0);
    assert!(
        result.steps.iter().all(|s| s.model.is_none()),
        "no model steps expected"
    );
    let score = std::fs::read_to_string(out.path().join("job/score.musicxml")).unwrap();
    assert!(score.contains("<fifths>2</fifths>"), "C major → D major has two sharps");
    assert!(
        score.contains("<step>C</step>\n          <alter>1</alter>"),
        "B4 → C#5:\n{score}"
    );
    assert!(out.path().join("job/report.md").exists());
    assert_eq!(result.plan.summary, "transpose to D; deliver MusicXML");
    let names: Vec<_> = result.artifacts.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, vec!["score.musicxml", "report.md"]);
    let saw_planned = events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, ProgressEvent::Planned { .. }));
    assert!(saw_planned);
}

#[tokio::test]
async fn dry_run_describes_model_steps_instead_of_calling() {
    let ctx = offline_context();
    let out = tempfile::tempdir().unwrap();
    let plan = Plan {
        summary: "tab".into(),
        steps: vec![
            Step::Transpose {
                semitones: Some(1),
                from_key: None,
                to_key: None,
            },
            Step::Tablature {
                instrument: "guitar".into(),
                tuning: None,
                capo: None,
            },
        ],
        deliverables: vec![Format::MusicXml, Format::Humdrum],
        unsupported: vec![],
        layout: Default::default(),
    };
    let req = JobRequest {
        input: fixture(),
        instructions: "x".into(),
        out_dir: out.path().to_path_buf(),
        plan: Some(plan),
        review: true,
        dry_run: true,
    };
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let logs = logs.clone();
        Arc::new(move |e: ProgressEvent| {
            if let ProgressEvent::Log { message } = e {
                logs.lock().unwrap().push(message)
            }
        })
    };
    let result = pipeline::run_job(&ctx, &req, sink).await.expect("dry run succeeds");
    let logs = logs.lock().unwrap();
    assert!(
        logs.iter()
            .any(|l| l.contains("strings chosen by the deterministic solver")),
        "{logs:?}"
    );
    assert!(logs.iter().any(|l| l.contains("would convert to Humdrum")), "{logs:?}");
    // Transposition still ran: C major up a semitone → Db major (5 flats).
    let score = std::fs::read_to_string(out.path().join("score.musicxml")).unwrap();
    assert!(score.contains("<fifths>-5</fifths>"), "{score}");
    assert!(result.steps.iter().any(|s| s.label == "transpose"));
    // Tablature is deterministic, so the dry run produced a real TAB part with frets.
    assert!(score.contains("<sign>TAB</sign>"), "{score}");
    assert!(score.contains("<fret>"), "{score}");
    assert!(result.steps.iter().any(|s| s.label.starts_with("tablature")));
}

#[tokio::test]
async fn model_step_without_key_fails_clearly() {
    let ctx = offline_context();
    let out = tempfile::tempdir().unwrap();
    let plan = Plan {
        summary: "arrange".into(),
        steps: vec![Step::Arrange {
            instruments: vec!["cello".into()],
            notes: None,
        }],
        deliverables: vec![Format::MusicXml],
        unsupported: vec![],
        layout: Default::default(),
    };
    let req = JobRequest {
        input: fixture(),
        instructions: "x".into(),
        out_dir: out.path().to_path_buf(),
        plan: Some(plan),
        review: false,
        dry_run: false,
    };
    let err = pipeline::run_job(&ctx, &req, pipeline::noop_sink()).await.unwrap_err();
    assert!(format!("{err:#}").contains("no API key is configured"), "{err:#}");
}

#[tokio::test]
async fn layout_options_are_injected_into_lilypond() {
    let ctx = offline_context();
    if !ctx.tools.musicxml2ly.available() {
        eprintln!("musicxml2ly not installed; skipping");
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let plan = Plan {
        summary: String::new(),
        steps: vec![],
        deliverables: vec![Format::LilyPond],
        unsupported: vec![],
        layout: notebender::layout::LayoutOptions {
            scale: Some(0.75),
            spacing: Some(0.9),
            ..Default::default()
        },
    };
    let req = JobRequest {
        input: fixture(),
        instructions: "smaller".into(),
        out_dir: out.path().to_path_buf(),
        plan: Some(plan),
        review: false,
        dry_run: true,
    };
    let result = pipeline::run_job(&ctx, &req, pipeline::noop_sink())
        .await
        .expect("job runs");
    let ly = std::fs::read_to_string(out.path().join("score.ly")).unwrap();
    assert!(ly.contains("#(set-global-staff-size 15.00)"), "{ly}");
    assert!(ly.contains("spacing-increment = #1.08"), "{ly}");
    assert!(result.steps.iter().any(|s| s.label == "layout"));
    // Layout implies a PDF deliverable (engraved when lilypond exists; warned otherwise).
    assert!(result.plan.deliverables.contains(&Format::Pdf));
}

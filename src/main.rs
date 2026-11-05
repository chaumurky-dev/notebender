//! `notebender` command-line interface.

use anyhow::{Context as _, Result};
use clap::{Args, Parser, Subcommand};
use notebender::claude::Client;
use notebender::config::Config;
use notebender::models::Tier;
use notebender::musicxml;
use notebender::pipeline::{self, Context, Format, JobRequest, Plan, ProgressEvent, Step};
use notebender::router::Router;
use notebender::tools::Toolbox;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "notebender",
    version,
    about = "Transform PDF sheet music with Claude: OMR, transposition, tablature, arranging, format conversion."
)]
struct Cli {
    /// Config file (default: ./notebender.toml, then the platform config dir).
    #[arg(long, global = true, env = "NOTEBENDER_CONFIG")]
    config: Option<PathBuf>,
    /// Route a task to a tier for this run, e.g. `--model omr=fable` (repeatable).
    /// Tasks: plan, omr, convert, tablature, arrange, edit, review. Tiers: haiku, sonnet, opus, fable.
    #[arg(long = "model", global = true, value_name = "TASK=TIER")]
    model_overrides: Vec<String>,
    /// Verbose logging (-v debug, -vv trace).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run a free-text request against a score (the planner decides the steps).
    Run {
        /// Input PDF (or MusicXML to skip OMR).
        input: PathBuf,
        /// What to do, in plain language.
        #[arg(short, long)]
        prompt: String,
        #[command(flatten)]
        out: OutArgs,
        /// Extra model-backed QA pass at the end.
        #[arg(long)]
        review: bool,
    },
    /// Transcribe a PDF to MusicXML (and optionally other formats) without other changes.
    Omr {
        /// Input PDF (or MusicXML).
        input: PathBuf,
        #[command(flatten)]
        out: OutArgs,
        #[command(flatten)]
        formats: FormatArgs,
    },
    /// Transpose (deterministic; no API calls for MusicXML input).
    Transpose {
        /// Input PDF (or MusicXML to skip OMR).
        input: PathBuf,
        /// Interval in semitones, e.g. 1, -2, 12.
        #[arg(short, long, allow_hyphen_values = true, conflicts_with = "to")]
        semitones: Option<i32>,
        /// Target key, e.g. C, "Bb", "F# minor".
        #[arg(long)]
        to: Option<String>,
        /// Source key (default: detected from the score).
        #[arg(long, requires = "to")]
        from: Option<String>,
        #[command(flatten)]
        out: OutArgs,
        #[command(flatten)]
        formats: FormatArgs,
    },
    /// Add tablature for a fretted instrument.
    Tab {
        /// Input PDF (or MusicXML to skip OMR).
        input: PathBuf,
        /// Fretted instrument: guitar, bass, ukulele, mandolin, banjo, …
        #[arg(long, default_value = "guitar")]
        instrument: String,
        /// Tuning, e.g. "standard", "drop D", "DADGAD".
        #[arg(long)]
        tuning: Option<String>,
        /// Capo fret.
        #[arg(long)]
        capo: Option<u8>,
        #[command(flatten)]
        out: OutArgs,
        #[command(flatten)]
        formats: FormatArgs,
    },
    /// Arrange for different instruments.
    Arrange {
        /// Input PDF (or MusicXML to skip OMR).
        input: PathBuf,
        /// Comma-separated instruments in score order, e.g. "cello,guitar".
        #[arg(long = "for", value_name = "INSTRUMENTS")]
        instruments: String,
        /// Extra guidance for the arranger.
        #[arg(long)]
        notes: Option<String>,
        #[command(flatten)]
        out: OutArgs,
        #[command(flatten)]
        formats: FormatArgs,
    },
    /// Apply a free-form edit described in words (e.g. "simplify the left hand").
    Edit {
        /// Input PDF (or MusicXML to skip OMR).
        input: PathBuf,
        /// The edit to apply, in plain language.
        #[arg(short, long)]
        instruction: String,
        #[command(flatten)]
        out: OutArgs,
        #[command(flatten)]
        formats: FormatArgs,
    },
    /// Convert a score to other notation formats.
    Convert {
        /// Input PDF (or MusicXML to skip OMR).
        input: PathBuf,
        /// Target format: lilypond, pdf, humdrum, abc (repeatable or comma-separated).
        #[arg(long = "to", value_name = "FMT", required = true)]
        formats: Vec<String>,
        #[command(flatten)]
        out: OutArgs,
    },
    /// Start the local web UI.
    Serve {
        /// Bind address (default from config: 127.0.0.1).
        #[arg(long)]
        host: Option<String>,
        /// Port (default from config: 8787).
        #[arg(long)]
        port: Option<u16>,
        /// Directory for uploads and results (default: platform data dir).
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Check configuration, API key and external tools.
    Doctor,
    /// Show the task → model routing table.
    Models,
    /// Manage the config file.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Write a fully commented default config to ./notebender.toml (or --path).
    Init {
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
    /// Print the effective configuration.
    Show,
    /// Print where the config file is (or would be) read from.
    Path,
}

#[derive(Args, Debug)]
struct OutArgs {
    /// Output directory (default: ./out/<input-stem>).
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Never call the API: run deterministic steps (transposition, musicxml2ly, LilyPond) and only
    /// describe model-backed ones. Needs MusicXML input and a subcommand that defines the plan
    /// (not `run`, whose plan comes from the planner model).
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct FormatArgs {
    /// Extra output format besides MusicXML: lilypond, pdf, humdrum, abc (repeatable or comma-separated).
    #[arg(long = "format", value_name = "FMT")]
    formats: Vec<String>,
}

fn parse_formats(specs: &[String]) -> Result<Vec<Format>> {
    let mut v = vec![Format::MusicXml];
    for s in specs {
        for part in s.split(',') {
            let f = Format::parse(part)
                .with_context(|| format!("unknown format '{part}' (musicxml, lilypond, pdf, humdrum, abc)"))?;
            v.push(f);
        }
    }
    v.sort();
    v.dedup();
    Ok(v)
}

fn default_out(input: &Path) -> PathBuf {
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("score");
    PathBuf::from("out").join(stem)
}

struct App {
    config: Arc<Config>,
    config_path: Option<PathBuf>,
    router: Arc<Router>,
    tools: Arc<Toolbox>,
}

impl App {
    fn load(cli: &Cli) -> Result<Self> {
        let (config, config_path) = Config::load(cli.config.as_deref())?;
        let router = Router::from_config(&config)
            .with_override_specs(cli.model_overrides.iter().map(String::as_str))
            .map_err(|e| anyhow::anyhow!(e))?;
        let tools = Toolbox::detect(&config.tools);
        Ok(App {
            config: Arc::new(config),
            config_path,
            router: Arc::new(router),
            tools: Arc::new(tools),
        })
    }

    fn context(&self, need_api: bool) -> Result<Context> {
        let client = match Client::new(&self.config) {
            Ok(c) => Some(Arc::new(c)),
            Err(e) if need_api => return Err(anyhow::anyhow!(e)),
            Err(e) => {
                tracing::debug!("{e}; model-backed steps will fail");
                None
            }
        };
        Ok(Context {
            config: self.config.clone(),
            router: self.router.clone(),
            client,
            tools: self.tools.clone(),
        })
    }
}

fn cli_progress() -> pipeline::ProgressSink {
    Arc::new(|ev: ProgressEvent| match ev {
        ProgressEvent::Planned { plan } => {
            eprintln!("plan: {}", plan.summary);
            for s in &plan.steps {
                eprintln!("  - {}", serde_json::to_string(s).unwrap_or_default());
            }
        }
        ProgressEvent::StepStarted { label, model, .. } => match model {
            Some(m) => eprintln!("▶ {label} [{m}]"),
            None => eprintln!("▶ {label}"),
        },
        ProgressEvent::StepFinished { report } => {
            if report.model.is_some() {
                eprintln!(
                    "✓ {} ({} in / {} out tokens, ≈${:.4}, {:.1}s){}",
                    report.label,
                    report.input_tokens,
                    report.output_tokens,
                    report.estimated_cost_usd,
                    report.seconds,
                    if report.notes.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", report.notes.join("; "))
                    }
                );
            } else {
                eprintln!(
                    "✓ {} ({:.1}s){}",
                    report.label,
                    report.seconds,
                    if report.notes.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", report.notes.join("; "))
                    }
                );
            }
        }
        ProgressEvent::Warning { message } => eprintln!("⚠ {message}"),
        ProgressEvent::Artifact { artifact } => eprintln!("  wrote {}", artifact.path.display()),
        ProgressEvent::Log { message } => eprintln!("  {message}"),
    })
}

async fn run_with_plan(
    app: &App,
    input: PathBuf,
    out: OutArgs,
    plan: Option<Plan>,
    instructions: String,
    review: bool,
) -> Result<()> {
    let out_dir = out.out.unwrap_or_else(|| default_out(&input));
    let ctx = app.context(false)?;
    if !out.dry_run && ctx.client.is_none() {
        // Deterministic-only plans on MusicXML input still work without a key; the pipeline fails
        // with a clear message if a model step is reached.
        eprintln!(
            "note: no API key in ${}; only deterministic steps can run",
            app.config.api.key_env
        );
    }
    let req = JobRequest {
        input,
        instructions,
        out_dir: out_dir.clone(),
        plan,
        review,
        dry_run: out.dry_run,
    };
    let result = pipeline::run_job(&ctx, &req, cli_progress()).await?;
    println!();
    println!("{}", result.summary);
    println!("artifacts in {}", out_dir.display());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let level = match cli.verbose {
        0 => "warn,notebender=info",
        1 => "info,notebender=debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| level.into()))
        .with_writer(std::io::stderr)
        .init();

    let app = App::load(&cli)?;
    match cli.command {
        Command::Run {
            input,
            prompt,
            out,
            review,
        } => run_with_plan(&app, input, out, None, prompt, review).await,
        Command::Omr { input, out, formats } => {
            let plan = Plan {
                summary: "transcribe".into(),
                steps: vec![],
                deliverables: parse_formats(&formats.formats)?,
                unsupported: vec![],
            };
            run_with_plan(&app, input, out, Some(plan), "Transcribe this score.".into(), false).await
        }
        Command::Transpose {
            input,
            semitones,
            to,
            from,
            out,
            formats,
        } => {
            if semitones.is_none() && to.is_none() {
                anyhow::bail!("give --semitones N or --to KEY");
            }
            let step = Step::Transpose {
                semitones,
                from_key: from.clone(),
                to_key: to.clone(),
            };
            let instructions = match (&semitones, &to) {
                (Some(n), _) => format!("Transpose by {n:+} semitones."),
                (_, Some(k)) => format!("Transpose to {k}."),
                _ => unreachable!(),
            };
            let plan = Plan {
                summary: instructions.clone(),
                steps: vec![step],
                deliverables: parse_formats(&formats.formats)?,
                unsupported: vec![],
            };
            run_with_plan(&app, input, out, Some(plan), instructions, false).await
        }
        Command::Tab {
            input,
            instrument,
            tuning,
            capo,
            out,
            formats,
        } => {
            let instructions = format!("Add {instrument} tablature.");
            let plan = Plan {
                summary: instructions.clone(),
                steps: vec![Step::Tablature {
                    instrument,
                    tuning,
                    capo,
                }],
                deliverables: parse_formats(&formats.formats)?,
                unsupported: vec![],
            };
            run_with_plan(&app, input, out, Some(plan), instructions, false).await
        }
        Command::Arrange {
            input,
            instruments,
            notes,
            out,
            formats,
        } => {
            let instruments: Vec<String> = instruments
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if instruments.is_empty() {
                anyhow::bail!("--for needs at least one instrument");
            }
            let instructions = format!("Arrange for {}.", instruments.join(", "));
            let plan = Plan {
                summary: instructions.clone(),
                steps: vec![Step::Arrange { instruments, notes }],
                deliverables: parse_formats(&formats.formats)?,
                unsupported: vec![],
            };
            run_with_plan(&app, input, out, Some(plan), instructions, false).await
        }
        Command::Edit {
            input,
            instruction,
            out,
            formats,
        } => {
            let plan = Plan {
                summary: format!("edit: {instruction}"),
                steps: vec![Step::Edit {
                    instruction: instruction.clone(),
                }],
                deliverables: parse_formats(&formats.formats)?,
                unsupported: vec![],
            };
            run_with_plan(&app, input, out, Some(plan), instruction, false).await
        }
        Command::Convert { input, formats, out } => {
            let deliverables = parse_formats(&formats)?;
            let instructions = format!(
                "Convert to {}.",
                deliverables.iter().map(|f| f.label()).collect::<Vec<_>>().join(", ")
            );
            let plan = Plan {
                summary: instructions.clone(),
                steps: vec![],
                deliverables,
                unsupported: vec![],
            };
            run_with_plan(&app, input, out, Some(plan), instructions, false).await
        }
        Command::Serve { host, port, data_dir } => {
            let ctx = app.context(false)?;
            if ctx.client.is_none() {
                eprintln!(
                    "warning: no API key found in ${}; jobs that need a model will fail",
                    app.config.api.key_env
                );
            }
            let host = host.unwrap_or_else(|| app.config.server.host.clone());
            let port = port.unwrap_or(app.config.server.port);
            let data_dir = data_dir.unwrap_or_else(|| app.config.data_dir());
            notebender::web::serve(ctx, &host, port, data_dir).await
        }
        Command::Doctor => doctor(&app),
        Command::Models => {
            println!("{:<10} {:<8} {:<8} {:<24} purpose", "task", "tier", "effort", "model");
            for (task, route) in app.router.table() {
                println!(
                    "{:<10} {:<8} {:<8} {:<24} {}",
                    task.to_string(),
                    route.tier.to_string(),
                    route.effort.to_string(),
                    app.config.model_id(route.tier),
                    task.description()
                );
            }
            println!("\n{:<8} {:<24} {:>8} {:>8}", "tier", "model", "$/M in", "$/M out");
            for t in Tier::ALL {
                let p = app.config.price(t);
                println!(
                    "{:<8} {:<24} {:>8.2} {:>8.2}",
                    t.to_string(),
                    app.config.model_id(t),
                    p.input_per_mtok,
                    p.output_per_mtok
                );
            }
            println!(
                "\nDeterministic (no model): transposition, validation, page merging, musicxml2ly, LilyPond engraving."
            );
            Ok(())
        }
        Command::Config { action } => match action {
            ConfigAction::Init { path, force } => {
                let path = path.unwrap_or_else(|| PathBuf::from("notebender.toml"));
                if path.exists() && !force {
                    anyhow::bail!("{} exists (use --force to overwrite)", path.display());
                }
                std::fs::write(&path, notebender::config::EXAMPLE_TOML)?;
                println!("wrote {}", path.display());
                Ok(())
            }
            ConfigAction::Show => {
                println!("{}", app.config.to_toml());
                Ok(())
            }
            ConfigAction::Path => {
                match &app.config_path {
                    Some(p) => println!("{}", p.display()),
                    None => {
                        println!("(no config file found; using defaults). Searched:");
                        for p in Config::search_paths() {
                            println!("  {}", p.display());
                        }
                    }
                }
                Ok(())
            }
        },
    }
}

fn doctor(app: &App) -> Result<()> {
    let mut problems = 0;
    println!("notebender {}", env!("CARGO_PKG_VERSION"));
    match &app.config_path {
        Some(p) => println!("config: {}", p.display()),
        None => println!("config: built-in defaults (run `notebender config init` to create one)"),
    }
    match app.config.api_key() {
        Ok(k) => println!(
            "API key: found in ${} ({}…{})",
            app.config.api.key_env,
            &k[..k.len().min(7)],
            &k[k.len().saturating_sub(4)..]
        ),
        Err(e) => {
            println!("API key: MISSING — {e}");
            println!("         (deterministic commands still work; model-backed steps will fail)");
        }
    }
    println!("API base URL: {}", app.config.api.base_url);
    println!("\nExternal tools:");
    for t in app.tools.all() {
        match &t.path {
            Some(p) => println!("  ✓ {:<12} {}  — {}", t.name, p.display(), t.purpose),
            None => println!(
                "  ✗ {:<12} not found — {} (install: {})",
                t.name, t.purpose, t.install_hint
            ),
        }
    }
    if !app.tools.pdftoppm.available() {
        println!("\nnote: without pdftoppm, PDFs are sent to the model whole instead of page by page.");
    }
    if !app.tools.lilypond.available() {
        println!("note: without lilypond, PDF output is unavailable; LilyPond/Humdrum/ABC fall back to a model.");
    }
    println!("\nRouting:");
    for (task, route) in app.router.table() {
        println!(
            "  {:<10} → {:<7} {}",
            task.to_string(),
            route.tier.to_string(),
            app.config.model_id(route.tier)
        );
    }
    // Sanity check the deterministic engine.
    let sample = include_str!("../tests/fixtures/simple.musicxml");
    let iv = musicxml::Interval::from_semitones(2, None);
    match musicxml::transpose(sample, &iv) {
        Ok(_) => println!("\nMusicXML engine: ok"),
        Err(e) => {
            println!("\nMusicXML engine: FAILED ({e})");
            problems += 1;
        }
    }
    if problems > 0 {
        anyhow::bail!("{problems} problem(s) found");
    }
    Ok(())
}

# CLAUDE.md

Guidance for coding agents working in this repository.

## What this is

`notebender` (Rust, MIT, public repo chaumurky-dev/notebender): turns a PDF of sheet music plus a plain-language request into MusicXML / LilyPond / Humdrum / ABC / PDF. OMR, transposition, tablature, arranging and free-form edits. Claude does the judgement work; everything computable is deterministic Rust. CLI plus a local axum web UI. Early/experimental. User docs are in `README.md`.

## Layout

- `src/main.rs` CLI (clap). `src/lib.rs` architecture overview. Both front-ends call `pipeline::run_job`.
- `src/pipeline.rs` data model (`Plan`, `Step`, `Format`, `JobRequest`, `ProgressEvent`); `src/pipeline_run.rs` execution (OMR -> plan -> steps -> deliverables -> `report.md`).
- `src/models.rs` tiers, model ids, prices. `src/router.rs` task -> tier. `src/config.rs` TOML config + `EXAMPLE_TOML`.
- `src/layout.rs` engraving layout options (staff size, spacing, page targets) injected into LilyPond source; deterministic.
- `src/agents/` model-backed agents (planner, omr, tablature, arrange, edit, convert, review) and the shared `xml_task.rs` driver (validate + one repair round).
- `src/claude/` Messages API client (streaming, retries, prompt caching, fallbacks).
- `src/musicxml/` deterministic transpose / validate / merge. `src/tools.rs` + `src/pdf.rs` external tools (lilypond, musicxml2ly, pdftoppm, pdfinfo).
- `src/web/` server, `src/web/static/` UI. `tests/` integration tests, `tests/fixtures/simple.musicxml`.

## Commands

```sh
cargo build
cargo test                                          # also `cargo test --doc`
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all                                     # rustfmt.toml: max_width 120
cargo run -q -- doctor                              # tools, key, routing
cargo run -q -- transpose tests/fixtures/simple.musicxml --to Bb --format lilypond,pdf -o out/x   # offline, no key
```

CI also builds on Rust 1.89 (MSRV) with `RUSTFLAGS=-D warnings`; do not use newer language/library features.

## Conventions

- Model ids appear only in `src/models.rs` and the config (`EXAMPLE_TOML`). Everything else asks the router for a `Tier`.
- Cheapest appropriate routing: Haiku for parsing/QA, Sonnet for structured tasks, Opus for OMR and arranging. Fable is never a default.
- Deterministic first: if a step can be computed (transpose, validate, merge, musicxml2ly, lilypond), do it in code, not with a model. Model output is validated before use.
- Tests for deterministic code. Tests must not call the network or need an API key; use the fixture or a mock server (see `tests/claude_client.rs`).
- Keep `EXAMPLE_TOML` equal to the defaults (a test enforces it); config rejects unknown keys.
- If you change a CLI flag, route, config key or the pipeline, update `README.md` in the same change.
- No `unwrap` on user input or API responses; return `anyhow` errors with context.

## Secrets and privacy

- Never log, print, commit or write the API key. It lives only in the environment variable named by `[api] key_env`; the config file must not contain it. `./notebender.toml`, `out/`, `jobs/` are git-ignored; keep it that way.
- The repo is public: no machine-specific absolute paths, usernames, emails or personal data in code, docs, tests or fixtures. Use `~` or relative paths. Do not paste real doctor output with local paths.
- Do not add real copyrighted scores as fixtures.

## Prompts

System prompts are `const SYSTEM` strings at the top of each agent in `src/agents/*.rs` (shared output rules: `OUTPUT_RULES` in `xml_task.rs`). The planner's JSON schema is `plan_schema()` in `planner.rs`; update it together with `Step`.

## Adding things

New step: `Step` variant + planner schema, `TaskKind` default tier/effort in `router.rs`, agent in `src/agents/` (prefer `run_xml_task`), a branch in `apply_step` (with `--dry-run` handling), optional CLI subcommand, README. New output format: `Format` in `pipeline.rs` (`id`, `extension`, `label`, `parse`), handling in `deliver` and `agents/convert.rs`.

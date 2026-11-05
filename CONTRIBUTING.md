# Contributing

Thanks for helping. notebender is early and experimental; small, focused changes are easiest to review.

## Before you open a PR

Run what CI runs (Rust 1.89 or newer):

```sh
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets && cargo test --doc
```

For the offline pipeline you can also try, with LilyPond and poppler installed:

```sh
cargo run -q -- transpose tests/fixtures/simple.musicxml --to Bb --format lilypond,pdf -o out/check
```

Tests must not need an API key or network access.

## Adding an agent, step or format

- **Agent / step:** add a `Step` variant and planner schema entry, a `TaskKind` with its default tier in `src/router.rs`, the agent in `src/agents/` (MusicXML-in/MusicXML-out agents should use `run_xml_task`, which validates and repairs), and a branch in `apply_step` in `src/pipeline_run.rs` that honours `--dry-run`. Add a CLI subcommand if it is user-facing.
- **Format:** extend `Format` in `src/pipeline.rs` and handle it in `deliver` (`src/pipeline_run.rs`) and `src/agents/convert.rs`. Prefer a deterministic converter over a model when one exists.
- **Prefer deterministic code** over a model call whenever the result can be computed, and give it unit tests (see `src/musicxml/` and `tests/musicxml.rs`).
- Model ids belong only in `src/models.rs` and config. Route tasks to the cheapest tier that does the job well.
- Update `README.md` when behaviour, flags, routes or config change. Keep `EXAMPLE_TOML` in sync with the defaults.

## Pull requests

- CI green (fmt, clippy, tests on Linux and macOS, offline pipeline, MSRV).
- Tests for deterministic code and for bug fixes.
- Describe what changed and why; mention any cost implications for model-backed changes (more calls, bigger prompts, a more expensive tier).
- No machine-specific paths, usernames, emails or personal data in code, docs, tests or fixtures. Do not add copyrighted scores as fixtures.

## Security

Never commit an API key, a `notebender.toml` containing secrets, job data or outputs. The key is read only from the environment (`ANTHROPIC_API_KEY` by default) and the config rejects a literal key. If you find a leaked credential or a security problem, do not open a public issue with the secret in it; rotate the key and contact the maintainers privately.

By contributing you agree your work is licensed under the MIT licence.

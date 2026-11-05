# notebender

Upload a PDF of sheet music, describe a transformation in plain words, and get back MusicXML, LilyPond, Humdrum, ABC and/or an engraved PDF. Supported transformations: transcription (optical music recognition, OMR), transposition, guitar tablature, arranging for other instruments, and free-form edits ("simplify the left hand"). The work is driven by Claude: each task is routed to the cheapest model tier that is appropriate for it, and deterministic steps (transposition, validation, page merging, LilyPond conversion and engraving) use no model at all. It has a CLI and a small local web UI.

**Status:** early and experimental, meant for local use. Not deployed anywhere. Always check the output against the original score.

## Quick start

macOS (Homebrew):

```sh
# Rust (stable, 1.89 or newer) and the optional external tools
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install lilypond poppler

git clone https://github.com/chaumurky-dev/notebender.git
cd notebender
cargo build --release
export PATH="$PWD/target/release:$PATH"      # or run `cargo run --release -- ...`

export ANTHROPIC_API_KEY=...                 # never put this in a file in the repo
notebender doctor
```

Debian/Ubuntu equivalent of the tools: `sudo apt install lilypond poppler-utils`.

Try it:

```sh
# Transcribe a PDF, transpose it from G to C, add guitar tab, and get a PDF back
notebender run score.pdf -p "Transpose to C and add guitar tab" -o out/demo

# No API key needed: transpose existing MusicXML and engrave it
notebender transpose tests/fixtures/simple.musicxml --to Bb --format lilypond,pdf -o out/bb
```

Web UI:

```sh
notebender serve        # then open http://127.0.0.1:8787
```

## Dependencies

Everything except the items below is a Rust crate fetched by cargo. There is no Python, and no system TLS library is needed (HTTPS uses rustls). The network is used only to call the Anthropic API.

| Kind | Dependency | Purpose | Without it | macOS (brew) | Debian/Ubuntu (apt) | Windows |
|---|---|---|---|---|---|---|
| Build | Rust stable >= 1.89 (cargo) | Compile the crate | Cannot build | `brew install rustup` or rustup.rs | rustup.rs | rustup.rs |
| Runtime, required | Anthropic API key with access to the configured models | OMR, planning, tablature, arranging, edits, non-LilyPond conversion | Only deterministic work runs: `transpose` / `convert --to lilypond,pdf` on MusicXML input | n/a | n/a | n/a |
| Runtime, optional | LilyPond (includes `musicxml2ly`) | Engrave PDF; deterministic MusicXML to LilyPond | No PDF output; LilyPond export falls back to a model | `brew install lilypond` | `sudo apt install lilypond` | [lilypond.org/download](https://lilypond.org/download.html) |
| Runtime, optional | poppler (`pdftoppm`, `pdfinfo`) | Rasterise PDF pages for page-by-page OMR; count pages | The whole PDF is sent to the model in one request; page count is a rough scan | `brew install poppler` | `sudo apt install poppler-utils` | Poppler Windows builds, e.g. [oschwartz10612/poppler-windows](https://github.com/oschwartz10612/poppler-windows) |
| Optional, detected only | verovio | Not used yet (reserved for MusicXML to Humdrum/SVG) | Nothing | `brew install verovio` | see [verovio.org](https://www.verovio.org) | see verovio.org |
| Optional, detected only | MuseScore (`mscore`) | Not used yet (reserved for MusicXML to PDF) | Nothing | [musescore.org/download](https://musescore.org/download) | [musescore.org/download](https://musescore.org/download) | [musescore.org/download](https://musescore.org/download) |

`notebender doctor` reports which tools it found and what install command it suggests for the rest. Tool names/paths can be changed in the `[tools]` config section.

## CLI reference

```
notebender [--config FILE] [--model TASK=TIER]... [-v|-vv] <COMMAND>
```

Global flags (accepted by every subcommand):

| Flag | Meaning |
|---|---|
| `--config FILE` | Config file (also `NOTEBENDER_CONFIG`). Default: `./notebender.toml`, then the platform config dir. |
| `--model TASK=TIER` | Route one task to a tier for this run, repeatable, e.g. `--model omr=fable`. Tasks: `plan, omr, convert, tablature, arrange, edit, review`. Tiers: `haiku, sonnet, opus, fable`. |
| `-v`, `-vv` | Debug / trace logging (to stderr). |

Subcommands that process a score share these options: `-o, --out DIR` (default `./out/<input-stem>`) and `--dry-run` (do not call the API: run the deterministic steps and describe what the model-backed ones would do). Input is a PDF, or a `.musicxml`/`.xml` file, which skips OMR. Formats are `musicxml` (always written), `lilypond`, `pdf`, `humdrum`, `abc`; `--format` takes a comma-separated list and can be repeated.

| Command | Flags | What it does |
|---|---|---|
| `run INPUT -p PROMPT` | `--review` | Free-text request; the planner (Haiku) decides the steps and deliverables. `--review` adds a cheap QA pass. |
| `omr INPUT` | `--format FMT` | Transcribe a PDF to MusicXML, optionally with other formats. |
| `transpose INPUT` | `-s, --semitones N` or `--to KEY`; `--from KEY`; `--format FMT` | Deterministic transposition. `--to` and `--semitones` conflict; `--from` requires `--to` and defaults to the key detected in the score. |
| `tab INPUT` | `--instrument NAME` (default `guitar`), `--tuning TEXT`, `--capo N`, `--format FMT` | Add a tablature part. |
| `arrange INPUT` | `--for A,B` (required), `--notes TEXT`, `--format FMT` | Arrange for the listed instruments, in score order. |
| `edit INPUT -i TEXT` | `--format FMT` | Apply a free-form edit described in words (Sonnet), e.g. "remove the repeats". |
| `convert INPUT` | `--to FMT` (required, repeatable) | Convert to other notation formats. |

Every score subcommand also takes **layout flags** for the engraved output (`--scale`, `--staff-size`, `--spacing`, `--system-spacing`, `--page-count`, `--systems-per-page`, `--paper-size`); see [Layout](#layout-size-and-spacing). Using any of them implies a PDF deliverable.
| `serve` | `--host`, `--port`, `--data-dir` | Start the web UI (defaults from config: `127.0.0.1:8787`). |
| `doctor` | | Check config, API key, external tools, routing and the MusicXML engine. A missing key is reported as a warning; exits non-zero only if the engine self-test fails. |
| `models` | | Print the task to model routing table and prices. |
| `config init [--path P] [--force]` | | Write the commented default config to `./notebender.toml`. |
| `config show` | | Print the effective configuration. |
| `config path` | | Print which config file is used, or the places searched. |

Examples:

```sh
notebender omr scan.pdf --format lilypond --format pdf -o out/scan
notebender run song.musicxml -p "Remove the repeats and simplify the left hand" --review
notebender transpose song.musicxml -s -2                 # down a tone, spelled for the key
notebender transpose song.musicxml --from G --to C --format pdf
notebender tab song.pdf --instrument guitar --tuning "drop D" --capo 2
notebender arrange piano.pdf --for "cello,guitar" --notes "easy, keep the melody in the cello"
notebender edit song.musicxml -i "double the melody an octave up in the last 8 bars"
notebender convert song.musicxml --to lilypond,abc,humdrum
notebender --model omr=fable omr hard-handwriting.pdf
notebender transpose song.musicxml --to D --dry-run
notebender convert song.musicxml --to pdf --scale 0.8 --spacing 0.9     # fit more on each page
notebender convert song.musicxml --to pdf --page-count 2                 # squeeze onto two pages
notebender run song.pdf -p "make it bigger and more readable"           # planner sets layout
```

`transpose` and `convert --to lilypond,pdf` on MusicXML input need no API key as long as the local tools (musicxml2ly, LilyPond) exist. `--dry-run` needs an explicit plan, so `run --dry-run` is rejected; use a specific subcommand. A dry run cannot transcribe a PDF.

Outputs go in the output directory: `score.musicxml`, `score.ly` / `score.krn` / `score.abc` / `score.pdf` as requested, `transcription.musicxml` (raw OMR result, PDF input only), `report.md`, and a `.work/` scratch directory (page images, intermediate files).

## Layout: size and spacing

Engraved output (LilyPond/PDF) can be made more compact to fit more music per page, or larger for readability. This is deterministic (no model) and works on both `musicxml2ly` output and model-written LilyPond: notebender injects a small block after the `\version` line. In free-text requests the planner maps phrases like "smaller", "fit on two pages", "bigger", "tighter spacing" onto these options.

| Option | CLI flag | Effect |
|---|---|---|
| `scale` | `--scale 0.8` | Overall size multiplier (sets the global staff size; LilyPond default is 20pt). 0.8 fits roughly 25% more per page; 1.2 is large print. |
| `staff_size` | `--staff-size 16` | Absolute staff size in points (overrides `scale`). |
| `spacing` | `--spacing 0.8` | Horizontal note spacing multiplier: more or fewer notes per line at the same staff size. |
| `system_spacing` | `--system-spacing 0.75` | Vertical gap between systems. |
| `page_count` | `--page-count 2` | Ask LilyPond to fit the piece on exactly N pages (it compresses to fit; check the result). |
| `systems_per_page` | `--systems-per-page 6` | Fixed number of systems per page. |
| `paper_size` | `--paper-size letter` | Paper name: a4, letter, legal, a3, … |

Values are range-checked (scale 0.4–2.5, staff size 8–50pt, spacing 0.4–3). Re-running on an already laid-out `.ly` replaces the previous block instead of stacking.

## Web UI

`notebender serve` starts a small local server with a single-page UI: pick a score file (PDF or MusicXML), write what you want in words, optionally tick "QA review", and watch the plan and per-step progress. Finished jobs list downloadable files. Jobs run one at a time. The UI always goes through the planner, so it needs an API key even for a plain transposition.

| Route | Purpose |
|---|---|
| `GET /` | The UI (plus `/static/app.js`, `/static/style.css`) |
| `GET /api/health` | API key present?, external tools, routing table |
| `POST /api/jobs` | Multipart form. Fields: `file` (.pdf, .musicxml or .xml; required), `instructions` (required), `review` (optional, truthy), `dry_run` (optional, truthy). Returns `201 {"id": "<uuid>"}`. Upload cap is `limits.max_upload_mb`. |
| `GET /api/jobs` | List jobs |
| `GET /api/jobs/{id}` | Status (`queued`, `running`, `done`, `failed`), progress events and the result |
| `GET /api/jobs/{id}/files/{name}` | Download an output file (`score.musicxml`, `score.pdf`, `report.md`, ...) |
| `DELETE /api/jobs/{id}` | Delete a job and its files (409 while it is still running) |

Storage, under the data dir (`--data-dir`, else `[server] data_dir`, else `NOTEBENDER_DATA_DIR`, else the platform data dir such as `~/Library/Application Support/notebender` on macOS or `~/.local/share/notebender` on Linux):

```
<data_dir>/jobs/<uuid>/
  input.<pdf|musicxml|xml>   uploaded score
  request.json               job request
  job.json                   status, progress events, result
  out/                       score.musicxml, score.pdf, report.md, ...
```

Jobs that were queued or running when the server stopped are marked failed ("interrupted by restart") on the next start.

**Security:** the server binds to `127.0.0.1` by default and has **no authentication**. Anyone who can reach it can spend your API credit and read your jobs. Do not expose it on a network (`--host 0.0.0.0`, port forwarding, a tunnel) as it is.

## Configuration

The config file is optional; every key has a default. Lookup order:

1. `--config PATH`
2. `$NOTEBENDER_CONFIG`
3. `./notebender.toml`
4. `$XDG_CONFIG_HOME/notebender/config.toml` (macOS: `~/Library/Application Support/notebender/config.toml`)
5. Built-in defaults

(An explicit `--config` path that does not exist is an error; a missing file elsewhere is not.) `notebender config path` shows what was picked; `notebender config init` writes the example below; `notebender config show` prints the effective settings. Unknown keys are rejected, so typos are caught.

| Environment variable | Use |
|---|---|
| `ANTHROPIC_API_KEY` | The API key. The variable name can be changed with `[api] key_env`. |
| `NOTEBENDER_CONFIG` | Path to the config file. |
| `NOTEBENDER_DATA_DIR` | Web UI data directory (overridden by `--data-dir` and `[server] data_dir`). |

**The API key never goes in the config file.** The file only names the environment variable that holds it, and a literal `key = "sk-..."` entry is rejected as an unknown key. `./notebender.toml` is git-ignored.

```toml
# notebender configuration.
# Every key is optional; omitted keys use the built-in defaults shown here.
# The API key is NEVER stored in this file: put it in the environment variable named by `key_env`.

[api]
key_env = "ANTHROPIC_API_KEY"          # environment variable holding the key
base_url = "https://api.anthropic.com"
max_retries = 3
timeout_secs = 1800
fallbacks = true                       # server-side refusal fallbacks on Sonnet/Opus/Fable

# Tier → model id. Update these when new models ship.
[models]
haiku = "claude-haiku-4-5"
sonnet = "claude-sonnet-5-5"
opus = "claude-opus-5-5"
fable = "claude-fable-5-1"

# USD per million tokens, used only for the cost estimates in reports.
[prices.haiku]
input_per_mtok = 1.0
output_per_mtok = 5.0
[prices.sonnet]
input_per_mtok = 2.0
output_per_mtok = 10.0
[prices.opus]
input_per_mtok = 4.0
output_per_mtok = 20.0
[prices.fable]
input_per_mtok = 10.0
output_per_mtok = 50.0

# Which tier handles which task ("cheapest appropriate"). Transposition, validation,
# page merging and LilyPond engraving are deterministic and never use a model.
[routing]
plan = "haiku"        # parse the request into steps
omr = "opus"          # read the score; try "fable" for hard handwriting
convert = "sonnet"    # LilyPond/Humdrum/ABC when no local converter exists
tablature = "sonnet"
arrange = "opus"
edit = "sonnet"
review = "haiku"

# Reasoning effort per task (low|medium|high|xhigh|max); ignored on Haiku.
[effort]
plan = "low"
omr = "high"
convert = "medium"
tablature = "medium"
arrange = "high"
edit = "medium"
review = "low"

# Command names or absolute paths of optional external tools.
[tools]
lilypond = "lilypond"
musicxml2ly = "musicxml2ly"
pdftoppm = "pdftoppm"
pdfinfo = "pdfinfo"
verovio = "verovio"
musescore = "mscore"

[limits]
max_pdf_pages = 40
max_upload_mb = 32
page_dpi = 150
omr_concurrency = 3

[server]
host = "127.0.0.1"
port = 8787
# data_dir = "/path/to/notebender-data"   # default: platform data dir
```

## Model routing and cost

`notebender models` prints the routing and the prices used for estimates:

```
task       tier     effort   model                    purpose
plan       haiku    low      claude-haiku-4-5         parse the request into a plan
omr        opus     high     claude-opus-5-5          read the score (PDF/images) into MusicXML
convert    sonnet   medium   claude-sonnet-5-5        convert notation formats (LLM fallback)
tablature  sonnet   medium   claude-sonnet-5-5        add fretted-instrument tablature
arrange    opus     high     claude-opus-5-5          arrange for different instruments
edit       sonnet   medium   claude-sonnet-5-5        apply a free-form edit to the score
review     haiku    low      claude-haiku-4-5         QA the result against the request

tier     model                      $/M in  $/M out
haiku    claude-haiku-4-5             1.00     5.00
sonnet   claude-sonnet-5-5            2.00    10.00
opus     claude-opus-5-5              4.00    20.00
fable    claude-fable-5-1            10.00    50.00

Deterministic (no model): transposition, validation, page merging, musicxml2ly, LilyPond engraving.
```

The rule is "cheapest appropriate":

- **Haiku** for parsing the request into a structured plan and for the QA pass; both are short, structured and need no musical judgement.
- **Sonnet** for tasks with a clear structure to follow: choosing fingerings for tablature, free-form edits, and format conversion when no local converter exists.
- **Opus** for open-ended musical decisions: reading a scanned score (OMR) and re-arranging for new instruments, where mistakes are expensive to fix later.
- **Fable** is never used by default. Opt in per task, e.g. `--model omr=fable` or `[routing] omr = "fable"`.
- **No model** for everything that can be computed.

Overrides: set `[routing]` (and `[effort]`) in the config for a permanent change, or pass `--model task=tier` (repeatable) for one run. Model ids live in `[models]`, so a new model release is a config change.

**Cost reporting.** Every job writes `report.md` next to the outputs: the request, the plan, original and final score summaries, and a per-step table of model, input/output tokens, estimated cost and seconds, plus a total, notes from the models and warnings. The CLI prints the same per step as it goes. Costs are computed from the list prices in `[prices]`; they are estimates only and do not reflect discounts, cache pricing or what your account is actually billed.

**Prompt caching.** Each agent's system prompt is sent with a cache marker, so repeated calls with the same prefix (for example the per-page OMR requests) can be served from the cache.

**Refusal fallbacks.** With `[api] fallbacks = true` (default), requests to Sonnet, Opus and Fable opt into the API's server-side fallback: if a safety classifier declines a request, Anthropic re-runs it on its recommended fallback model instead of failing the step. When this happens the step notes record "served by fallback model ...", and the cost of that step may differ from the estimate. Set `fallbacks = false` to disable.

## How it works

1. **Input.** A `.musicxml`/`.xml` file is validated and used as is. A PDF goes through OMR first. (Compressed `.mxl` is rejected.)
2. **Prepare pages** (deterministic). With poppler, each page is rasterised to a PNG (`limits.page_dpi`); PDFs longer than `limits.max_pdf_pages` are refused because each page is a model call.
3. **OMR** (Opus). Each page is transcribed to MusicXML in its own request. Page 1 goes first; its part list is passed to the other pages as context so part ids and names stay consistent, and those pages run concurrently (`limits.omr_concurrency`). Per-page documents are merged deterministically in Rust. Without poppler the whole PDF is sent as one request.
4. **Plan** (Haiku). For `run` and the web UI, the request (and a structural summary of the score) becomes a plan: steps, deliverable formats, and anything unsupported (reported as a warning and skipped). The other subcommands build the plan directly with no model call.
5. **Steps**, in order:
   - **Transpose** (Rust, no model). Pitches, key signatures and chord symbols are rewritten. With `--semitones`, the interval's spelling (e.g. augmented fourth vs diminished fifth) is chosen so the resulting key signature has the fewest accidentals; for example C up 1 semitone becomes Db major (5 flats) rather than C# major (7 sharps), and C up 6 becomes F# major. With `--to`, the spelling follows the target key and the source key is detected from the score unless `--from` is given.
   - **Tablature** (Sonnet). Adds a separate `TAB` part rather than altering existing staves, with string tuning in `staff-details` and `<string>`/`<fret>` technicals on every note, tuned to stay in position and within fret 19. An existing guitar part is tabbed in place of deriving one.
   - **Arrange** (Opus). Writes one part per requested instrument, in the requested order, with ranges, clefs and transpositions handled.
   - **Edit** (Sonnet). Any other change described in words.
   - Every model step that returns MusicXML is validated. On failure the validator error is sent back for **one repair round**; if it still fails, a warning is recorded. If a reply hits the token limit, the model is asked to **continue** (a few times at most), and the report notes it.
6. **Deliverables.** `score.musicxml` is always written. LilyPond uses `musicxml2ly` when installed (deterministic), otherwise a Sonnet call; Humdrum and ABC use a Sonnet call (no local converter yet). A failed export becomes a warning, not a failed job.
7. **Engraving** (deterministic). For PDF, the LilyPond source is compiled with `lilypond`. Without LilyPond the PDF is skipped with a warning.
8. **Review** (optional, Haiku). `--review` checks structural summaries and the step notes against the request.
9. **Report.** `report.md`, described above.

## Limitations

- **OMR is only as good as the model and the scan.** Expect wrong notes, dropped ornaments and rhythm slips, especially on dense, skewed or handwritten pages. The model marks uncertain spots with `<!-- OMR: uncertain ... -->` comments in the raw transcription. Verify against the original before you rely on the result, and treat arrangements and tablature as a first draft.
- **Long pieces cost real money.** Rough estimate at the default Opus prices ($4 / $20 per million tokens): a 4-page piano piece is 4 Opus OMR calls of roughly 3-5k tokens in (page image, system prompt, part list) and 10-30k tokens out (verbose MusicXML plus reasoning), about $0.20-$0.60 per page, so around **$1-$2.50 for the transcription**. An Opus arrangement of the result adds a similar amount again, since the whole score goes in and comes out. Transposition, validation and engraving are free. Check `report.md` for real numbers; these are estimates.
- Compressed `.mxl` is not supported; unzip it to `.musicxml`.
- The web UI has no authentication (see above). Jobs run one at a time and live only in memory plus the data dir.
- Verovio and MuseScore are detected but not used. Humdrum and ABC export rely on a model.
- Not deployed anywhere; there is no hosted version.

## Development

```sh
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
```

CI (`.github/workflows/ci.yml`, with `RUSTFLAGS=-D warnings`) runs on pushes to `main` and on pull requests:

- `fmt + clippy`: `cargo fmt --all -- --check` and clippy with `-D warnings` (ubuntu)
- `test`: `cargo test --all-targets` and `cargo test --doc` on ubuntu and macOS
- `offline pipeline`: installs LilyPond and poppler, then runs `transpose tests/fixtures/simple.musicxml --to Bb --format lilypond,pdf` and checks the PDF, LilyPond file and key signature (no API key)
- `MSRV`: `cargo check --all-targets` on Rust 1.89

Layout:

```
src/
  main.rs            CLI (clap): subcommands, progress printing, doctor, models
  lib.rs             crate docs: architecture overview
  config.rs          TOML config, lookup order, env vars, EXAMPLE_TOML
  models.rs          tiers, model ids, list prices, effort levels
  router.rs          task -> tier routing and --model overrides
  tools.rs           external tool detection and wrappers (lilypond, musicxml2ly, pdftoppm, pdfinfo)
  pdf.rs             input classification, page counting, rasterising
  pipeline.rs        data model: Plan, Step, Format, JobRequest, JobResult, ProgressEvent
  pipeline_run.rs    run_job: OMR -> plan -> steps -> deliverables -> report.md
  claude/            Anthropic Messages API client (streaming, retries, caching, fallbacks)
  agents/            model-backed agents; system prompts live here
    planner.rs         request -> Plan (structured output)
    omr.rs             page-by-page transcription and merge
    xml_task.rs        shared "MusicXML in, MusicXML out" driver with validation and repair
    tablature.rs arrange.rs edit.rs   thin wrappers over xml_task
    convert.rs         musicxml2ly first, model fallback
    review.rs          optional QA pass
  musicxml/          deterministic: transpose.rs, validate.rs, merge.rs
  web/               axum server (mod.rs), static UI (static/), tests
tests/               integration tests and fixtures
```

**Adding a step or agent.** Add a `Step` variant (and its JSON schema entry) in `pipeline.rs` / `agents/planner.rs`; add a `TaskKind` with a default tier, effort, token ceiling and description in `router.rs` (and the `[routing]`/`[effort]` entries via defaults in `config.rs`, plus `EXAMPLE_TOML`); implement the agent in `src/agents/` (for MusicXML in/out, call `run_xml_task` with a system prompt and `OUTPUT_RULES`, which gives you validation and repair); handle the variant in `apply_step` in `pipeline_run.rs`, including the `--dry-run` branch; add a CLI subcommand if it deserves one. Prefer a deterministic implementation under `src/musicxml/` when the job can be computed. Add tests for deterministic code. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Licence

MIT. See [LICENSE](LICENSE).

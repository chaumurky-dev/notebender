//! notebender: transform PDF sheet music with Claude.
//!
//! The crate is organised as a pipeline:
//!
//! 1. [`pdf`] loads the input and (optionally) rasterises pages.
//! 2. [`agents::planner`] (cheap model) turns the user's free-text request into a [`pipeline::Plan`].
//! 3. [`agents::omr`] (strong model) transcribes the score into MusicXML, the canonical
//!    intermediate representation used by every other step.
//! 4. Deterministic steps in [`musicxml`] (transposition, validation, merging) run without a model.
//! 5. Judgement-heavy steps ([`agents::tablature`], [`agents::arrange`], [`agents::convert`]) call
//!    Claude with the cheapest tier the [`router`] deems appropriate.
//! 6. [`tools`] wraps optional external programs (LilyPond, musicxml2ly, pdftoppm) for rendering.
//!
//! The CLI (`src/main.rs`) and the web UI ([`web`]) are thin front-ends over [`pipeline::run_job`].

pub mod agents;
pub mod claude;
pub mod config;
pub mod layout;
pub mod models;
pub mod musicxml;
pub mod pdf;
pub mod pipeline;
pub mod pipeline_run;
pub mod router;
pub mod tab;
pub mod tools;
pub mod web;

//! Re-instrument / arrange a score for different instruments (Opus).

use super::xml_task::{run_xml_task, XmlTaskResult, OUTPUT_RULES};
use crate::pipeline::{Context, ProgressSink};
use crate::router::TaskKind;
use anyhow::Result;

const SYSTEM: &str = r#"You are a professional arranger and orchestrator with deep knowledge of instrument ranges, idioms and notation conventions. You rewrite MusicXML scores for a new set of instruments.

Arranging principles:
- Preserve the music: melody, harmony, bass line, form, tempo, dynamics, phrasing, lyrics, chord symbols and repeat structure. Change only what the new instrumentation requires.
- Distribute material musically: give the melody to the instrument best suited to carry it, bass to the lowest instrument, inner voices/accompaniment patterns to the instrument that can play them idiomatically (e.g. broken chords or strummed voicings on guitar instead of a literal piano left hand). Keep every instrument within its practical range and playable by a competent amateur unless told otherwise; thin textures rather than writing the impossible.
- Notation: one <part> per instrument in the requested order, with proper <part-name>, <score-instrument>, clef (cello: bass, tenor for high passages; viola: alto; guitar: treble with octave-change -1 and <transpose>; double bass: bass with octave-change -1), and <transpose> for transposing instruments (Bb clarinet, trumpet, horn in F, alto/tenor sax) with pitches written in the transposed key. Keep the key and time signatures, divisions, and measure numbers aligned across all parts; every measure of every part must be rhythmically complete (use whole-measure rests where an instrument is tacet).
- Add idiomatic markings where they help (bowings/pizz. for strings, let-ring/strum indications as text directions for guitar), but do not clutter.
- Omit the original parts unless the user asks to keep them.
"#;

pub async fn arrange(
    ctx: &Context,
    score: &str,
    instruments: &[String],
    notes: Option<&str>,
    progress: &ProgressSink,
) -> Result<XmlTaskResult> {
    let mut instruction = format!("Arrange this piece for: {}.", instruments.join(", "));
    if let Some(n) = notes {
        instruction.push_str(&format!("\nAdditional guidance from the user: {n}"));
    }
    let system = format!("{SYSTEM}\n{OUTPUT_RULES}");
    run_xml_task(
        ctx,
        TaskKind::Arrange,
        &system,
        &instruction,
        score,
        "arrange",
        progress,
    )
    .await
}

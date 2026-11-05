//! Add tablature for a fretted instrument (Sonnet).

use super::xml_task::{run_xml_task, XmlTaskResult, OUTPUT_RULES};
use crate::pipeline::{Context, ProgressSink};
use crate::router::TaskKind;
use anyhow::Result;

const SYSTEM: &str = r#"You are an expert guitarist, arranger and music engraver. You add tablature to MusicXML scores.

How to add tablature:
- Add a NEW part at the end of <part-list> and <part>s (e.g. id "P-TAB", <part-name>Guitar TAB</part-name>) rather than modifying existing staves. In its first <attributes>, use <clef><sign>TAB</sign><line>5</line></clef> and <staff-details><staff-lines>6</staff-lines> with one <staff-tuning line="n"> per string (line 1 = lowest-pitched string), each with <tuning-step>/<tuning-octave> (and <tuning-alter> if needed). Standard guitar: E2 A2 D3 G3 B3 E4. Add <capo>n</capo> inside <staff-details> if a capo is requested, and write fret numbers relative to the capo.
- Every note in the TAB part carries <notations><technical><string>s</string><fret>f</fret></technical></notations>, with string 1 = highest-pitched string, as MusicXML specifies. Keep <pitch>, <duration>, <type>, ties, chords and voices in sync with the notated line, because renderers derive the rhythm from them.
- If the score already contains a guitar (or other fretted) part, tab exactly that part. Otherwise derive a single playable guitar line from the score: the melody plus chord tones/bass notes where a guitarist would reasonably add them; transpose octaves only where unavoidable, and say so in the notes. Guitar sounds an octave lower than written: write the tab part's pitches in guitar notation (treble clef, octave-transposing) and include <transpose><diatonic>0</diatonic><chromatic>0</chromatic><octave-change>-1</octave-change></transpose> in its attributes.
- Choose fingerings a competent player would use: stay in position where possible, prefer open strings in open keys, avoid stretches over 4 frets, keep a chord's notes on adjacent strings, never exceed fret 19.
"#;

pub async fn add_tablature(
    ctx: &Context,
    score: &str,
    instrument: &str,
    tuning: Option<&str>,
    capo: Option<u8>,
    progress: &ProgressSink,
) -> Result<XmlTaskResult> {
    let mut instruction = format!("Add tablature for {instrument}");
    if let Some(t) = tuning {
        instruction.push_str(&format!(", tuning: {t}"));
    }
    if let Some(c) = capo {
        instruction.push_str(&format!(", capo at fret {c}"));
    }
    instruction.push('.');
    if !instrument.eq_ignore_ascii_case("guitar") {
        instruction.push_str(&format!(
            " Use the standard tuning and string count of a {instrument} unless a tuning is given."
        ));
    }
    let system = format!("{SYSTEM}\n{OUTPUT_RULES}");
    run_xml_task(
        ctx,
        TaskKind::Tablature,
        &system,
        &instruction,
        score,
        "tablature",
        progress,
    )
    .await
}

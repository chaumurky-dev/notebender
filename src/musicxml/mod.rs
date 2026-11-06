//! Deterministic MusicXML processing (no model involved).
//!
//! * [`transpose`] — chromatic/diatonic transposition of pitches, key signatures and harmony.
//! * [`validate`] — well-formedness plus a structural [`Summary`] (parts, measures, keys, clefs).
//! * [`merge`] — stitch per-page partwise documents (from page-by-page OMR) into one score.

pub mod merge;
pub mod repair;
pub mod transpose;
pub mod validate;

pub use merge::merge_pages;
pub use repair::{beams_are_sparse, fix_missing_backups};
pub use transpose::{detect_key, transpose, Interval, Key, KeyParseError, Mode};
pub use validate::{summarize, validate, PartSummary, Summary, ValidationError};

use quick_xml::events::Event;
use quick_xml::Reader;

/// Parse with DOCTYPE allowed (MusicXML files normally carry one).
pub(crate) fn parse_doc(xml: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    Start(String),
    End(String),
    Empty(String),
    Text,
    Other,
}

/// A slice of the source document; concatenating all `raw` fields reproduces it exactly.
#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: Kind,
    pub raw: String,
}

pub(crate) fn tokenize(xml: &str) -> anyhow::Result<Vec<Token>> {
    let mut reader = Reader::from_str(xml);
    let mut tokens = Vec::new();
    let mut prev = 0usize;
    loop {
        let ev = reader.read_event()?;
        let end = reader.buffer_position() as usize;
        let kind = match &ev {
            Event::Eof => break,
            Event::Start(e) => Kind::Start(e.name().into_inner().to_string()),
            Event::End(e) => Kind::End(e.name().into_inner().to_string()),
            Event::Empty(e) => Kind::Empty(e.name().into_inner().to_string()),
            Event::Text(_) | Event::CData(_) | Event::GeneralRef(_) => Kind::Text,
            _ => Kind::Other,
        };
        tokens.push(Token {
            kind,
            raw: xml[prev..end].to_string(),
        });
        prev = end;
    }
    if prev < xml.len() {
        tokens.push(Token {
            kind: Kind::Other,
            raw: xml[prev..].to_string(),
        });
    }
    Ok(tokens)
}

pub(crate) fn join(tokens: &[Token]) -> String {
    tokens.iter().map(|t| t.raw.as_str()).collect()
}

/// Index of the `End` token matching the `Start` at `i`.
pub(crate) fn matching_end(t: &[Token], i: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (j, tok) in t.iter().enumerate().skip(i) {
        match tok.kind {
            Kind::Start(_) => depth += 1,
            Kind::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            _ => {}
        }
    }
    None
}

//! Structural validation and summarisation of score-partwise MusicXML.

use super::parse_doc;
use super::transpose::{key_of, Key};
use anyhow::Context;
use roxmltree::Node;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("not well-formed XML: {0}")]
    NotXml(String),
    #[error("root element must be <score-partwise>, found <{0}>")]
    WrongRoot(String),
    #[error("missing <part-list> or it has no <score-part>")]
    NoPartList,
    #[error("document has no <part> elements")]
    NoParts,
    #[error("part {part:?} has no measures")]
    EmptyPart { part: String },
    #[error("measure {measure}: bad pitch: {detail}")]
    BadPitch { measure: String, detail: String },
    #[error("part {part:?}: a <measure> has no number attribute")]
    MissingMeasureNumber { part: String },
    #[error("part-list and parts disagree: {0}")]
    PartListMismatch(String),
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.has_tag_name(name))
}

pub fn validate(xml: &str) -> Result<(), ValidationError> {
    let doc = parse_doc(xml).map_err(|e| ValidationError::NotXml(e.to_string()))?;
    let root = doc.root_element();
    if !root.has_tag_name("score-partwise") {
        return Err(ValidationError::WrongRoot(root.tag_name().name().to_string()));
    }
    let list = children(root, "part-list").next().ok_or(ValidationError::NoPartList)?;
    let listed: BTreeSet<&str> = children(list, "score-part").filter_map(|p| p.attribute("id")).collect();
    if listed.is_empty() {
        return Err(ValidationError::NoPartList);
    }
    let parts: Vec<Node> = children(root, "part").collect();
    if parts.is_empty() {
        return Err(ValidationError::NoParts);
    }
    let ids: BTreeSet<&str> = parts.iter().filter_map(|p| p.attribute("id")).collect();
    if ids != listed {
        return Err(ValidationError::PartListMismatch(format!(
            "part-list ids {listed:?} vs part ids {ids:?}"
        )));
    }
    for part in parts {
        let id = part.attribute("id").unwrap_or("").to_string();
        let measures: Vec<Node> = children(part, "measure").collect();
        if measures.is_empty() {
            return Err(ValidationError::EmptyPart { part: id });
        }
        for m in measures {
            let Some(num) = m.attribute("number") else {
                return Err(ValidationError::MissingMeasureNumber { part: id });
            };
            for p in m.descendants().filter(|n| n.has_tag_name("pitch")) {
                let has = |name| children(p, name).any(|c| c.text().is_some_and(|t| !t.trim().is_empty()));
                if !has("step") {
                    return Err(ValidationError::BadPitch {
                        measure: num.to_string(),
                        detail: "<pitch> without <step>".into(),
                    });
                }
                if !has("octave") {
                    return Err(ValidationError::BadPitch {
                        measure: num.to_string(),
                        detail: "<pitch> without <octave>".into(),
                    });
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Summary {
    pub title: Option<String>,
    pub composer: Option<String>,
    pub parts: Vec<PartSummary>,
    /// Maximum measure count over parts.
    pub measures: usize,
    pub key: Option<Key>,
    /// e.g. "4/4".
    pub time: Option<String>,
    pub has_tablature: bool,
    pub note_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PartSummary {
    pub id: String,
    pub name: String,
    pub instrument: Option<String>,
    pub measures: usize,
    /// e.g. "G2", "F4", "TAB".
    pub clefs: Vec<String>,
    pub staves: u8,
    pub note_count: usize,
}

fn text_of(n: Node, name: &'static str) -> Option<String> {
    children(n, name)
        .next()
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[allow(clippy::field_reassign_with_default)]
pub fn summarize(xml: &str) -> anyhow::Result<Summary> {
    let doc = parse_doc(xml).context("not well-formed XML")?;
    let root = doc.root_element();
    let mut s = Summary::default();
    s.title = children(root, "work")
        .next()
        .and_then(|w| text_of(w, "work-title"))
        .or_else(|| text_of(root, "movement-title"));
    s.composer = children(root, "identification")
        .next()
        .and_then(|i| {
            children(i, "creator")
                .find(|c| c.attribute("type") == Some("composer"))
                .and_then(|c| c.text())
        })
        .map(|t| t.trim().to_string());
    s.key = root
        .descendants()
        .find(|n| n.has_tag_name("key") && children(*n, "fifths").next().is_some())
        .and_then(key_of);
    s.time = root
        .descendants()
        .find(|n| n.has_tag_name("time"))
        .and_then(|t| Some(format!("{}/{}", text_of(t, "beats")?, text_of(t, "beat-type")?)));
    let list = children(root, "part-list").next();
    for part in children(root, "part") {
        let id = part.attribute("id").unwrap_or("").to_string();
        let sp = list.and_then(|l| children(l, "score-part").find(|p| p.attribute("id") == Some(id.as_str())));
        let mut ps = PartSummary {
            name: sp.and_then(|p| text_of(p, "part-name")).unwrap_or_else(|| id.clone()),
            instrument: sp.and_then(|p| {
                p.descendants()
                    .find(|n| n.has_tag_name("instrument-name"))
                    .and_then(|n| n.text())
                    .map(|t| t.trim().to_string())
            }),
            id,
            staves: 1,
            ..Default::default()
        };
        let mut staves = 1u32;
        for m in children(part, "measure") {
            ps.measures += 1;
            for n in children(m, "note") {
                if children(n, "rest").next().is_none() {
                    ps.note_count += 1;
                }
                if let Some(st) = text_of(n, "staff").and_then(|t| t.parse::<u32>().ok()) {
                    staves = staves.max(st);
                }
            }
            for a in children(m, "attributes") {
                if let Some(n) = text_of(a, "staves").and_then(|t| t.parse::<u32>().ok()) {
                    staves = staves.max(n);
                }
                for c in children(a, "clef") {
                    let sign = text_of(c, "sign").unwrap_or_default();
                    if sign == "TAB" {
                        s.has_tablature = true;
                    }
                    let label = match (sign.as_str(), text_of(c, "line")) {
                        ("TAB", _) | ("percussion", _) => sign.clone(),
                        (_, Some(l)) => format!("{sign}{l}"),
                        _ => sign.clone(),
                    };
                    if !ps.clefs.contains(&label) {
                        ps.clefs.push(label);
                    }
                }
            }
        }
        ps.staves = staves.min(255) as u8;
        s.measures = s.measures.max(ps.measures);
        s.note_count += ps.note_count;
        s.parts.push(ps);
    }
    Ok(s)
}

impl Summary {
    pub fn describe(&self) -> String {
        let mut out = String::new();
        let n = self.parts.len();
        out.push_str(&format!("{n} part{}", if n == 1 { "" } else { "s" }));
        if n > 0 {
            let parts: Vec<String> = self
                .parts
                .iter()
                .map(|p| {
                    let mut d = format!(
                        "{}: {} {}",
                        p.name,
                        p.staves,
                        if p.staves == 1 { "staff" } else { "staves" }
                    );
                    if !p.clefs.is_empty() {
                        d.push_str(&format!(", {} clefs", p.clefs.join("/")));
                    }
                    d.push_str(&format!("; {} measures", p.measures));
                    d
                })
                .collect();
            out.push_str(&format!(" ({})", parts.join(" | ")));
        }
        if let Some(k) = &self.key {
            out.push_str(&format!(", key {}", k.name()));
        }
        if let Some(t) = &self.time {
            out.push_str(&format!(", {t}"));
        }
        out.push_str(&format!(", {} notes", self.note_count));
        if self.has_tablature {
            out.push_str(", has tablature");
        }
        out
    }
}

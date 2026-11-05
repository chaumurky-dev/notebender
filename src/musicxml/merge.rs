//! Merge per-page score-partwise documents into one.

use super::{join, parse_doc, tokenize, Kind};
use anyhow::{bail, Context};
use roxmltree::Node;
use std::collections::HashMap;
use std::ops::Range;

/// Whitespace-insensitive canonical form of an element (for equality checks).
fn canon(n: Node) -> String {
    let mut s = format!("<{}", n.tag_name().name());
    let mut attrs: Vec<_> = n.attributes().map(|a| (a.name(), a.value())).collect();
    attrs.sort();
    for (k, v) in attrs {
        s.push_str(&format!(" {k}={v:?}"));
    }
    s.push('>');
    for c in n.children() {
        if c.is_element() {
            s.push_str(&canon(c));
        } else if let Some(t) = c.text() {
            s.push_str(t.trim());
        }
    }
    s.push_str("</>");
    s
}

type State = HashMap<String, String>;

fn state_key(n: Node) -> Option<String> {
    let name = n.tag_name().name();
    match name {
        "divisions" => Some(name.to_string()),
        "key" | "time" | "clef" => Some(format!("{name}#{}", n.attribute("number").unwrap_or("1"))),
        _ => None,
    }
}

fn update_state(state: &mut State, measure: Node) {
    for a in measure.children().filter(|c| c.has_tag_name("attributes")) {
        for c in a.children().filter(|c| c.is_element()) {
            if let Some(k) = state_key(c) {
                state.insert(k, canon(c));
            }
        }
    }
}

/// Range to delete for an element: the element plus preceding whitespace-only text.
fn removal_range(n: Node) -> Range<usize> {
    let r = n.range();
    let mut start = r.start;
    if let Some(p) = n.prev_sibling() {
        if p.is_text() && p.text().is_some_and(|t| t.trim().is_empty()) {
            start = p.range().start;
        }
    }
    start..r.end
}

fn slice_without(xml: &str, range: Range<usize>, dels: &[Range<usize>]) -> String {
    let mut out = String::new();
    let mut pos = range.start;
    for d in dels {
        if d.start >= range.start && d.end <= range.end && d.start >= pos {
            out.push_str(&xml[pos..d.start]);
            pos = d.end;
        }
    }
    out.push_str(&xml[pos..range.end]);
    out
}

fn set_number(raw: &str, num: usize) -> String {
    let marker = " number=";
    if let Some(i) = raw.find(marker) {
        let q = i + marker.len();
        if let Some(quote) = raw[q..].chars().next().filter(|c| *c == '"' || *c == '\'') {
            if let Some(len) = raw[q + 1..].find(quote) {
                return format!("{}{quote}{num}{quote}{}", &raw[..q], &raw[q + 2 + len..]);
            }
        }
    }
    let at = "<measure".len();
    format!("{} number=\"{num}\"{}", &raw[..at], &raw[at..])
}

pub fn merge_pages(pages: &[String]) -> anyhow::Result<(String, Vec<String>)> {
    if pages.is_empty() {
        bail!("no pages to merge");
    }
    let mut warnings = Vec::new();
    let docs: Vec<_> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| parse_doc(p).with_context(|| format!("page {} is not well-formed XML", i + 1)))
        .collect::<Result<_, _>>()?;
    for (i, d) in docs.iter().enumerate() {
        let r = d.root_element().tag_name().name();
        if r != "score-partwise" {
            bail!("page {} has root <{r}>, expected <score-partwise>", i + 1);
        }
    }
    let base_xml = pages[0].as_str();
    let base_parts: Vec<Node> = docs[0]
        .root_element()
        .children()
        .filter(|c| c.has_tag_name("part"))
        .collect();
    if base_parts.is_empty() {
        bail!("page 1 has no <part> elements");
    }
    let base_ids: Vec<&str> = base_parts.iter().map(|p| p.attribute("id").unwrap_or("")).collect();

    // Running attribute state per base part, from page 1.
    let mut states: Vec<State> = base_parts
        .iter()
        .map(|p| {
            let mut s = State::new();
            for m in p.children().filter(|c| c.has_tag_name("measure")) {
                update_state(&mut s, m);
            }
            s
        })
        .collect();
    // Text to append per base part.
    let mut appended: Vec<String> = vec![String::new(); base_parts.len()];

    for (pi, doc) in docs.iter().enumerate().skip(1) {
        let page = pi + 1;
        let xml = pages[pi].as_str();
        let parts: Vec<Node> = doc
            .root_element()
            .children()
            .filter(|c| c.has_tag_name("part"))
            .collect();
        let ids: Vec<&str> = parts.iter().map(|p| p.attribute("id").unwrap_or("")).collect();
        let same_ids = ids.iter().all(|i| base_ids.contains(i));
        let positional = !same_ids && parts.len() == base_parts.len();
        // mapping: base part index -> page part
        let mut mapping: Vec<Option<Node>> = vec![None; base_parts.len()];
        if positional {
            for (bi, p) in parts.iter().enumerate() {
                if ids[bi] != base_ids[bi] {
                    warnings.push(format!(
                        "page {page}: part id {:?} is unknown; mapped by position to {:?}",
                        ids[bi], base_ids[bi]
                    ));
                }
                mapping[bi] = Some(*p);
            }
        } else {
            for (bi, id) in base_ids.iter().enumerate() {
                mapping[bi] = parts.iter().find(|p| p.attribute("id") == Some(id)).copied();
                if mapping[bi].is_none() {
                    warnings.push(format!("page {page}: part {id:?} is missing; skipped"));
                }
            }
            for (p, id) in parts.iter().zip(&ids) {
                if !base_ids.contains(id) {
                    let _ = p;
                    warnings.push(format!("page {page}: unknown part {id:?} ignored"));
                }
            }
        }
        for (bi, mp) in mapping.iter().enumerate() {
            let Some(part) = mp else { continue };
            let measures: Vec<Node> = part.children().filter(|c| c.has_tag_name("measure")).collect();
            // Redundant leading attributes in the first measure.
            let mut dels: Vec<Range<usize>> = Vec::new();
            if let Some(first) = measures.first() {
                if let Some(attrs) = first.children().find(|c| c.has_tag_name("attributes")) {
                    let mut removed = 0;
                    let elems: Vec<Node> = attrs.children().filter(|c| c.is_element()).collect();
                    for c in &elems {
                        if let Some(k) = state_key(*c) {
                            if states[bi].get(&k) == Some(&canon(*c)) {
                                dels.push(removal_range(*c));
                                removed += 1;
                            }
                        }
                    }
                    if removed == elems.len() && removed > 0 {
                        dels.clear();
                        dels.push(removal_range(attrs));
                    }
                }
            }
            for m in &measures {
                update_state(&mut states[bi], *m);
                appended[bi].push_str(&slice_without(xml, m.range(), &dels));
                appended[bi].push('\u{0}');
            }
        }
    }

    // Insert appended measures after each base part's last measure.
    let mut inserts: Vec<(usize, String)> = Vec::new();
    for (bi, part) in base_parts.iter().enumerate() {
        if appended[bi].is_empty() {
            continue;
        }
        let Some(last) = part.children().rfind(|c| c.has_tag_name("measure")) else {
            continue;
        };
        let ws = match last.prev_sibling() {
            Some(p) if p.is_text() && p.text().is_some_and(|t| t.trim().is_empty()) => p.text().unwrap(),
            _ => "\n",
        };
        let text: String = appended[bi]
            .split('\u{0}')
            .filter(|s| !s.is_empty())
            .map(|m| format!("{ws}{m}"))
            .collect();
        inserts.push((last.range().end, text));
    }
    inserts.sort_by_key(|(p, _)| std::cmp::Reverse(*p));
    let mut merged = base_xml.to_string();
    for (pos, text) in inserts {
        merged.insert_str(pos, &text);
    }

    // Renumber.
    let mut toks = tokenize(&merged)?;
    let mut counter = 0usize;
    for t in toks.iter_mut() {
        match &t.kind {
            Kind::Start(n) if n == "part" => counter = 0,
            Kind::Start(n) | Kind::Empty(n) if n == "measure" => {
                let implicit = t.raw.contains("implicit=\"yes\"") || t.raw.contains("implicit='yes'");
                if !implicit {
                    counter += 1;
                    t.raw = set_number(&t.raw, counter);
                }
            }
            _ => {}
        }
    }
    let out = join(&toks);
    parse_doc(&out).context("merged document is not well-formed")?;
    Ok((out, warnings))
}

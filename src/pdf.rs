//! Input handling: load a PDF or MusicXML and prepare what the OMR agent needs.

use crate::tools::Toolbox;
use anyhow::{Context, Result};
use base64::Engine;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Pdf,
    MusicXml,
}

pub fn classify(path: &Path) -> Result<InputKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => Ok(InputKind::Pdf),
        "musicxml" | "xml" => Ok(InputKind::MusicXml),
        "mxl" => anyhow::bail!("compressed .mxl is not supported yet; unzip it to .musicxml first"),
        _ => {
            // Sniff.
            let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            if bytes.starts_with(b"%PDF") {
                Ok(InputKind::Pdf)
            } else if bytes.iter().take(512).any(|&b| b == b'<') {
                Ok(InputKind::MusicXml)
            } else {
                anyhow::bail!("cannot tell whether {} is a PDF or MusicXML", path.display())
            }
        }
    }
}

/// Count pages: pdfinfo if present, else a crude scan for `/Type /Page` objects.
pub async fn page_count(tools: &Toolbox, pdf: &Path) -> Result<u32> {
    if let Some(n) = tools.pdf_page_count(pdf).await {
        return Ok(n);
    }
    let bytes = std::fs::read(pdf)?;
    let text = String::from_utf8_lossy(&bytes);
    let n = text.matches("/Type /Page").count() - text.matches("/Type /Pages").count();
    Ok(n.max(1) as u32)
}

/// Pages ready to send to the model: either rasterised PNGs (preferred, one request per page)
/// or the whole PDF as a single document block.
#[derive(Debug, Clone)]
pub enum ScoreImages {
    Pages(Vec<PathBuf>),
    WholePdf(PathBuf),
}

pub async fn prepare(tools: &Toolbox, pdf: &Path, workdir: &Path, dpi: u32) -> Result<ScoreImages> {
    if tools.pdftoppm.available() {
        let dir = workdir.join("pages");
        std::fs::create_dir_all(&dir)?;
        let pages = tools.pdf_to_pngs(pdf, &dir, dpi).await?;
        Ok(ScoreImages::Pages(pages))
    } else {
        Ok(ScoreImages::WholePdf(pdf.to_path_buf()))
    }
}

pub fn base64_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

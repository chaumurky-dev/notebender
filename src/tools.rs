//! Optional external programs. Everything here degrades gracefully: if a tool is missing the
//! pipeline either falls back to a model-backed step or skips the artifact with a warning.

use crate::config::ToolsConfig;
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub name: &'static str,
    pub command: String,
    pub path: Option<PathBuf>,
    pub purpose: &'static str,
    pub install_hint: &'static str,
}

impl ToolStatus {
    pub fn available(&self) -> bool {
        self.path.is_some()
    }
}

/// Resolved locations of external tools.
#[derive(Debug, Clone)]
pub struct Toolbox {
    pub lilypond: ToolStatus,
    pub musicxml2ly: ToolStatus,
    pub pdftoppm: ToolStatus,
    pub pdfinfo: ToolStatus,
    pub verovio: ToolStatus,
    pub musescore: ToolStatus,
}

fn probe(name: &'static str, command: &str, purpose: &'static str, install_hint: &'static str) -> ToolStatus {
    let path = which::which(command).ok();
    ToolStatus {
        name,
        command: command.to_string(),
        path,
        purpose,
        install_hint,
    }
}

impl Toolbox {
    pub fn detect(cfg: &ToolsConfig) -> Self {
        Toolbox {
            lilypond: probe(
                "lilypond",
                &cfg.lilypond,
                "engrave LilyPond → PDF",
                "brew install lilypond  (or https://lilypond.org/download.html)",
            ),
            musicxml2ly: probe(
                "musicxml2ly",
                &cfg.musicxml2ly,
                "deterministic MusicXML → LilyPond (ships with LilyPond)",
                "brew install lilypond",
            ),
            pdftoppm: probe(
                "pdftoppm",
                &cfg.pdftoppm,
                "rasterise PDF pages for page-by-page OMR",
                "brew install poppler",
            ),
            pdfinfo: probe(
                "pdfinfo",
                &cfg.pdfinfo,
                "count PDF pages (ships with poppler)",
                "brew install poppler",
            ),
            verovio: probe(
                "verovio",
                &cfg.verovio,
                "optional: MusicXML → Humdrum/SVG without a model",
                "brew install verovio",
            ),
            musescore: probe(
                "mscore",
                &cfg.musescore,
                "optional: MusicXML → PDF via MuseScore",
                "https://musescore.org/download",
            ),
        }
    }

    pub fn all(&self) -> Vec<&ToolStatus> {
        vec![
            &self.lilypond,
            &self.musicxml2ly,
            &self.pdftoppm,
            &self.pdfinfo,
            &self.verovio,
            &self.musescore,
        ]
    }

    /// Convert MusicXML to LilyPond with `musicxml2ly`. Returns the `.ly` source.
    pub async fn musicxml_to_lilypond(&self, musicxml_path: &Path, workdir: &Path) -> Result<String> {
        let exe = self.musicxml2ly.path.clone().context("musicxml2ly is not installed")?;
        let out = workdir.join("converted.ly");
        let status = Command::new(exe)
            .arg("--output")
            .arg(&out)
            .arg(musicxml_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("running musicxml2ly")?;
        if !status.status.success() {
            anyhow::bail!("musicxml2ly failed: {}", String::from_utf8_lossy(&status.stderr).trim());
        }
        Ok(tokio::fs::read_to_string(&out).await?)
    }

    /// Engrave a `.ly` file to PDF with LilyPond. Returns the PDF path.
    pub async fn lilypond_to_pdf(&self, ly_path: &Path, out_dir: &Path, stem: &str) -> Result<PathBuf> {
        let exe = self.lilypond.path.clone().context("lilypond is not installed")?;
        let output = Command::new(exe)
            .arg("--pdf")
            .arg("--loglevel=ERROR")
            .arg("-o")
            .arg(out_dir.join(stem))
            .arg(ly_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("running lilypond")?;
        let pdf = out_dir.join(format!("{stem}.pdf"));
        if !output.status.success() || !pdf.exists() {
            anyhow::bail!("lilypond failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(pdf)
    }

    /// Rasterise every page of a PDF to PNG files (`<prefix>-N.png`), returned in page order.
    pub async fn pdf_to_pngs(&self, pdf: &Path, out_dir: &Path, dpi: u32) -> Result<Vec<PathBuf>> {
        let exe = self.pdftoppm.path.clone().context("pdftoppm is not installed")?;
        let prefix = out_dir.join("page");
        let output = Command::new(exe)
            .arg("-png")
            .arg("-r")
            .arg(dpi.to_string())
            .arg(pdf)
            .arg(&prefix)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("running pdftoppm")?;
        if !output.status.success() {
            anyhow::bail!("pdftoppm failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        let mut pages: Vec<(u32, PathBuf)> = Vec::new();
        for entry in std::fs::read_dir(out_dir)? {
            let p = entry?.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(num) = name.strip_prefix("page-").and_then(|r| r.strip_suffix(".png")) {
                if let Ok(n) = num.parse::<u32>() {
                    pages.push((n, p));
                }
            }
        }
        pages.sort();
        if pages.is_empty() {
            anyhow::bail!("pdftoppm produced no pages");
        }
        Ok(pages.into_iter().map(|(_, p)| p).collect())
    }

    /// Page count via `pdfinfo`, if available.
    pub async fn pdf_page_count(&self, pdf: &Path) -> Option<u32> {
        let exe = self.pdfinfo.path.clone()?;
        let output = Command::new(exe).arg(pdf).output().await.ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .find_map(|l| l.strip_prefix("Pages:"))
            .and_then(|v| v.trim().parse().ok())
    }
}

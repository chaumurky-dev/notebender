//! Configuration: a TOML file plus environment variables.
//!
//! Lookup order for the config file: `--config PATH`, `$NOTEBENDER_CONFIG`, `./notebender.toml`,
//! then `$XDG_CONFIG_HOME/notebender/config.toml` (macOS: `~/Library/Application Support/notebender/config.toml`).
//! Missing file ⇒ built-in defaults. The API key is **never** stored in the file: the file names
//! the environment variable that holds it (default `ANTHROPIC_API_KEY`).

use crate::models::{Effort, Price, Tier};
use crate::router::TaskKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub api: ApiConfig,
    /// Tier → concrete model id.
    pub models: BTreeMap<Tier, String>,
    /// Tier → list price used for cost estimates.
    pub prices: BTreeMap<Tier, Price>,
    /// Task → tier. Anything omitted uses [`TaskKind::default_tier`].
    pub routing: BTreeMap<TaskKind, Tier>,
    /// Task → effort. Anything omitted uses [`TaskKind::default_effort`].
    pub effort: BTreeMap<TaskKind, Effort>,
    pub tools: ToolsConfig,
    pub limits: LimitsConfig,
    pub server: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiConfig {
    /// Name of the environment variable holding the API key.
    pub key_env: String,
    pub base_url: String,
    /// Retries on 429/5xx/overloaded/connection errors.
    pub max_retries: u32,
    /// Per-request timeout in seconds (streaming keeps the connection alive, so this is generous).
    pub timeout_secs: u64,
    /// Opt into server-side `fallbacks: "default"` on models that support it, so a safety
    /// classifier decline is re-run on Anthropic's recommended fallback model instead of failing.
    pub fallbacks: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            key_env: "ANTHROPIC_API_KEY".into(),
            base_url: DEFAULT_BASE_URL.into(),
            max_retries: 3,
            timeout_secs: 1800,
            fallbacks: true,
        }
    }
}

/// Paths (or bare command names resolved on `$PATH`) of optional external tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsConfig {
    pub lilypond: String,
    pub musicxml2ly: String,
    pub pdftoppm: String,
    pub pdfinfo: String,
    pub verovio: String,
    pub musescore: String,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        ToolsConfig {
            lilypond: "lilypond".into(),
            musicxml2ly: "musicxml2ly".into(),
            pdftoppm: "pdftoppm".into(),
            pdfinfo: "pdfinfo".into(),
            verovio: "verovio".into(),
            musescore: "mscore".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// Refuse PDFs longer than this (each page is a model call).
    pub max_pdf_pages: u32,
    /// Upload size cap for the web UI, in megabytes.
    pub max_upload_mb: u64,
    /// Rasterisation DPI for page images sent to the OMR model.
    pub page_dpi: u32,
    /// How many OMR page requests to run concurrently.
    pub omr_concurrency: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        LimitsConfig {
            max_pdf_pages: 40,
            max_upload_mb: 32,
            page_dpi: 150,
            omr_concurrency: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Where jobs (uploads + outputs) are stored. Defaults to the platform data dir.
    pub data_dir: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 8787,
            data_dir: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            api: ApiConfig::default(),
            models: Tier::ALL
                .iter()
                .map(|&t| (t, t.default_model_id().to_string()))
                .collect(),
            prices: Tier::ALL.iter().map(|&t| (t, t.default_price())).collect(),
            routing: TaskKind::ALL.iter().map(|&k| (k, k.default_tier())).collect(),
            effort: TaskKind::ALL.iter().map(|&k| (k, k.default_effort())).collect(),
            tools: ToolsConfig::default(),
            limits: LimitsConfig::default(),
            server: ServerConfig::default(),
        }
    }
}

impl Config {
    /// Load from an explicit path, or search the standard locations. A missing file is not an error.
    pub fn load(explicit: Option<&Path>) -> Result<(Config, Option<PathBuf>)> {
        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => Self::search_paths().into_iter().find(|p| p.is_file()),
        };
        match path {
            Some(p) => {
                let text = std::fs::read_to_string(&p).with_context(|| format!("reading config {}", p.display()))?;
                let cfg = Self::from_toml(&text).with_context(|| format!("parsing config {}", p.display()))?;
                Ok((cfg, Some(p)))
            }
            None => Ok((Config::default(), None)),
        }
    }

    pub fn from_toml(text: &str) -> Result<Config> {
        let mut cfg: Config = toml::from_str(text)?;
        // Fill any tier/task the user omitted with defaults so lookups never miss.
        for t in Tier::ALL {
            cfg.models.entry(t).or_insert_with(|| t.default_model_id().to_string());
            cfg.prices.entry(t).or_insert_with(|| t.default_price());
        }
        for k in TaskKind::ALL {
            cfg.routing.entry(k).or_insert_with(|| k.default_tier());
            cfg.effort.entry(k).or_insert_with(|| k.default_effort());
        }
        Ok(cfg)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config serialises")
    }

    pub fn search_paths() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Ok(p) = std::env::var("NOTEBENDER_CONFIG") {
            if !p.is_empty() {
                v.push(PathBuf::from(p));
            }
        }
        v.push(PathBuf::from("notebender.toml"));
        if let Some(dir) = dirs::config_dir() {
            v.push(dir.join("notebender").join("config.toml"));
        }
        v
    }

    pub fn model_id(&self, tier: Tier) -> &str {
        self.models
            .get(&tier)
            .map(String::as_str)
            .unwrap_or_else(|| tier.default_model_id())
    }

    pub fn price(&self, tier: Tier) -> Price {
        self.prices.get(&tier).copied().unwrap_or_else(|| tier.default_price())
    }

    /// Read the API key from the configured environment variable.
    pub fn api_key(&self) -> Result<String> {
        match std::env::var(&self.api.key_env) {
            Ok(k) if !k.trim().is_empty() => Ok(k.trim().to_string()),
            _ => anyhow::bail!(
                "no API key: set the {} environment variable (see README → Configuration)",
                self.api.key_env
            ),
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        if let Some(d) = &self.server.data_dir {
            return d.clone();
        }
        if let Ok(d) = std::env::var("NOTEBENDER_DATA_DIR") {
            if !d.is_empty() {
                return PathBuf::from(d);
            }
        }
        dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("notebender")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml();
        let back = Config::from_toml(&text).unwrap();
        assert_eq!(back.model_id(Tier::Opus), "claude-opus-5-5");
        assert_eq!(back.routing[&TaskKind::Omr], Tier::Opus);
    }

    #[test]
    fn partial_file_is_filled_with_defaults() {
        let cfg = Config::from_toml("[routing]\nomr = \"fable\"\n").unwrap();
        assert_eq!(cfg.routing[&TaskKind::Omr], Tier::Fable);
        assert_eq!(cfg.routing[&TaskKind::Plan], Tier::Haiku);
        assert_eq!(cfg.model_id(Tier::Haiku), "claude-haiku-4-5");
        assert_eq!(cfg.api.key_env, "ANTHROPIC_API_KEY");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::from_toml("[api]\nkey = \"sk-ant-literal\"\n").is_err());
    }
}

/// Commented example written by `notebender config init`.
pub const EXAMPLE_TOML: &str = r#"# notebender configuration.
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
tablature = "haiku"
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
"#;

#[cfg(test)]
mod example_tests {
    use super::*;

    #[test]
    fn example_config_parses_and_matches_defaults() {
        let cfg = Config::from_toml(EXAMPLE_TOML).unwrap();
        let def = Config::default();
        assert_eq!(cfg.to_toml(), def.to_toml());
    }
}

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::transfer;

const APP_DIR: &str = "piclecor";
const CONFIG_FILE: &str = "config.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Destination {
    pub name: String,
    /// Local path or rsync remote form `[user@]host:/path`.
    pub target: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Theme: System",
            Self::Light => "Theme: Light",
            Self::Dark => "Theme: Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    /// Camera cards write files at capture, so newest first shows the latest shoot.
    #[default]
    NewestFirst,
    OldestFirst,
    Name,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::NewestFirst => Self::OldestFirst,
            Self::OldestFirst => Self::Name,
            Self::Name => Self::NewestFirst,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NewestFirst => "Sort: Newest first",
            Self::OldestFirst => "Sort: Oldest first",
            Self::Name => "Sort: Name",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub destinations: Vec<Destination>,
    pub last_folder: Option<PathBuf>,
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub sort: SortOrder,
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join(APP_DIR).join(CONFIG_FILE))
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

impl Config {
    pub fn load() -> Self {
        let mut config: Self = config_path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();

        // Older entries may hold shell escapes typed by hand.
        for d in &mut config.destinations {
            d.target = transfer::unquote(&d.target);
        }
        config
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = config_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

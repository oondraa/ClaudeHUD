//! User preferences, stored as JSON in %APPDATA%\ClaudeHUD\settings.json.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Collapse {
    S5,
    S8,
    S15,
    Never,
}

impl Collapse {
    pub const ALL: [Collapse; 4] = [Collapse::S5, Collapse::S8, Collapse::S15, Collapse::Never];

    pub fn duration(self) -> Option<Duration> {
        match self {
            Collapse::S5 => Some(Duration::from_secs(5)),
            Collapse::S8 => Some(Duration::from_secs(8)),
            Collapse::S15 => Some(Duration::from_secs(15)),
            Collapse::Never => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Collapse::S5 => "5 s",
            Collapse::S8 => "8 s",
            Collapse::S15 => "15 s",
            Collapse::Never => "Nikdy",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Sound {
    /// ClaudeHUD's own quiet chime.
    Soft,
    /// The Windows notification sound.
    System,
    Off,
}

impl Sound {
    pub fn label(self) -> &'static str {
        match self {
            Sound::Soft => "Jemný tón",
            Sound::System => "Systémový",
            Sound::Off => "Vypnuto",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Sound::Soft => Sound::System,
            Sound::System => Sound::Off,
            Sound::Off => Sound::Soft,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The resting island lets clicks through unless Alt is held.
    pub click_through: bool,
    pub dim_while_typing: bool,
    pub hide_fullscreen: bool,
    pub collapse: Collapse,
    pub notify_done: bool,
    pub notify_waiting: bool,
    pub notify_error: bool,
    /// Show each tool step in the panel instead of the prompt.
    pub tool_steps: bool,
    pub sound: Sound,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            click_through: true,
            dim_while_typing: true,
            hide_fullscreen: true,
            collapse: Collapse::S8,
            notify_done: true,
            notify_waiting: true,
            notify_error: true,
            tool_steps: false,
            sound: Sound::Soft,
        }
    }
}

fn path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    base.join("ClaudeHUD").join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

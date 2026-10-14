use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub snlm0e: String,
    pub f_sid: String,
    pub bl: String,
    pub cookie: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub response_id: Option<String>,
    #[serde(default)]
    pub choice_id: Option<String>,
}

fn default_model() -> String {
    "Argon4p0".to_string()
}

impl SessionData {
    pub fn config_path() -> PathBuf {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".config/dragoman/session.json")
        } else {
            PathBuf::from("session.json")
        }
    }

    pub fn load() -> Result<Self, String> {
        let path = Self::config_path();
        if !path.exists() {
            return Err(format!(
                "Session file not found at {}. Run 'dragoman login' to authenticate with your Google account first.",
                path.display()
            ));
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read session file: {}", e))?;

        let session: SessionData = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse session file: {}", e))?;

        Ok(session)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize session: {}", e))?;

        std::fs::write(&path, json).map_err(|e| format!("Failed to write session file: {}", e))?;

        Ok(())
    }
}

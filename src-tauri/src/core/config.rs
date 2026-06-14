use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde::Deserialize;

use super::domain::{AgentConfig, ConfigStatus};
use super::errors::{AppError, AppResult};

impl AgentConfig {
    pub fn load() -> AppResult<Self> {
        let path = config_path()?;
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Err(AppError::ConfigNotConfigured),
        };

        let _ = tighten_permissions(&path);

        #[derive(Deserialize)]
        struct Raw {
            api_key: String,
            #[serde(default)]
            model: Option<String>,
            #[serde(default)]
            base_url: Option<String>,
        }
        let raw: Raw = toml::from_str(&content).map_err(|e| AppError::Config(e.to_string()))?;

        if raw.api_key.trim().is_empty() {
            return Err(AppError::ConfigNotConfigured);
        }

        Ok(AgentConfig {
            api_key: raw.api_key,
            model: raw.model.unwrap_or_else(|| "glm-4.6".to_string()),
            base_url: raw.base_url
                .unwrap_or_else(|| "https://open.bigmodel.cn/api/paas/v4".to_string()),
        })
    }

    pub fn status() -> ConfigStatus {
        match Self::load() {
            Ok(c) => ConfigStatus { configured: true, model: c.model },
            Err(_) => ConfigStatus { configured: false, model: "(未配置)".to_string() },
        }
    }
}

fn config_path() -> AppResult<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| AppError::Config("no HOME directory".into()))?;
    Ok(home.join(".config/sophoni/config.toml"))
}

fn tighten_permissions(path: &PathBuf) -> AppResult<()> {
    let mut perms = fs::metadata(path)?.permissions();
    if perms.mode() & 0o077 != 0 {
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

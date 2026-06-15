use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use serde::Deserialize;

use super::domain::{AgentConfig, ConfigStatus};
use super::errors::{AppError, AppResult};

impl AgentConfig {
    pub fn load() -> AppResult<(Self, String)> {
        let path = config_path()?;
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Err(AppError::ConfigNotConfigured),
        };

        let _ = tighten_permissions(&path);

        // 先尝试多 Provider 格式
        if let Some((config, provider)) = try_parse_multi_provider(&content)? {
            return Ok((config, provider));
        }

        // 回退到旧格式（平铺）
        let config = try_parse_legacy(&content)?;
        Ok((config, "glm".to_string()))
    }

    pub fn status() -> ConfigStatus {
        match Self::load() {
            Ok((c, provider)) => ConfigStatus {
                configured: true,
                provider,
                model: c.model,
            },
            Err(_) => ConfigStatus {
                configured: false,
                provider: "(未配置)".to_string(),
                model: "(未配置)".to_string(),
            },
        }
    }
}

#[derive(Deserialize)]
struct ProviderEntry {
    api_key: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
}

#[derive(Deserialize)]
struct MultiProviderConfig {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    glm: Option<ProviderEntry>,
    #[serde(default)]
    minimax: Option<ProviderEntry>,
}

struct ProviderDefaults {
    model: String,
    base_url: String,
}

fn provider_defaults(name: &str) -> ProviderDefaults {
    match name {
        "glm" => ProviderDefaults {
            model: "glm-4.6".to_string(),
            base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
        },
        "minimax" => ProviderDefaults {
            model: "MiniMax-M3".to_string(),
            base_url: "https://api.minimax.io/v1".to_string(),
        },
        _ => ProviderDefaults {
            model: "unknown".to_string(),
            base_url: String::new(),
        },
    }
}

fn try_parse_multi_provider(content: &str) -> AppResult<Option<(AgentConfig, String)>> {
    let multi: MultiProviderConfig = match toml::from_str(content) {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };

    let active = match multi.active {
        Some(a) => a,
        None => return Ok(None),
    };

    let entry = match active.as_str() {
        "glm" => multi.glm.ok_or_else(|| AppError::Config("glm 段缺失".into()))?,
        "minimax" => multi
            .minimax
            .ok_or_else(|| AppError::Config("minimax 段缺失".into()))?,
        other => return Err(AppError::Config(format!("未知 provider: {other}"))),
    };

    if entry.api_key.trim().is_empty() {
        return Err(AppError::ConfigNotConfigured);
    }

    let defaults = provider_defaults(&active);
    Ok(Some((
        AgentConfig {
            api_key: entry.api_key,
            model: entry.model.unwrap_or(defaults.model),
            base_url: entry.base_url.unwrap_or(defaults.base_url),
        },
        active,
    )))
}

fn try_parse_legacy(content: &str) -> AppResult<AgentConfig> {
    #[derive(Deserialize)]
    struct LegacyConfig {
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        base_url: Option<String>,
    }

    let legacy: LegacyConfig =
        toml::from_str(content).map_err(|e| AppError::Config(e.to_string()))?;

    let api_key = legacy
        .api_key
        .filter(|k| !k.trim().is_empty())
        .ok_or(AppError::ConfigNotConfigured)?;

    Ok(AgentConfig {
        api_key,
        model: legacy.model.unwrap_or_else(|| "glm-4.6".to_string()),
        base_url: legacy
            .base_url
            .unwrap_or_else(|| "https://open.bigmodel.cn/api/paas/v4".to_string()),
    })
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

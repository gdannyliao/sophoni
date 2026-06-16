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
            risk_level: parse_risk_level(&content),
            workspace_path: parse_workspace_path(&content),
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
        risk_level: parse_risk_level(content),
        workspace_path: parse_workspace_path(content),
    })
}

fn parse_workspace_path(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("workspace_path") {
            let val = val.trim().trim_start_matches('=').trim().trim_matches('"');
            if val.is_empty() {
                return None;
            }
            return Some(val.to_string());
        }
    }
    None
}

pub fn save_workspace_path(path: &str) -> AppResult<()> {
    let config_path = config_path()?;
    let content = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut found = false;
    for line in lines.iter_mut() {
        if line.trim_start().starts_with("workspace_path") {
            *line = format!("workspace_path = \"{path}\"");
            found = true;
            break;
        }
    }
    if !found {
        lines.push(format!("workspace_path = \"{path}\""));
    }

    let new_content = lines.join("\n") + "\n";

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config_path, new_content)?;
    let _ = tighten_permissions(&config_path);
    Ok(())
}

fn parse_risk_level(content: &str) -> super::command_risk::RiskLevel {
    use super::command_risk::RiskLevel;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("risk_level") {
            let val = val.trim().trim_start_matches('=').trim().trim_matches('"');
            return match val {
                "relaxed" => RiskLevel::Relaxed,
                "unrestricted" => RiskLevel::Unrestricted,
                _ => RiskLevel::Standard,
            };
        }
    }
    RiskLevel::Standard
}

pub fn save_risk_level(level: super::command_risk::RiskLevel) -> AppResult<()> {
    let path = config_path()?;
    let content = if path.exists() {
        fs::read_to_string(&path)?
    } else {
        String::new()
    };

    let level_str = match level {
        super::command_risk::RiskLevel::Standard => "standard",
        super::command_risk::RiskLevel::Relaxed => "relaxed",
        super::command_risk::RiskLevel::Unrestricted => "unrestricted",
    };

    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    let mut found = false;
    for line in lines.iter_mut() {
        if line.trim_start().starts_with("risk_level") {
            *line = format!("risk_level = \"{level_str}\"");
            found = true;
            break;
        }
    }
    if !found {
        lines.push(format!("risk_level = \"{level_str}\""));
    }

    let new_content = lines.join("\n") + "\n";

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, new_content)?;
    let _ = tighten_permissions(&path);
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::super::errors::AppError;
    use super::AgentConfig;
    use std::path::Path;
    use std::sync::{Mutex, OnceLock};

    static HOME_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct HomeEnvGuard {
        original: Option<String>,
    }

    impl HomeEnvGuard {
        fn set(home: &Path) -> Self {
            let original = std::env::var("HOME").ok();
            std::env::set_var("HOME", home);
            Self { original }
        }
    }

    impl Drop for HomeEnvGuard {
        fn drop(&mut self) {
            if let Some(h) = &self.original {
                std::env::set_var("HOME", h);
            } else {
                std::env::remove_var("HOME");
            }
        }
    }

    fn with_home_dir<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = HOME_ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = HomeEnvGuard::set(home);

        f()
    }

    #[test]
    fn config_returns_not_configured_when_file_missing() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();

        let result = with_home_dir(&temp, AgentConfig::load);

        let _ = std::fs::remove_dir_all(&temp);

        assert!(matches!(result, Err(AppError::ConfigNotConfigured)));
    }

    #[test]
    fn config_loads_api_key_model_base_url() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        let config_dir = temp.join(".config/sophoni");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "api_key = \"sk-test\"\nmodel = \"glm-4.6\"\nbase_url = \"https://example.com\"\n",
        )
        .unwrap();

        let (cfg, provider) = with_home_dir(&temp, || AgentConfig::load().unwrap());

        let _ = std::fs::remove_dir_all(&temp);

        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.model, "glm-4.6");
        assert_eq!(cfg.base_url, "https://example.com");
        assert_eq!(provider, "glm");
    }

    #[test]
    fn config_applies_defaults_for_optional_fields() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        let config_dir = temp.join(".config/sophoni");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), "api_key = \"sk-only\"\n").unwrap();

        let (cfg, provider) = with_home_dir(&temp, || AgentConfig::load().unwrap());

        let _ = std::fs::remove_dir_all(&temp);

        assert_eq!(cfg.api_key, "sk-only");
        assert_eq!(cfg.model, "glm-4.6");
        assert_eq!(cfg.base_url, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(provider, "glm");
    }

    #[test]
    fn config_status_reports_unconfigured_when_missing() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();

        let status = with_home_dir(&temp, AgentConfig::status);

        let _ = std::fs::remove_dir_all(&temp);

        assert!(!status.configured);
    }

    #[test]
    fn config_multi_provider_active_glm() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        let config_dir = temp.join(".config/sophoni");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "active = \"glm\"\n[glm]\napi_key = \"sk-glm\"\n[minimax]\napi_key = \"sk-mm\"\nmodel = \"MiniMax-M3\"\n",
        ).unwrap();

        let (cfg, provider) = with_home_dir(&temp, || AgentConfig::load().unwrap());

        let _ = std::fs::remove_dir_all(&temp);

        assert_eq!(provider, "glm");
        assert_eq!(cfg.api_key, "sk-glm");
        assert_eq!(cfg.model, "glm-4.6");
    }

    #[test]
    fn config_multi_provider_active_minimax() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        let config_dir = temp.join(".config/sophoni");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "active = \"minimax\"\n[glm]\napi_key = \"sk-glm\"\n[minimax]\napi_key = \"sk-mm\"\nmodel = \"MiniMax-M3\"\n",
        ).unwrap();

        let (cfg, provider) = with_home_dir(&temp, || AgentConfig::load().unwrap());

        let _ = std::fs::remove_dir_all(&temp);

        assert_eq!(provider, "minimax");
        assert_eq!(cfg.api_key, "sk-mm");
        assert_eq!(cfg.model, "MiniMax-M3");
        assert_eq!(cfg.base_url, "https://api.minimax.io/v1");
    }

    #[test]
    fn config_multi_provider_unknown_active_is_error() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        let config_dir = temp.join(".config/sophoni");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "active = \"unknown\"\n[glm]\napi_key = \"sk\"\n",
        ).unwrap();

        let result = with_home_dir(&temp, AgentConfig::load);

        let _ = std::fs::remove_dir_all(&temp);

        assert!(result.is_err());
    }

    #[test]
    fn config_multi_provider_missing_section_is_error() {
        let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
        let config_dir = temp.join(".config/sophoni");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            "active = \"minimax\"\n[glm]\napi_key = \"sk\"\n",
        ).unwrap();

        let result = with_home_dir(&temp, AgentConfig::load);

        let _ = std::fs::remove_dir_all(&temp);

        assert!(result.is_err());
    }
}

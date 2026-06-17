//! 网络工具：web_search（搜索后端）+ web_fetch（网页抓取 + SSRF 防护）。

use std::net::IpAddr;

use super::errors::{AppError, AppResult};

/// 从配置文件 [search] 段反序列化。所有字段可选，组合判断用哪个后端。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SearchConfig {
    #[serde(default)]
    pub tavily_key: Option<String>,
    #[serde(default)]
    pub google_key: Option<String>,
    #[serde(default)]
    pub google_cx: Option<String>,
}

impl SearchConfig {
    /// 是否有任意一个可用的搜索后端
    pub fn has_backend(&self) -> bool {
        self.tavily_key
            .as_ref()
            .map(|k| !k.trim().is_empty())
            .unwrap_or(false)
            || (self
                .google_key
                .as_ref()
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false)
                && self
                    .google_cx
                    .as_ref()
                    .map(|c| !c.trim().is_empty())
                    .unwrap_or(false))
    }
}

pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
}

/// 格式化搜索结果为模型友好的文本。
/// 每条：[序号] 标题\n    URL\n    摘要。空结果返回占位符。
pub fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "（无搜索结果）".to_string();
    }
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{}] {}\n    {}\n    {}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// SSRF 防护：判断 IP 是否为内网/保留地址，应拒绝抓取。
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// 校验 URL scheme 合法（只允许 http/https）。
pub fn validate_url_scheme(url: &str) -> AppResult<()> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(AppError::Tool(format!(
            "不支持的 URL（仅允许 http/https）: {url}"
        )))
    }
}

// ── 搜索后端（Task 5 实现）──

/// 搜索后端抽象。Tavily 和 Google CSE 各一实现。
#[async_trait::async_trait]
pub trait WebSearchBackend: Send + Sync {
    async fn search(
        &self,
        client: &reqwest::Client,
        query: &str,
        max_results: usize,
    ) -> AppResult<Vec<SearchResult>>;
}

/// 按配置选择后端：优先 Tavily，fallback Google CSE。
/// 返回 None 表示没有可用后端配置。
pub fn select_backend(_config: &SearchConfig) -> Option<Box<dyn WebSearchBackend>> {
    // Task 5 实现
    None
}

// ── web_fetch（Task 6 实现）──

/// 抓取 URL 并转为纯文本。
pub async fn web_fetch(
    _client: &reqwest::Client,
    _url: &str,
    _max_chars: usize,
) -> AppResult<String> {
    // Task 6 实现
    Err(AppError::Tool("web_fetch 尚未实现".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_private_ip_rejects_loopback_v4() {
        assert!(is_private_ip(&"127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rejects_10_subnet() {
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rejects_192_168_subnet() {
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rejects_172_16_subnet() {
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_allows_public() {
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rejects_loopback_v6() {
        assert!(is_private_ip(&"::1".parse().unwrap()));
    }

    #[test]
    fn is_private_ip_rejects_unspecified_v4() {
        assert!(is_private_ip(&"0.0.0.0".parse().unwrap()));
    }

    #[test]
    fn format_results_empty_returns_placeholder() {
        assert_eq!(format_results(&[]), "（无搜索结果）");
    }

    #[test]
    fn format_results_formats_with_index_title_url_snippet() {
        let results = vec![
            SearchResult {
                title: "标题A".into(),
                url: "https://a.com".into(),
                snippet: "摘要A".into(),
            },
            SearchResult {
                title: "标题B".into(),
                url: "https://b.com".into(),
                snippet: "摘要B".into(),
            },
        ];
        let out = format_results(&results);
        assert!(out.contains("[1] 标题A"));
        assert!(out.contains("https://a.com"));
        assert!(out.contains("摘要A"));
        assert!(out.contains("[2] 标题B"));
    }

    #[test]
    fn validate_url_scheme_accepts_http() {
        assert!(validate_url_scheme("http://example.com").is_ok());
        assert!(validate_url_scheme("https://example.com").is_ok());
    }

    #[test]
    fn validate_url_scheme_rejects_others() {
        assert!(validate_url_scheme("file:///etc/passwd").is_err());
        assert!(validate_url_scheme("ftp://example.com").is_err());
        assert!(validate_url_scheme("javascript:alert(1)").is_err());
    }

    #[test]
    fn search_config_has_backend_with_tavily() {
        let c = SearchConfig {
            tavily_key: Some("k".into()),
            google_key: None,
            google_cx: None,
        };
        assert!(c.has_backend());
    }

    #[test]
    fn search_config_has_backend_with_google() {
        let c = SearchConfig {
            tavily_key: None,
            google_key: Some("k".into()),
            google_cx: Some("cx".into()),
        };
        assert!(c.has_backend());
    }

    #[test]
    fn search_config_no_backend_when_empty() {
        let c = SearchConfig {
            tavily_key: None,
            google_key: Some("k".into()),
            google_cx: None, // 缺 cx
        };
        assert!(!c.has_backend());

        let c2 = SearchConfig {
            tavily_key: Some("  ".into()), // 空白
            google_key: None,
            google_cx: None,
        };
        assert!(!c2.has_backend());
    }
}

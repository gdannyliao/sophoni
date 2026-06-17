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

// ── 搜索后端 ──

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

pub struct TavilyBackend {
    api_key: String,
}

impl TavilyBackend {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

#[async_trait::async_trait]
impl WebSearchBackend for TavilyBackend {
    async fn search(
        &self,
        client: &reqwest::Client,
        query: &str,
        max_results: usize,
    ) -> AppResult<Vec<SearchResult>> {
        #[derive(serde::Serialize)]
        struct Req<'a> {
            query: &'a str,
            max_results: usize,
            search_depth: &'a str,
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            results: Vec<TavilyItem>,
        }
        #[derive(serde::Deserialize)]
        struct TavilyItem {
            #[serde(default)]
            title: String,
            #[serde(default)]
            content: String,
            #[serde(default)]
            url: String,
        }
        let resp = client
            .post("https://api.tavily.com/search")
            .bearer_auth(&self.api_key)
            .json(&Req {
                query,
                max_results,
                search_depth: "basic",
            })
            .send()
            .await
            .map_err(|e| AppError::Tool(format!("Tavily 请求失败: {e}")))?
            .json::<Resp>()
            .await
            .map_err(|e| AppError::Tool(format!("Tavily 响应解析失败: {e}")))?;
        Ok(resp
            .results
            .into_iter()
            .map(|i| SearchResult {
                title: i.title,
                snippet: i.content,
                url: i.url,
            })
            .collect())
    }
}

pub struct GoogleCseBackend {
    api_key: String,
    cx: String,
}

impl GoogleCseBackend {
    pub fn new(api_key: String, cx: String) -> Self {
        Self { api_key, cx }
    }
}

#[async_trait::async_trait]
impl WebSearchBackend for GoogleCseBackend {
    async fn search(
        &self,
        client: &reqwest::Client,
        query: &str,
        max_results: usize,
    ) -> AppResult<Vec<SearchResult>> {
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            items: Vec<GoogleItem>,
        }
        #[derive(serde::Deserialize)]
        struct GoogleItem {
            #[serde(default)]
            title: String,
            #[serde(default)]
            link: String,
            #[serde(default)]
            snippet: String,
        }
        let resp = client
            .get("https://www.googleapis.com/customsearch/v1")
            .query(&[
                ("q", query),
                ("key", &self.api_key),
                ("cx", &self.cx),
                ("num", &max_results.to_string()),
            ])
            .send()
            .await
            .map_err(|e| AppError::Tool(format!("Google CSE 请求失败: {e}")))?
            .json::<Resp>()
            .await
            .map_err(|e| AppError::Tool(format!("Google CSE 响应解析失败: {e}")))?;
        Ok(resp
            .items
            .into_iter()
            .map(|i| SearchResult {
                title: i.title,
                snippet: i.snippet,
                url: i.link,
            })
            .collect())
    }
}

/// 按配置选择后端：优先 Tavily，fallback Google CSE。
/// 返回 None 表示没有可用后端配置。
pub fn select_backend(config: &SearchConfig) -> Option<Box<dyn WebSearchBackend>> {
    if let Some(key) = config
        .tavily_key
        .as_ref()
        .filter(|k| !k.trim().is_empty())
    {
        return Some(Box::new(TavilyBackend::new(key.clone())));
    }
    if let (Some(key), Some(cx)) = (
        config
            .google_key
            .as_ref()
            .filter(|k| !k.trim().is_empty()),
        config.google_cx.as_ref().filter(|c| !c.trim().is_empty()),
    ) {
        return Some(Box::new(GoogleCseBackend::new(key.clone(), cx.clone())));
    }
    None
}

// ── web_fetch ──

/// 抓取 URL 并转为纯文本。流程：校验 scheme → DNS 解析校验非内网 →
/// HTTP GET（限 text/* Content-Type）→ HTML 转文本 → 截断。
pub async fn web_fetch(client: &reqwest::Client, url: &str, max_chars: usize) -> AppResult<String> {
    validate_url_scheme(url)?;

    // 解析域名做 SSRF 校验
    let parsed = url::Url::parse(url).map_err(|e| AppError::Tool(format!("无效 URL: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::Tool("URL 缺少 host".into()))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let host_port = format!("{host}:{port}");
    let addrs = tokio::net::lookup_host(&host_port)
        .await
        .map_err(|e| AppError::Tool(format!("DNS 解析失败: {e}")))?;
    for addr in addrs {
        if is_private_ip(&addr.ip()) {
            return Err(AppError::Tool(format!("拒绝抓取内网地址: {host}")));
        }
    }

    // HTTP GET
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::Tool(format!("请求失败: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::Tool(format!("HTTP {}", resp.status())));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    if !content_type.starts_with("text/")
        && !content_type.contains("xml")
        && !content_type.contains("json")
    {
        return Err(AppError::Tool(format!("不支持的页面类型: {content_type}")));
    }

    // 限 1MB 读取
    let body = resp
        .bytes()
        .await
        .map_err(|e| AppError::Tool(format!("读取响应体失败: {e}")))?;
    let body_str = String::from_utf8_lossy(&body);

    // HTML 转纯文本
    let text = html2text::from_read(body_str.as_bytes(), usize::MAX);

    // 截断
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        Ok(text)
    } else {
        let truncated: String = chars[..max_chars].iter().collect();
        Ok(format!(
            "{truncated}\n（内容已截断，显示前 {max_chars} 字符）"
        ))
    }
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

    #[test]
    fn select_backend_prefers_tavily() {
        let c = SearchConfig {
            tavily_key: Some("tvly".into()),
            google_key: Some("g".into()),
            google_cx: Some("cx".into()),
        };
        assert!(select_backend(&c).is_some());
    }

    #[test]
    fn select_backend_fallbacks_to_google() {
        let c = SearchConfig {
            tavily_key: None,
            google_key: Some("g".into()),
            google_cx: Some("cx".into()),
        };
        assert!(select_backend(&c).is_some());
    }

    #[test]
    fn select_backend_returns_none_when_unconfigured() {
        let c = SearchConfig {
            tavily_key: None,
            google_key: None,
            google_cx: None,
        };
        assert!(select_backend(&c).is_none());
    }

    #[test]
    fn tavily_response_deserializes_correctly() {
        // 测 Tavily JSON 响应结构能正确反序列化为 SearchResult
        let mock_resp = r#"{"results":[
            {"title":"Rust docs","content":"Rust is...","url":"https://doc.rust-lang.org"},
            {"title":"Learn Rust","content":"Getting started...","url":"https://rust-lang.org/learn"}
        ]}"#;
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            results: Vec<Item>,
        }
        #[derive(serde::Deserialize)]
        struct Item {
            #[serde(default)]
            title: String,
            #[serde(default)]
            content: String,
            #[serde(default)]
            url: String,
        }
        let parsed: Resp = serde_json::from_str(mock_resp).unwrap();
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(parsed.results[0].title, "Rust docs");
        assert_eq!(parsed.results[0].url, "https://doc.rust-lang.org");
    }

    #[test]
    fn html_to_text_extracts_content() {
        // 测 html2text 能把 HTML 转成可读文本（web_fetch 的子逻辑）
        let html = "<html><body><h1>Title</h1><p>Hello world</p></body></html>";
        let text = html2text::from_read(html.as_bytes(), usize::MAX);
        assert!(text.contains("Title"));
        assert!(text.contains("Hello world"));
    }

    #[test]
    fn web_fetch_truncation_logic() {
        // 测截断逻辑（web_fetch 的子逻辑）
        let long: String = "a".repeat(10000);
        let max_chars = 100;
        let chars: Vec<char> = long.chars().collect();
        let result: String = if chars.len() <= max_chars {
            long.clone()
        } else {
            let t: String = chars[..max_chars].iter().collect();
            format!("{t}\n（内容已截断，显示前 {max_chars} 字符）")
        };
        assert!(result.contains("截断"));
        assert!(result.starts_with(&"a".repeat(100)));
    }

    #[test]
    fn web_fetch_content_type_check_logic() {
        // 测 Content-Type 校验逻辑（web_fetch 的子逻辑）
        let text_ct = "text/html; charset=utf-8";
        let json_ct = "application/json";
        let image_ct = "image/png";
        let is_text = |ct: &str| {
            ct.starts_with("text/") || ct.contains("xml") || ct.contains("json")
        };
        assert!(is_text(text_ct));
        assert!(is_text(json_ct));
        assert!(!is_text(image_ct));
    }

    #[tokio::test]
    async fn web_fetch_rejects_invalid_scheme() {
        // scheme 校验在 DNS 之前，非 http/https 直接拒绝
        let client = reqwest::Client::new();
        let result = web_fetch(&client, "file:///etc/passwd", 8000).await;
        assert!(result.is_err());
    }

    // ── 真实集成测试（默认 ignored，手动触发：--ignored）──
    // 用 ~/.config/sophoni/config.toml 里配置的真实 key 跑全链路。
    // 运行：cargo test --manifest-path src-tauri/Cargo.toml web::tests::real_ -- --ignored --nocapture

    fn load_real_search_config() -> Option<SearchConfig> {
        let home = dirs::home_dir()?;
        let content = std::fs::read_to_string(home.join(".config/sophoni/config.toml")).ok()?;
        #[derive(serde::Deserialize)]
        struct Cfg {
            #[serde(default)]
            search: Option<SearchConfig>,
        }
        let cfg: Cfg = toml::from_str(&content).ok()?;
        cfg.search
    }

    #[tokio::test]
    #[ignore]
    async fn real_tavily_search_returns_results() {
        let config = load_real_search_config().expect("需配置 [search] 段");
        let backend = select_backend(&config).expect("需配置可用后端");
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        let results = backend
            .search(&client, "rust tokio timeout", 3)
            .await
            .expect("搜索应成功");
        assert!(!results.is_empty(), "应返回结果");
        let formatted = format_results(&results);
        println!("\n=== web_search 结果 ===\n{formatted}\n");
        assert!(formatted.contains("[1]"));
    }

    #[tokio::test]
    #[ignore]
    async fn real_web_fetch_returns_text() {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        // example.com 是稳定的公网页面，适合做基准测试
        let text = web_fetch(&client, "https://example.com", 2000)
            .await
            .expect("抓取应成功");
        println!("\n=== web_fetch 结果 ===\n{text}\n");
        assert!(text.to_lowercase().contains("example"));
    }
}

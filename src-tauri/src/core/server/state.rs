use std::sync::Arc;
use tokio::sync::Mutex;

/// 服务层共享状态。通过 axum State 注入到每个 handler。
#[derive(Clone)]
pub struct ServerState {
    pub inner: Arc<ServerStateInner>,
}

pub struct ServerStateInner {
    /// 当前配对码（6 位）。配对成功后轮换。用 Mutex 因为会改。
    pub pair_code: Mutex<String>,
    /// 已签发的长期 token。配对成功后存这，重启失效。
    pub token: Mutex<Option<String>>,
}

impl ServerState {
    pub fn new() -> Self {
        let code = generate_pair_code();
        tracing::info!(%code, "server: 配对码生成");
        Self {
            inner: Arc::new(ServerStateInner {
                pair_code: Mutex::new(code),
                token: Mutex::new(None),
            }),
        }
    }

    /// 校验配对码，正确则签发新 token 并轮换配对码。返回 token。
    pub async fn try_pair(&self, code: &str) -> Option<String> {
        let mut pair = self.inner.pair_code.lock().await;
        if pair.is_empty() || *pair != code {
            return None;
        }
        let token = generate_token();
        *self.inner.token.lock().await = Some(token.clone());
        // 轮换配对码（一次性，防重放）
        *pair = generate_pair_code();
        tracing::info!("server: 配对成功，token 已签发，配对码已轮换");
        Some(token)
    }

    /// 校验请求的 token 是否匹配已签发的。
    pub async fn check_token(&self, token: &str) -> bool {
        let t = self.inner.token.lock().await;
        t.as_deref() == Some(token)
    }

    /// 读取当前配对码（供二维码生成）。
    pub async fn current_pair_code(&self) -> String {
        self.inner.pair_code.lock().await.clone()
    }
}

/// 6 位数字配对码。
fn generate_pair_code() -> String {
    let n: u32 = rand_u32() % 1_000_000;
    format!("{:06}", n)
}

/// 32 字节十六进制 token。
fn generate_token() -> String {
    let mut s = String::with_capacity(64);
    for _ in 0..32 {
        s.push_str(&format!("{:02x}", rand_u32() as u8));
    }
    s
}

/// 简单随机数（不依赖 rand crate，用系统时间 + 线程 id 做熵源）。
/// 对配对码/token 够用——它们是局域网内一次性凭证，不是密码学强度的密钥。
fn rand_u32() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tid = format!("{:?}", std::thread::current().id());
    let tid_hash = tid
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    nanos.wrapping_add(tid_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pair_with_correct_code_returns_token_and_rotates() {
        let state = ServerState::new();
        let code = state.inner.pair_code.lock().await.clone();
        let token = state.try_pair(&code).await;
        assert!(token.is_some(), "正确配对码应返回 token");

        // 旧配对码应已失效（轮换）
        let token2 = state.try_pair(&code).await;
        assert!(token2.is_none(), "旧配对码不应再次有效");
    }

    #[tokio::test]
    async fn pair_with_wrong_code_returns_none() {
        let state = ServerState::new();
        let real = state.inner.pair_code.lock().await.clone();
        let wrong = if real == "000000" { "111111" } else { "000000" };
        let token = state.try_pair(wrong).await;
        assert!(token.is_none(), "错误配对码应返回 None");
    }

    #[tokio::test]
    async fn check_token_validates_issued_token() {
        let state = ServerState::new();
        let code = state.inner.pair_code.lock().await.clone();
        let token = state.try_pair(&code).await.unwrap();
        assert!(state.check_token(&token).await, "已签发 token 应校验通过");
        assert!(!state.check_token("wrong").await, "错误 token 应校验失败");
    }
}

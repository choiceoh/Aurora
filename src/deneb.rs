use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct DenebClient {
    client: Client,
    base_url: String,
}

impl DenebClient {
    pub fn new(base_url: &str) -> Self {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Generic RPC call to Deneb gateway.
    pub async fn rpc_call(&self, method: &str, params: Value) -> Result<Value, String> {
        let url = format!("{}/api/v1/rpc", self.base_url);
        let id = format!("aurora-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());

        let body = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Deneb 연결 실패: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Deneb RPC 오류 (HTTP {status}): {text}"));
        }

        let resp: Value = response.json().await
            .map_err(|e| format!("Deneb 응답 파싱 실패: {e}"))?;

        // Check for RPC error
        if let Some(error) = resp.get("error") {
            let msg = error.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(format!("Deneb RPC 오류: {msg}"));
        }

        // Extract result
        resp.get("result").cloned()
            .ok_or_else(|| "Deneb 응답에 result 필드 없음".to_string())
    }

    /// Health check — calls aurora.ping
    pub async fn health_check(&self) -> Result<bool, String> {
        let result = self.rpc_call("aurora.ping", json!({})).await?;
        Ok(result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    /// Send a chat message to Deneb's AI agent.
    pub async fn chat_send(&self, message: &str, session_key: &str) -> Result<String, String> {
        let result = self.rpc_call("aurora.chat", json!({
            "message": message,
            "sessionKey": session_key,
        })).await?;

        result.get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Deneb 응답에 text 필드 없음".to_string())
    }

    /// Search Deneb's memory.
    pub async fn memory_search(&self, query: &str) -> Result<String, String> {
        let result = self.rpc_call("aurora.memory", json!({
            "query": query,
        })).await?;

        result.get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Deneb 메모리 검색 결과 없음".to_string())
    }
}

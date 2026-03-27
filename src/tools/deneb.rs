use crate::deneb::DenebClient;
use super::Registry;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::runtime::Handle;

pub fn register(registry: &mut Registry, client: Arc<DenebClient>) {
    register_ask_deneb(registry, client.clone());
    register_deneb_memory(registry, client);
}

fn register_ask_deneb(registry: &mut Registry, client: Arc<DenebClient>) {
    let handler = move |args: Value| -> Result<String, String> {
        let message = args.get("message")
            .and_then(|v| v.as_str())
            .ok_or("message 파라미터가 필요합니다")?
            .to_string();
        let session_key = args.get("session_key")
            .and_then(|v| v.as_str())
            .unwrap_or("aurora")
            .to_string();

        let client = client.clone();
        let handle = Handle::current();
        // Block on async call from sync context
        std::thread::spawn(move || {
            handle.block_on(client.chat_send(&message, &session_key))
        })
        .join()
        .map_err(|_| "Deneb 호출 스레드 오류".to_string())?
    };

    registry.register_tool(
        "ask_deneb",
        "Deneb AI 에이전트에게 메시지를 보내고 응답을 받습니다. Deneb은 장기 메모리와 다양한 도구를 갖춘 AI 에이전트입니다.",
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Deneb에게 보낼 메시지"
                },
                "session_key": {
                    "type": "string",
                    "description": "세션 식별자 (기본값: aurora)"
                }
            },
            "required": ["message"]
        }),
        Box::new(handler),
    );
}

fn register_deneb_memory(registry: &mut Registry, client: Arc<DenebClient>) {
    let handler = move |args: Value| -> Result<String, String> {
        let query = args.get("query")
            .and_then(|v| v.as_str())
            .ok_or("query 파라미터가 필요합니다")?
            .to_string();

        let client = client.clone();
        let handle = Handle::current();
        std::thread::spawn(move || {
            handle.block_on(client.memory_search(&query))
        })
        .join()
        .map_err(|_| "Deneb 메모리 검색 스레드 오류".to_string())?
    };

    registry.register_tool(
        "deneb_memory",
        "Deneb의 장기 메모리에서 정보를 검색합니다. 과거 대화, 기억된 사실, 컨텍스트 등을 찾을 수 있습니다.",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "검색할 내용"
                }
            },
            "required": ["query"]
        }),
        Box::new(handler),
    );
}

use super::Registry;
use crate::preprocessing::telegram::format_tables_for_telegram;
use serde_json::{json, Value};

pub fn register(registry: &mut Registry) {
    let handler = move |args: Value| -> Result<String, String> {
        let text = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("text 파라미터가 필요합니다")?;

        Ok(format_tables_for_telegram(text))
    };

    registry.register_tool(
        "telegram_format",
        "마크다운 테이블을 텔레그램에서 깨지지 않는 모노스페이스 <pre> 블록으로 변환합니다. 텔레그램은 마크다운 테이블(| col | col |)을 지원하지 않으므로 이 도구로 변환 후 복사하세요.",
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "마크다운 테이블이 포함된 텍스트"
                }
            },
            "required": ["text"]
        }),
        Box::new(handler),
    );
}

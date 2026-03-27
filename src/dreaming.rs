use crate::deneb::DenebClient;
use crate::types::Message;
use std::time::{Duration, Instant};

/// Dreaming 트리거 임계값
const DREAM_TIME_INTERVAL: Duration = Duration::from_secs(5 * 60); // 5분
const DREAM_MESSAGE_THRESHOLD: usize = 10; // 메시지 10개
const DREAM_CHARS_THRESHOLD: usize = 20_000; // 20KB 텍스트
const DREAM_SESSION_KEY: &str = "aurora-dream";

/// Dreaming 상태 추적
pub struct DreamTracker {
    last_dream_time: Instant,
    messages_since_dream: usize,
    chars_since_dream: usize,
    is_dreaming: bool,
    dream_count: usize,
}

impl DreamTracker {
    pub fn new() -> Self {
        Self {
            last_dream_time: Instant::now(),
            messages_since_dream: 0,
            chars_since_dream: 0,
            is_dreaming: false,
            dream_count: 0,
        }
    }

    /// 새 메시지가 추가될 때 호출 — 축적 데이터 갱신
    pub fn track_message(&mut self, content_len: usize) {
        self.messages_since_dream += 1;
        self.chars_since_dream += content_len;
    }

    /// 툴 결과가 추가될 때 호출 — 데이터 양만 추적
    pub fn track_tool_result(&mut self, content_len: usize) {
        // 툴 결과도 메시지 카운트에 포함 (데이터 양 정확히 추적)
        self.messages_since_dream += 1;
        self.chars_since_dream += content_len;
    }

    /// Dreaming을 트리거해야 하는지 판단
    pub fn should_dream(&self) -> bool {
        if self.is_dreaming {
            return false;
        }

        // 최소 메시지가 있어야 dream 의미 있음
        if self.messages_since_dream < 2 {
            return false;
        }

        // 조건 1: 시간 경과 (5분 이상 + 새 메시지 2개 이상)
        let time_trigger = self.last_dream_time.elapsed() >= DREAM_TIME_INTERVAL;

        // 조건 2: 데이터 양 초과 (메시지 수 또는 문자 수)
        let volume_trigger = self.messages_since_dream >= DREAM_MESSAGE_THRESHOLD
            || self.chars_since_dream >= DREAM_CHARS_THRESHOLD;

        time_trigger || volume_trigger
    }

    /// Dreaming 시작
    pub fn start_dreaming(&mut self) {
        self.is_dreaming = true;
    }

    /// Dreaming 완료 — 카운터 리셋
    pub fn finish_dreaming(&mut self) {
        self.is_dreaming = false;
        self.last_dream_time = Instant::now();
        self.messages_since_dream = 0;
        self.chars_since_dream = 0;
        self.dream_count += 1;
    }

    /// Dreaming 실패 시 — 상태만 리셋, 카운터는 유지하여 재시도 가능
    pub fn fail_dreaming(&mut self) {
        self.is_dreaming = false;
    }

    pub fn is_dreaming(&self) -> bool {
        self.is_dreaming
    }

    pub fn dream_count(&self) -> usize {
        self.dream_count
    }

    pub fn messages_since_dream(&self) -> usize {
        self.messages_since_dream
    }

    pub fn chars_since_dream(&self) -> usize {
        self.chars_since_dream
    }
}

/// UTF-8 safe truncation
fn safe_truncate_str(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    let mut end = max_chars;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// 대화 내용을 요약하여 Deneb에 장기 메모리로 저장
pub async fn dream(
    deneb: &DenebClient,
    messages: &[Message],
) -> Result<String, String> {
    // 시스템 프롬프트 제외, 최근 메시지만 요약 대상
    let relevant: Vec<&Message> = messages
        .iter()
        .filter(|m| m.role != "system")
        .collect();

    if relevant.is_empty() {
        return Err("요약할 대화가 없습니다".to_string());
    }

    // 대화 내용을 컴팩트한 텍스트로 변환
    let mut summary_input = String::with_capacity(8192);
    summary_input.push_str("다음 대화를 분석하여 중요한 정보를 장기 메모리에 저장해주세요.\n");
    summary_input.push_str("사용자의 코딩 스타일, 프로젝트 구조, 선호하는 패턴, 해결한 문제, 중요한 결정사항 등을 기억해주세요.\n\n");
    summary_input.push_str("--- 대화 내용 ---\n");

    let mut total_chars = 0;
    const MAX_DREAM_INPUT: usize = 20_000;

    // 최신 메시지부터 역순으로 수집
    for msg in relevant.iter().rev() {
        let role_label = match msg.role.as_str() {
            "user" => "사용자",
            "assistant" => "Aurora",
            "tool" => "도구결과",
            _ => &msg.role,
        };

        let content = msg.content.as_deref().unwrap_or("");
        // 툴 결과는 짧게, 일반 메시지도 제한 (UTF-8 safe)
        let content = if msg.role == "tool" {
            safe_truncate_str(content, 500)
        } else {
            safe_truncate_str(content, 2000)
        };

        let entry = format!("[{role_label}]: {content}\n");
        total_chars += entry.len();
        if total_chars > MAX_DREAM_INPUT {
            break;
        }
        summary_input.push_str(&entry);
    }

    // Deneb에 dream 요청 전송
    deneb
        .chat_send(&summary_input, DREAM_SESSION_KEY)
        .await
}

//! Интеграционные тесты контракта tool-пар на уровне сессии агента (ADR-084).
//!
//! Проверяют сквозной сценарий инцидента `read_image:3` (2026-10-14): в
//! истории остался висячий `tool_call`, провайдер отвечает фатальным HTTP 400,
//! а агент обязан отремонтировать запрос санитайзером и сделать ровно один
//! повтор хода — без L3-компактификации. Вынесены из `agent.rs` в отдельный
//! файл: тот у порога лимита длины продакшн-файла (`C-33`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::tests::{FakeLlm, make_session};
use crate::error::{HarnessError, Result};
use crate::llm::{ChatMessage, ChatRequest, LlmProvider, Role, ToolCall, ToolPairRepair};

/// Инвариант контракта tool-пар для проверок на уровне запроса: каждый
/// `id` из `tool_calls` ассистента закрыт tool-ответом до ближайшего
/// не-tool сообщения.
fn request_closes_all_tool_calls(messages: &[ChatMessage]) -> bool {
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == Role::Assistant && !messages[i].tool_calls.is_empty() {
            let ids: Vec<&str> = messages[i]
                .tool_calls
                .iter()
                .map(|c| c.id.as_str())
                .collect();
            i += 1;
            let start = i;
            while i < messages.len() && messages[i].role == Role::Tool {
                i += 1;
            }
            for id in ids {
                if !messages[start..i]
                    .iter()
                    .any(|m| m.tool_call_id.as_deref() == Some(id))
                {
                    return false;
                }
            }
        } else {
            i += 1;
        }
    }
    true
}

fn two_read_image_calls() -> Vec<ToolCall> {
    vec![
        ToolCall {
            id: "img:1".into(),
            name: "read_image".into(),
            arguments: serde_json::json!({}),
        },
        ToolCall {
            id: "img:2".into(),
            name: "read_image".into(),
            arguments: serde_json::json!({}),
        },
    ]
}

#[tokio::test]
async fn build_request_repairs_tool_pair_hole_in_history() {
    // Интеграционный уровень AgentSession: в истории дыра (второй
    // tool_call без ответа) — собранный запрос обязан быть валиден.
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut s = make_session(tmp.path(), Arc::new(FakeLlm::new()), |_| {});
    s.history.push(ChatMessage::user("сравни кадры"));
    s.history
        .push(ChatMessage::assistant("", two_read_image_calls()));
    s.history
        .push(ChatMessage::tool_result("img:1", "первый кадр"));

    let (request, repair) = s.build_request();
    assert_eq!(
        repair,
        ToolPairRepair {
            synthesized: 1,
            dropped_orphans: 0
        }
    );
    assert!(
        request_closes_all_tool_calls(&request.messages),
        "каждый tool_call закрыт: {:?}",
        request.messages
    );
    // История сессии не тронута (AD-5): правка только в копии запроса.
    assert_eq!(s.messages().len(), 3, "история без синтетических вставок");
}

/// Провайдер сценария инцидента (`read_image:3`): первый ход — HTTP 400
/// «нарушен контракт tool-пар», затем — успех.
#[derive(Debug)]
struct ToolPairErrorLlm {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl LlmProvider for ToolPairErrorLlm {
    fn name(&self) -> &'static str {
        "tool-pair"
    }
    fn model(&self) -> &'static str {
        "tool-pair-1"
    }
    async fn complete(&self, _req: ChatRequest) -> Result<ChatMessage> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Err(HarnessError::Llm(
                "kimi: HTTP 400: an assistant message with 'tool_calls' must be \
                 followed by tool messages responding to each 'tool_call_id'. The \
                 following tool_call_ids did not have response messages: read_image:3"
                    .into(),
            ))
        } else {
            Ok(ChatMessage::assistant("ответ после ретрая", Vec::new()))
        }
    }
}

#[tokio::test]
async fn http_400_tool_pair_contract_triggers_single_retry_and_journal_event() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(ToolPairErrorLlm {
        calls: AtomicUsize::new(0),
    });
    let mut s = make_session(tmp.path(), provider.clone(), |_| {});
    // История с дырой: при повторной сборке санитайзер реально чинит.
    s.history.push(ChatMessage::user("сравни кадры"));
    s.history
        .push(ChatMessage::assistant("", two_read_image_calls()));
    s.history
        .push(ChatMessage::tool_result("img:1", "первый кадр"));

    let reply = s.send("новая задача", None).await.expect("повтор хода");
    assert_eq!(reply, "ответ после ретрая");
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        2,
        "ровно один повтор после 400"
    );
    let journal = std::fs::read_to_string(s.log_path().expect("журнал")).expect("прочитать журнал");
    assert!(
        journal.contains("tool_pairs_repaired"),
        "ремонт зафиксирован событием: {journal}"
    );
    assert!(
        journal.contains("нарушен контракт tool-пар"),
        "заметка о контракте: {journal}"
    );
}

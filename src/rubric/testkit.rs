//! Общие фикстуры тестов `rubric` (B1): заглушка LLM-судьи, конфиги сэмплов,
//! рубрики-примеры, досье из двух источников. Разделяются тестовыми модулями
//! подмодулей.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::config::JudgeConfig;
use crate::error::{HarnessError, Result};
use crate::llm::{ChatMessage, ChatRequest, LlmProvider};

use super::report::{JudgeResponse, parse_judge_response};
use super::types::{Criterion, CriterionScore, EvidenceOn, Rubric};

/// Тестовый провайдер: возвращает заготовленные ответы по очереди.
#[derive(Debug)]
pub(super) struct FakeLlm {
    replies: Mutex<VecDeque<String>>,
}

impl FakeLlm {
    pub(super) fn new(replies: &[&str]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().map(|s| (*s).to_string()).collect()),
        }
    }
}

#[async_trait]
impl LlmProvider for FakeLlm {
    fn name(&self) -> &'static str {
        "fake"
    }
    fn model(&self) -> &'static str {
        "fake-judge-1"
    }
    async fn complete(&self, _req: ChatRequest) -> Result<ChatMessage> {
        let reply = self
            .replies
            .lock()
            .expect("mutex poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok(ChatMessage::assistant(reply, Vec::new()))
    }
}

/// Настройки судьи с одним сэмплом (для тестов потока запросов).
pub(super) fn one_sample() -> JudgeConfig {
    JudgeConfig {
        samples: 1,
        ..JudgeConfig::default()
    }
}

/// Три сэмпла судьи на критерий.
pub(super) fn three_samples() -> JudgeConfig {
    JudgeConfig {
        samples: 3,
        ..JudgeConfig::default()
    }
}

/// Рубрика-пример: два критерия с весами 1.0 и 3.0.
pub(super) fn sample_rubric() -> Rubric {
    let anchors = |tail: &str| {
        BTreeMap::from([
            (1u8, format!("отсутствует {tail}")),
            (3u8, format!("частично {tail}")),
            (5u8, format!("образцово {tail}")),
        ])
    };
    Rubric {
        name: "adr-quality".into(),
        description: "Качество ADR".into(),
        scale_max: 5,
        criteria: vec![
            Criterion {
                id: "context".into(),
                name: "Контекст".into(),
                description: "Описан контекст и проблема".into(),
                weight: 1.0,
                anchors: anchors("контекст"),
                evidence_on: EvidenceOn::High,
                evidence_roles: Vec::new(),
                coverage: None,
                blocking: false,
            },
            Criterion {
                id: "alternatives".into(),
                name: "Альтернативы".into(),
                description: "Рассмотрены альтернативы".into(),
                weight: 3.0,
                anchors: anchors("альтернативы"),
                evidence_on: EvidenceOn::High,
                evidence_roles: Vec::new(),
                coverage: None,
                blocking: false,
            },
        ],
        origin: "anchor".into(),
        pack: None,
    }
}

/// Оценка без меток (для тестов арифметики итога).
pub(super) fn plain_score(criterion_id: &str, weight: f64, score: u8) -> CriterionScore {
    CriterionScore {
        criterion_id: criterion_id.into(),
        weight,
        score,
        rationale: String::new(),
        samples: vec![score],
        checked: Vec::new(),
        stdev: 0.0,
        flags: Vec::new(),
        evidence_unconfirmed_ratio: 0.0,
        invalid_samples: 0,
    }
}

/// Критерий с заданным направлением доказательства.
pub(super) fn criterion(id: &str, weight: f64, on: EvidenceOn, roles: &[&str]) -> Criterion {
    Criterion {
        id: id.into(),
        name: id.into(),
        description: id.into(),
        weight,
        anchors: BTreeMap::new(),
        evidence_on: on,
        evidence_roles: roles.iter().map(|r| (*r).to_string()).collect(),
        coverage: None,
        blocking: false,
    }
}

/// Рубрика из одного переданного критерия.
pub(super) fn rubric_of(criteria: Vec<Criterion>) -> Rubric {
    Rubric {
        name: "semantic".into(),
        description: "Смысловая рубрика".into(),
        scale_max: 5,
        criteria,
        origin: "anchor".into(),
        pack: None,
    }
}

/// Сырой ответ судьи по одному критерию.
pub(super) fn run_of(criterion_id: &str, score: u8, rationale: &str) -> JudgeResponse {
    parse_judge_response(&format!(
        r#"{{"scores": [{{"criterion_id": "{criterion_id}", "score": {score}, "rationale": {}}}], "verdict": "v"}}"#,
        serde_json::to_string(rationale).expect("json-строка")
    ))
    .expect("ответ судьи")
}

/// Досье с двумя источниками: субъект и ссылка.
pub(super) fn two_source_pack() -> crate::rubric_pack::ContextPack {
    let text = format!(
        "{begin} subject: docs/adr/ADR-001.md ===\n\
         Решение: контроль слоя построен без LLM в гейте.\n\
         {end}\n\
         {begin} reference: ARCHITECTURE-SPINE.md#AD-2 ===\n\
         AD-2: Детерминированный слой контроля\nRule: механика контроля без LLM.\n\
         {end}\n",
        begin = crate::rubric_pack::SOURCE_BEGIN,
        end = crate::rubric_pack::SOURCE_END,
    );
    let inputs = vec![
        crate::rubric_pack::PackInput {
            path: "docs/adr/ADR-001.md".into(),
            sha256: crate::hash::sha256_hex(b"subject"),
            role: crate::rubric_pack::InputRole::Subject,
            id: None,
            status: None,
        },
        crate::rubric_pack::PackInput {
            path: "ARCHITECTURE-SPINE.md#AD-2".into(),
            sha256: crate::hash::sha256_hex(b"reference"),
            role: crate::rubric_pack::InputRole::Reference,
            id: Some("AD-2".into()),
            status: None,
        },
    ];
    let sha = crate::hash::sha256_hex(text.as_bytes());
    crate::rubric_pack::ContextPack {
        kind: crate::rubric_pack::PackKind::AdrVsSpine,
        subject: "docs/adr/ADR-001.md".into(),
        text,
        sha256: sha,
        inputs,
    }
}

/// Текст ошибки `Result` для ассертов.
pub(super) fn err_text(err: &HarnessError) -> String {
    err.to_string()
}

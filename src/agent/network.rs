//! «Длинное терпение» транспортных сбоев модели (DPI/VPN-окна) — подмодуль
//! `agent`, вынесенный из `agent.rs` под лимит 3000 строк на продовый модуль
//! (волна B1, правило `prod_file_length_limit`).
//!
//! Быстрая Network-политика провайдера (десятки секунд) не переживает
//! минутные окна DPI/VPN: пока ошибка класса Network и не вышел стенной
//! бюджет `[agent] network_retry_budget_secs` — вызов модели повторяется
//! целиком (внутри него — прежние короткие ретраи провайдера). В UI уходят
//! заметки с остатком бюджета; прерывание — токен отмены (TUI: Esc).

use std::time::Duration;

use tokio::sync::mpsc;

use super::{AgentEvent, AgentSession, race_cancel};
use crate::error::{HarnessError, Result};
use crate::llm::ChatMessage;

/// Префикс заметки «модель недоступна, ждём» — TUI распознаёт её и
/// показывает индикатор сети в статус-баре (не менять без правки app.rs).
pub(crate) const NET_WAIT_NOTE_PREFIX: &str = "⚠ модель недоступна (сеть/DPI)";
/// Префикс заметки «модель снова доступна» — снимает индикатор в TUI.
pub(crate) const NET_BACK_NOTE_PREFIX: &str = "модель снова доступна";

/// Ошибка класса Network по классификатору ретраев (текст — Display):
/// таймауты, reset/DPI, DNS — всё, что может починиться само за минуты.
fn is_network_error(err: &HarnessError) -> bool {
    crate::retry::classify(None, &err.to_string()) == crate::retry::ErrorKind::Network
}

impl AgentSession {
    /// Вызов модели с «длинным терпением» на транспортных окнах: быстрая
    /// Network-политика провайдера (десятки секунд) не переживает минутные
    /// окна DPI/VPN. Пока ошибка класса Network и не вышел стенной бюджет
    /// `[agent] network_retry_budget_secs` — ждём и повторяем вызов целиком
    /// (внутри него — прежние короткие ретраи провайдера). В UI уходят
    /// заметки с остатком бюджета; прерывание — токен отмены (TUI: Esc).
    /// Возвращает None при отмене хода во время паузы или повторного вызова.
    pub(super) async fn network_patience(
        &mut self,
        first_err: HarnessError,
        events: &Option<mpsc::Sender<AgentEvent>>,
        streamed: &mut bool,
    ) -> Option<Result<ChatMessage>> {
        let budget = Duration::from_secs(self.config.agent.network_retry_budget_secs);
        if budget.is_zero() || !is_network_error(&first_err) {
            return Some(Err(first_err));
        }
        let started = std::time::Instant::now();
        let mut err = first_err;
        #[cfg(test)]
        let policy = self
            .net_patience_policy
            .unwrap_or(crate::retry::NETWORK_PATIENCE);
        #[cfg(not(test))]
        let policy = crate::retry::NETWORK_PATIENCE;
        // Джиттер детерминирован от наносекунд запуска (как в openai_compat).
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::from(d.subsec_nanos()));
        let mut delays = policy.delays(seed);
        let mut attempt = 1usize;
        loop {
            let left = budget.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Some(Err(err));
            }
            let wait = delays.next().unwrap_or(Duration::from_secs(30)).min(left);
            attempt += 1;
            self.emit_note(
                events,
                format!(
                    "{NET_WAIT_NOTE_PREFIX} — повторяю в фоне: попытка {attempt}, \
                     пауза {}с, осталось ~{}с (прервать — Esc)",
                    wait.as_secs(),
                    left.as_secs(),
                ),
            );
            race_cancel(self.cancel.clone(), tokio::time::sleep(wait)).await?;
            // Запрос пересобирается: история за время ожидания не менялась,
            // но компактификация следующего хода увидит её актуальной.
            let request = self.build_request();
            let call = match (events, self.config.agent.stream) {
                (Some(tx), true) => {
                    *streamed = true;
                    race_cancel(self.cancel.clone(), self.stream_request(request, tx)).await
                }
                _ => race_cancel(self.cancel.clone(), self.provider.complete(request)).await,
            };
            let outcome = call?;
            match outcome {
                Ok(reply) => {
                    self.emit_note(
                        events,
                        format!("{NET_BACK_NOTE_PREFIX} — продолжаю (попытка {attempt})"),
                    );
                    return Some(Ok(reply));
                }
                Err(e) if is_network_error(&e) => err = e,
                // Несетевая ошибка (auth, 4xx, контекст) — ретраить нечего.
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::mpsc;

    use super::{NET_BACK_NOTE_PREFIX, NET_WAIT_NOTE_PREFIX};
    use crate::agent::AgentEvent;
    use crate::agent::tests::make_session;
    use crate::error::{HarnessError, Result};
    use crate::llm::{ChatMessage, ChatRequest, LlmProvider};

    /// Провайдер «флапающая сеть»: первые `fails` вызовов — транспортная
    /// ошибка (подпись DPI-окна), затем финальный ответ.
    #[derive(Debug)]
    struct FlapNetLlm {
        calls: AtomicUsize,
        fails: usize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for FlapNetLlm {
        fn name(&self) -> &'static str {
            "flap"
        }
        fn model(&self) -> &'static str {
            "flap-1"
        }
        async fn complete(&self, _req: ChatRequest) -> Result<ChatMessage> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fails {
                Err(HarnessError::Llm(
                    "flap: не удалось отправить запрос: соединение сброшено сетью/DPI".into(),
                ))
            } else {
                Ok(ChatMessage::assistant("дождались", Vec::new()))
            }
        }
    }

    /// Провайдер «сеть лежит всегда» (окно DPI длиннее бюджета).
    #[derive(Debug)]
    struct DownNetLlm {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for DownNetLlm {
        fn name(&self) -> &'static str {
            "down"
        }
        fn model(&self) -> &'static str {
            "down-1"
        }
        async fn complete(&self, _req: ChatRequest) -> Result<ChatMessage> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(HarnessError::Llm(
                "down: не удалось отправить запрос: таймаут соединения (operation timed out)"
                    .into(),
            ))
        }
    }

    /// Провайдер с несетевой ошибкой (auth): ретраить нечего.
    #[derive(Debug)]
    struct AuthLlm {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmProvider for AuthLlm {
        fn name(&self) -> &'static str {
            "auth"
        }
        fn model(&self) -> &'static str {
            "auth-1"
        }
        async fn complete(&self, _req: ChatRequest) -> Result<ChatMessage> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(HarnessError::Llm("auth: HTTP 401: invalid api key".into()))
        }
    }

    /// Миллисекундные паузы вместо секундных — тесты не ждут стенной политики.
    fn ms_patience() -> crate::retry::RetryPolicy {
        crate::retry::RetryPolicy {
            max_attempts: 1_000,
            base_ms: 1,
            max_ms: 5,
            jitter_pct: 0,
        }
    }

    #[tokio::test]
    async fn network_patience_retries_until_model_recovers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let llm = Arc::new(FlapNetLlm {
            calls: AtomicUsize::new(0),
            fails: 2,
        });
        let mut s = make_session(tmp.path(), llm.clone(), |cfg| {
            cfg.agent.network_retry_budget_secs = 180;
        });
        s.net_patience_policy = Some(ms_patience());
        let (tx, mut rx) = mpsc::channel(32);
        let reply = s
            .send("ход", Some(tx))
            .await
            .expect("ход обязан переждать окно DPI, а не упасть");
        assert_eq!(reply, "дождались");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 3, "2 сбоя + успех");
        let notes: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|ev| match ev {
                AgentEvent::Note(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(
            notes.iter().any(|t| t.starts_with(NET_WAIT_NOTE_PREFIX)),
            "заметка ожидания ушла в UI: {notes:?}"
        );
        assert!(
            notes.iter().any(|t| t.starts_with(NET_BACK_NOTE_PREFIX)),
            "заметка восстановления ушла в UI: {notes:?}"
        );
    }

    #[tokio::test]
    async fn network_patience_gives_up_after_budget() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let llm = Arc::new(DownNetLlm {
            calls: AtomicUsize::new(0),
        });
        let mut s = make_session(tmp.path(), llm.clone(), |cfg| {
            // Секунды бюджета хватит на несколько миллисекундных попыток.
            cfg.agent.network_retry_budget_secs = 1;
        });
        s.net_patience_policy = Some(ms_patience());
        let err = s
            .send("ход", None)
            .await
            .expect_err("окно длиннее бюджета — ход падает с исходной ошибкой");
        assert!(
            err.to_string().contains("timed out") || err.to_string().contains("таймаут"),
            "наверх уходит исходная сетевая ошибка: {err}"
        );
        assert!(
            llm.calls.load(Ordering::SeqCst) >= 2,
            "до сдачи было несколько попыток: {}",
            llm.calls.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn network_patience_disabled_at_zero_budget() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let llm = Arc::new(DownNetLlm {
            calls: AtomicUsize::new(0),
        });
        let mut s = make_session(tmp.path(), llm.clone(), |cfg| {
            cfg.agent.network_retry_budget_secs = 0;
        });
        let err = s
            .send("ход", None)
            .await
            .expect_err("бюджет 0 — старое поведение: падение сразу");
        assert!(err.to_string().contains("таймаут") || err.to_string().contains("timed out"));
        assert_eq!(
            llm.calls.load(Ordering::SeqCst),
            1,
            "без бюджета — один вызов, ретраев нет"
        );
    }

    #[tokio::test]
    async fn non_network_error_is_not_retried() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let llm = Arc::new(AuthLlm {
            calls: AtomicUsize::new(0),
        });
        let mut s = make_session(tmp.path(), llm.clone(), |cfg| {
            cfg.agent.network_retry_budget_secs = 180;
        });
        s.net_patience_policy = Some(ms_patience());
        let err = s
            .send("ход", None)
            .await
            .expect_err("auth-ошибка не лечится ожиданием");
        assert!(err.to_string().contains("401"));
        assert_eq!(
            llm.calls.load(Ordering::SeqCst),
            1,
            "несетевая ошибка — без ретраев"
        );
    }
}

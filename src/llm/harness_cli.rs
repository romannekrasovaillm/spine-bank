//! Провайдер LLM через внешний CLI-агент (`kind = "cli"` в `[models.*]`).
//!
//! КОНТРАКТ (владелец: модуль `llm::harness_cli`; план «Spine без собственной
//! LLM», вариант 1 — `CliHarnessProvider`):
//! - Назначение — вызовы модели, которым не нужны tool-calls в ответе
//!   (LLM-судья рубрик, `bench --golden`, `distill`, cron-задачи): вместо
//!   своего API-ключа Spine запускает УЖЕ АВТОРИЗОВАННЫЙ на машине
//!   пользователя CLI-харнесс (Claude Code, Codex, Qwen Code…) как
//!   подпроцесс — платит подписка хоста. Для агентного цикла с tool-calls
//!   провайдер НЕ подходит (см. ограничения ниже).
//! - Конфигурация: `kind = "cli"`, `command` (имя/путь бинаря), `args`
//!   (подставляются как есть), `timeout_secs` (дефолт 180 — общий потолок
//!   прогона). `base_url`/`api_key_*`/прокси/TLS-поля не используются.
//! - Промпт — ОДИН плоский текст, передаётся через STDIN (защита от лимитов
//!   длины argv и квотинга шелла): сообщения рендерятся блоками
//!   «[system]\n…», «[user]\n…», «[assistant]\n…», «[tool]\n…»; `tool_calls`
//!   ассистента — строкой «[assistant вызвал инструменты: `name(args_json)`, …]».
//! - Окружение НАСЛЕДУЕТСЯ целиком (в отличие от `env_scrub` у bash): смысл
//!   провайдера — авторизация самого CLI (его ключи/сессии), чистое окружение
//!   сломало бы вход. Вывод процесса в логи целиком не пишется; stderr в
//!   ошибке усечён до [`MAX_STDERR_CHARS`] символов.
//! - Ответ: stdout триммится; если он парсится как JSON со строковым полем
//!   `result` — берётся оно (конверт `claude -p --output-format json`), иначе
//!   строковые `text`/`content`, иначе — сырой stdout как текст. Пустой ответ
//!   после извлечения — ошибка [`HarnessError::Llm`].
//! - Ограничения по контракту: temperature, `max_tokens` и thinking из запроса
//!   ИГНОРИРУЮТСЯ (сэмплирование решает сам CLI). Tool-спеки запроса
//!   игнорируются с записью в `tracing::debug` (выбран более простой вариант
//!   контракта: секция «доступные инструменты» в промпте судье только мешает,
//!   а вызова всё равно нет). Изображения не передаются (у плоского промпта
//!   нет мультимодального канала) — тоже debug-запись.
//! - Стриминга нет: `stream()` — дефолтная обёртка трейта над `complete()`.
//! - Usage точно неизвестен (CLI не отдаёт его в общем виде): грубая оценка
//!   rough (4 символа ≈ 1 токен, как [`ChatMessage::rough_tokens`]) пишется
//!   в `tracing::debug`.
//! - Ошибки: бинарь не найден (spawn) — подсказка «установите CLI …»;
//!   ненулевой код выхода — код + усечённый stderr; таймаут `timeout_secs` —
//!   процесс убивается (`kill_on_drop` + явный kill), ошибка таймаута.

use std::fmt;
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::config::ModelConfig;
use crate::error::{HarnessError, Result};
use crate::llm::{ChatMessage, ChatRequest, LlmProvider, Role};

/// Потолок читаемого stdout, байт: сверх него вывод дренится и отбрасывается
/// (память не раздувается на разговорчивом CLI; ответ судьи — килобайты).
const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;

/// Потолок читаемого stderr, байт (дальше — дренаж с отбрасыванием); в текст
/// ошибки попадают первые [`MAX_STDERR_CHARS`] символов.
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// Усечение stderr в тексте ошибки, символов.
const MAX_STDERR_CHARS: usize = 2000;

/// Повторы spawn при ETXTBSY («Text file busy»): на многопоточной машине
/// открытый на запись fd свежего исполняемого файла может быть унаследован
/// чужим `fork()` в узком окне до его exec — ядро отклоняет наш exec по inode.
/// Окно транзиентно (чужой fd закрывается при его exec), поэтому короткий
/// ретрай лечит; без него параллельные прогоны (вкл. тесты) флапают.
const SPAWN_BUSY_RETRIES: usize = 5;

/// Пауза между попытками spawn при ETXTBSY, миллисекунд.
const SPAWN_BUSY_DELAY_MS: u64 = 20;

/// Провайдер LLM через внешний CLI-агент (контракт — в шапке модуля).
pub struct CliHarnessProvider {
    /// Имя провайдера из реестра (ключ `[models]`).
    name: String,
    /// Метка модели: `model` из конфига; при пустом — `command`
    /// (у cli-записей `model` — произвольная метка для отчётов/метрик).
    model: String,
    /// Команда (имя/путь бинаря CLI).
    command: String,
    /// Аргументы командной строки (как есть, без подстановок).
    args: Vec<String>,
    /// Общий таймаут прогона, секунды (`timeout_secs` из конфига).
    timeout_secs: u64,
}

impl fmt::Debug for CliHarnessProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliHarnessProvider")
            .field("name", &self.name)
            .field("model", &self.model)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl CliHarnessProvider {
    /// Собирает провайдера из конфигурации; процесс не запускается до
    /// первого запроса (лениво, как у остальных провайдеров).
    ///
    /// # Errors
    /// Пустой `command` при `kind = "cli"`.
    fn from_config(name: &str, cfg: &ModelConfig) -> Result<Self> {
        let command = cfg.command.as_deref().unwrap_or("").trim().to_string();
        if command.is_empty() {
            return Err(HarnessError::Llm(format!(
                "провайдер '{name}': kind = \"cli\" требует непустой command \
                 (бинарь CLI-харнесса, напр. \"claude\")"
            )));
        }
        let model = if cfg.model.trim().is_empty() {
            command.clone()
        } else {
            cfg.model.clone()
        };
        Ok(Self {
            name: name.to_string(),
            model,
            command,
            args: cfg.args.clone(),
            timeout_secs: cfg.timeout_secs.max(1),
        })
    }

    /// Собирает ОДИН плоский промпт из истории сообщений (формат блоков —
    /// в шапке модуля). Пустые content-блоки не рендерятся.
    fn build_prompt(req: &ChatRequest) -> String {
        let mut out = String::new();
        for msg in &req.messages {
            let content = msg.content.trim();
            if matches!(msg.role, Role::Assistant) && !msg.tool_calls.is_empty() {
                if !content.is_empty() {
                    let _ = write!(out, "[assistant]\n{content}\n\n");
                }
                let calls = msg
                    .tool_calls
                    .iter()
                    .map(|c| format!("{}({})", c.name, c.arguments))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(out, "[assistant вызвал инструменты: {calls}]\n");
            } else if !content.is_empty() {
                let _ = write!(out, "[{}]\n{content}\n\n", msg.role.as_str());
            }
        }
        out.trim_end().to_string()
    }

    /// Один прогон CLI: промпт в stdin, ответ из stdout. Окружение
    /// наследуется (авторизация — у самого CLI).
    ///
    /// # Errors
    /// Бинарь не найден / сбой spawn, ненулевой код выхода, таймаут.
    async fn run_once(&self, prompt: &str) -> Result<String> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Страховка: ребёнок не переживает родителя даже при панике выше.
            .kill_on_drop(true);
        let mut child = {
            let mut attempts = 0;
            loop {
                match cmd.spawn() {
                    Ok(child) => break child,
                    // ETXTBSY — транзиент (см. SPAWN_BUSY_RETRIES): ретрай.
                    Err(e)
                        if e.kind() == ErrorKind::ExecutableFileBusy
                            && attempts < SPAWN_BUSY_RETRIES =>
                    {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(SPAWN_BUSY_DELAY_MS)).await;
                    }
                    Err(e) => {
                        return Err(if e.kind() == ErrorKind::NotFound {
                            HarnessError::Llm(format!(
                                "провайдер '{}': команда '{}' не найдена — установите \
                                 CLI-харнесс (напр. Claude Code: `npm i -g \
                                 @anthropic-ai/claude-code`) или поправьте command в \
                                 config.toml [models.{}]",
                                self.name, self.command, self.name
                            ))
                        } else {
                            HarnessError::Llm(format!(
                                "провайдер '{}': не удалось запустить '{}': {e}",
                                self.name, self.command
                            ))
                        });
                    }
                }
            }
        };

        // Промпт пишется в stdin отдельной задачей: иначе дедлок на
        // заполненном pipe-буфере, пока читаются stdout/stderr.
        let writer = child.stdin.take().map(|mut pipe| {
            let data = prompt.to_string();
            tokio::spawn(async move {
                // Ошибка записи осознанно игнорируется: процесс вправе закрыть
                // stdin раньше (например, упасть с ошибкой до чтения промпта).
                let _ = pipe.write_all(data.as_bytes()).await;
                // drop(pipe) закрывает stdin — EOF для процесса.
            })
        });

        let stdout_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut readers = Vec::new();
        if let Some(out) = child.stdout.take() {
            readers.push(spawn_reader(out, stdout_buf.clone(), MAX_STDOUT_BYTES));
        }
        if let Some(err) = child.stderr.take() {
            readers.push(spawn_reader(err, stderr_buf.clone(), MAX_STDERR_BYTES));
        }

        let status = match tokio::time::timeout(
            Duration::from_secs(self.timeout_secs),
            child.wait(),
        )
        .await
        {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => {
                return Err(HarnessError::Llm(format!(
                    "провайдер '{}': сбой ожидания '{}': {e}",
                    self.name, self.command
                )));
            }
            Err(_) => {
                // kill() = SIGKILL + wait: процесс не остаётся жить после
                // таймаута; kill_on_drop — вторая страховка на панику выше.
                let _ = child.kill().await;
                return Err(HarnessError::Llm(format!(
                    "провайдер '{}': таймаут: CLI '{}' не ответил за {}с \
                     (timeout_secs в config.toml)",
                    self.name, self.command, self.timeout_secs
                )));
            }
        };

        if let Some(w) = &writer {
            w.abort();
        }
        // Читатели завершаются по EOF на закрытых пайпах; страховочный лимит.
        for reader in readers {
            let _ = tokio::time::timeout(Duration::from_secs(2), reader).await;
        }
        let take = |buf: &Arc<Mutex<Vec<u8>>>| {
            String::from_utf8_lossy(
                &buf.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .into_owned()
        };
        let stdout = take(&stdout_buf);
        let stderr = take(&stderr_buf);

        if !status.success() {
            let code = status
                .code()
                .map_or_else(|| "сигнал".into(), |c| c.to_string());
            let excerpt: String = stderr.trim().chars().take(MAX_STDERR_CHARS).collect();
            return Err(HarnessError::Llm(format!(
                "провайдер '{}': CLI '{}' завершился с кодом {code}: {excerpt}",
                self.name, self.command
            )));
        }
        Ok(stdout)
    }
}

/// Читатель потока: складывает вывод в ограниченный буфер (хранится НАЧАЛО —
/// там ответ/диагностика), сверх `cap` дренит и отбрасывает, чтобы процесс
/// не встал на заполненном pipe.
fn spawn_reader<R>(mut pipe: R, buf: Arc<Mutex<Vec<u8>>>, cap: usize) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut b = buf
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let room = cap.saturating_sub(b.len());
                    if room > 0 {
                        b.extend_from_slice(&chunk[..n.min(room)]);
                    }
                    drop(b);
                }
            }
        }
    })
}

/// Извлекает текст ответа из stdout CLI (контракт — в шапке модуля):
/// JSON-конверт `{"result": …}` → поле `result`; иначе `text`/`content`;
/// иначе сырой stdout. Пустые строковые поля пропускаются.
fn extract_answer(stdout: &str) -> String {
    let trimmed = stdout.trim();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        for key in ["result", "text", "content"] {
            if let Some(s) = value.get(key).and_then(serde_json::Value::as_str) {
                let s = s.trim();
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    trimmed.to_string()
}

#[async_trait]
impl LlmProvider for CliHarnessProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn model(&self) -> &str {
        &self.model
    }

    /// Нестриминговый запрос: один прогон CLI на запрос (контракт модуля).
    async fn complete(&self, req: ChatRequest) -> Result<ChatMessage> {
        if !req.tools.is_empty() {
            tracing::debug!(
                "провайдер '{}': {} tool-спек игнорируется — вызов инструментов \
                 через CLI-прокси недоступен (контракт harness_cli)",
                self.name,
                req.tools.len()
            );
        }
        let images: usize = req.messages.iter().map(|m| m.images.len()).sum();
        if images > 0 {
            tracing::debug!(
                "провайдер '{}': {images} изображений не переданы — у плоского \
                 промпта нет мультимодального канала",
                self.name
            );
        }
        let prompt = Self::build_prompt(&req);
        if prompt.is_empty() {
            return Err(HarnessError::Llm(format!(
                "провайдер '{}': пустой промпт (в сообщениях нет текста)",
                self.name
            )));
        }
        let stdout = self.run_once(&prompt).await?;
        let answer = extract_answer(&stdout);
        if answer.is_empty() {
            return Err(HarnessError::Llm(format!(
                "провайдер '{}': CLI '{}' вернул пустой ответ",
                self.name, self.command
            )));
        }
        // Точный usage CLI не отдаёт: грубая оценка (4 символа ≈ 1 токен) —
        // только в debug-лог, в журнал сессии не идёт.
        tracing::debug!(
            "провайдер '{}': ~{} токенов промпта, ~{} токенов ответа (оценка rough)",
            self.name,
            prompt.len() / 4,
            answer.len() / 4
        );
        Ok(ChatMessage::assistant(answer, Vec::new()))
    }
}

/// Фабрика CLI-провайдера (`kind = "cli"` в `[models.*]`).
///
/// # Errors
/// Пустой `command` в конфиге.
pub fn provider(name: &str, cfg: &ModelConfig) -> Result<Arc<dyn LlmProvider>> {
    Ok(Arc::new(CliHarnessProvider::from_config(name, cfg)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    /// Пишет исполняемый shell-скрипт фейкового CLI в tempdir (паттерн
    /// `tests/cli.rs::write_fake_harness`). Запись идёт во временное имя с
    /// последующим атомарным `rename`: читатель никогда не видит полузаписанный
    /// файл. Гонку ETXTBSY (чужой fork наследует наш открытый на запись fd;
    /// ядро отклоняет exec по inode) rename НЕ снимает — её лечит ретрай
    /// spawn в [`CliHarnessProvider::run_once`].
    fn write_fake_cli(dir: &Path, body: &str) -> PathBuf {
        let script = dir.join("fake-cli.sh");
        let staging = dir.join("fake-cli.sh.tmp");
        std::fs::write(&staging, format!("#!/bin/sh\n{body}\n")).expect("запись fake CLI");
        // +x: без права на исполнение spawn вернёт PermissionDenied.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(&staging)
                .expect("stat скрипта")
                .permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&staging, perms).expect("chmod +x");
        }
        std::fs::rename(&staging, &script).expect("атомарная установка скрипта");
        script
    }

    /// Провайдер поверх фейкового CLI-скрипта из tempdir.
    fn cli_provider(dir: &Path, body: &str, timeout_secs: u64) -> Arc<dyn LlmProvider> {
        let script = write_fake_cli(dir, body);
        let mc = ModelConfig {
            kind: Some("cli".into()),
            command: Some(script.to_string_lossy().into_owned()),
            timeout_secs,
            ..ModelConfig::default()
        };
        provider("fake-cli", &mc).expect("провайдер собирается")
    }

    #[test]
    fn build_prompt_flattens_roles_and_tool_calls() {
        let req = ChatRequest::chat(vec![
            ChatMessage::system("Ты — судья."),
            ChatMessage::user("Оцени текст"),
            ChatMessage::assistant(
                "",
                vec![crate::llm::ToolCall {
                    id: "c1".into(),
                    name: "kb_search".into(),
                    arguments: serde_json::json!({"q": 1}),
                }],
            ),
            ChatMessage::tool_result("c1", "найдено"),
        ]);
        let prompt = CliHarnessProvider::build_prompt(&req);
        assert!(prompt.contains("[system]\nТы — судья."), "{prompt}");
        assert!(prompt.contains("[user]\nОцени текст"), "{prompt}");
        assert!(
            prompt.contains("[assistant вызвал инструменты: kb_search({\"q\":1})]"),
            "{prompt}"
        );
        assert!(prompt.contains("[tool]\nнайдено"), "{prompt}");
    }

    #[test]
    fn extract_answer_prefers_result_then_text_then_raw() {
        assert_eq!(extract_answer("{\"result\": \" ok \"}"), "ok");
        assert_eq!(extract_answer("{\"text\": \"t\"}"), "t");
        assert_eq!(extract_answer("{\"content\": \"c\"}"), "c");
        // JSON без известных полей — возвращается сырым текстом.
        assert_eq!(extract_answer("{\"other\": 1}"), "{\"other\": 1}");
        assert_eq!(extract_answer("  plain\n"), "plain");
        // Пустой result пропускается, срабатывает следующий ключ.
        assert_eq!(extract_answer("{\"result\": \"\", \"text\": \"t2\"}"), "t2");
    }

    #[test]
    fn cli_model_label_falls_back_to_command() {
        // model пуст — меткой становится command (для отчётов/метрик).
        let mc = ModelConfig {
            kind: Some("cli".into()),
            command: Some("claude".into()),
            ..ModelConfig::default()
        };
        let p = provider("claude-cli", &mc).expect("провайдер");
        assert_eq!(p.name(), "claude-cli");
        assert_eq!(p.model(), "claude");
        // Явная метка model имеет приоритет.
        let mc = ModelConfig {
            kind: Some("cli".into()),
            command: Some("claude".into()),
            model: "opus-4.7".into(),
            ..ModelConfig::default()
        };
        assert_eq!(
            provider("claude-cli", &mc).expect("провайдер").model(),
            "opus-4.7"
        );
    }

    #[test]
    fn cli_kind_without_command_is_rejected() {
        let mc = ModelConfig {
            kind: Some("cli".into()),
            ..ModelConfig::default()
        };
        let err = provider("broken", &mc).expect_err("пустой command — ошибка");
        assert!(err.to_string().contains("command"), "{err}");
    }

    #[test]
    fn registry_resolves_cli_kind_provider() {
        let cfg = crate::config::Config {
            models: [(
                "claude-cli".into(),
                ModelConfig {
                    kind: Some("cli".into()),
                    command: Some("claude".into()),
                    args: vec!["-p".into(), "--output-format".into(), "json".into()],
                    ..ModelConfig::default()
                },
            )]
            .into(),
            default_model: "claude-cli".into(),
            ..Default::default()
        };
        let registry = crate::llm::LlmRegistry::from_config(&cfg).expect("registry");
        let p = registry.get("claude-cli").expect("провайдер в реестре");
        assert_eq!(p.name(), "claude-cli");
        // model не задан — меткой служит command.
        assert_eq!(p.model(), "claude");
        assert_eq!(registry.default().name(), "claude-cli");
    }

    #[tokio::test]
    async fn complete_extracts_result_field_from_json_envelope() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = cli_provider(tmp.path(), "printf '%s' '{\"result\":\"ok\"}'", 30);
        let msg = p
            .complete(ChatRequest::chat(vec![ChatMessage::user("привет")]))
            .await
            .expect("complete");
        assert_eq!(msg.content, "ok");
        assert!(msg.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn complete_returns_raw_stdout_as_text() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = cli_provider(tmp.path(), "echo 'простой ответ'", 30);
        let msg = p
            .complete(ChatRequest::chat(vec![ChatMessage::user("привет")]))
            .await
            .expect("complete");
        assert_eq!(msg.content, "простой ответ");
    }

    #[tokio::test]
    async fn nonzero_exit_yields_error_with_stderr_fragment() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = cli_provider(tmp.path(), "echo 'boo-диагностика' >&2\nexit 3", 30);
        let err = p
            .complete(ChatRequest::chat(vec![ChatMessage::user("привет")]))
            .await
            .expect_err("ненулевой exit — ошибка");
        let text = err.to_string();
        assert!(text.contains("кодом 3"), "код выхода в ошибке: {text}");
        assert!(
            text.contains("boo-диагностика"),
            "фрагмент stderr в ошибке: {text}"
        );
    }

    #[tokio::test]
    async fn timeout_kills_process_and_errors_fast() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = cli_provider(tmp.path(), "sleep 30", 1);
        let started = Instant::now();
        let err = p
            .complete(ChatRequest::chat(vec![ChatMessage::user("привет")]))
            .await
            .expect_err("таймаут — ошибка");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "ошибка таймаута пришла быстро: {elapsed:?}"
        );
        assert!(err.to_string().contains("таймаут"), "{err}");
    }

    #[tokio::test]
    async fn prompt_reaches_cli_through_stdin() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // `cat` эхом возвращает промпт — проверяем и формат блоков.
        let p = cli_provider(tmp.path(), "cat", 30);
        let msg = p
            .complete(ChatRequest::chat(vec![
                ChatMessage::system("Системная роль."),
                ChatMessage::user("УНИКАЛЬНЫЙ-МАРКЕР-42"),
            ]))
            .await
            .expect("complete");
        assert!(msg.content.contains("УНИКАЛЬНЫЙ-МАРКЕР-42"), "{msg:?}");
        assert!(msg.content.contains("[user]"), "{msg:?}");
        assert!(msg.content.contains("[system]"), "{msg:?}");
    }
}

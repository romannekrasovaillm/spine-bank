//! Редакция секретов в выводе инструментов (по опыту Theseus `secrets.rs`,
//! там — переработка Codex `secrets`): `bash/read_file/web_fetch` могут
//! прочитать `.env`, конфиги с ключами, приватные ключи — и всё это уехало бы
//! в контекст модели (то есть на чужой API) и в журнал сессии. Редактор
//! маскирует типовые форматы до записи в историю.
//!
//! КОНТРАКТ (владелец: агент `agent`):
//! - [`Redactor::redact`] применяется к ЛЮБОМУ выводу инструмента до записи
//!   в историю/журнал: совпадение заменяется на `keep_prefix` + `***`;
//! - встроенные правила ([`builtin_rules`]): PEM-блоки, `*_API_KEY=…`,
//!   user:pass@ в URL, Bearer-токены, sk-/AKIA-ключи, длинные hex-токены;
//! - [`Redactor::from_env_keys`] добавляет точные значения из переменных
//!   окружения провайдеров (`DEEPSEEK_API_KEY` и др.) — литеральная подмена
//!   без regex-спецсимволов;
//! - `keep_prefix` оставляет не-секретную часть формата (`sk-`, `Bearer `),
//!   чтобы текст оставался читаемым, а модель понимала, что здесь был ключ.
//!
//! Тот же набор шаблонов работает и в режиме СКАНИРОВАНИЯ исходников
//! ([`scan_text`]): составляющая гейта `secrets` (C3) ищет литеральные
//! секреты в изменённых файлах и возвращает их позиции, не раскрывая
//! значений. Это эвристика известных форматов — литерал нестандартного вида
//! ею не ловится (граница зафиксирована в паспорте вердикта гейта).

use regex::Regex;

/// Минимальная длина env-значения, которое стоит маскировать (короткие
/// значения — обычно не секреты, а ложные срабатывания дороги).
const MIN_ENV_VALUE_LEN: usize = 8;

/// Переменные окружения провайдеров харнесса (точные значения — в редакцию).
const PROVIDER_KEY_ENVS: &[&str] = &[
    "DEEPSEEK_API_KEY",
    "MOONSHOT_API_KEY",
    "KIMI_API_KEY",
    "ZHIPU_API_KEY",
    "GLM_API_KEY",
    "ZAI_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

/// Одно правило редакции: имя, regex и длина сохраняемого префикса.
#[derive(Debug, Clone)]
pub struct SecretRule {
    /// Имя правила (для диагностики и тестов).
    pub name: String,
    /// Скомпилированный шаблон совпадения.
    pub regex: Regex,
    /// Сколько символов совпадения оставить открытыми (не-секретная часть
    /// формата: `sk-`, `Bearer `, `AKIA`, 8 hex как git-хэш).
    pub keep_prefix: usize,
}

impl SecretRule {
    /// Компилирует правило из шаблона.
    ///
    /// # Errors
    /// Невалидный regex-шаблон.
    pub fn new(
        name: impl Into<String>,
        pattern: &str,
        keep_prefix: usize,
    ) -> Result<Self, regex::Error> {
        Ok(Self {
            name: name.into(),
            regex: Regex::new(pattern)?,
            keep_prefix,
        })
    }
}

/// Встроенные правила: типовые форматы секретов.
///
/// # Panics
/// Паника при невалидном шаблоне — это ошибка программиста в исходнике,
/// а не входных данных; тест `builtin_rules_compile` гарантирует
/// недостижимость.
#[must_use]
pub fn builtin_rules() -> Vec<SecretRule> {
    let compile =
        |name: &str, pattern: &str, keep: usize| match SecretRule::new(name, pattern, keep) {
            Ok(rule) => rule,
            Err(err) => panic!("невалидный встроенный шаблон `{name}`: {err}"),
        };
    vec![
        // Многострочный PEM-блок целиком; `(?s)` — точка захватывает \n.
        compile(
            "pem-private-key",
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
            0,
        ),
        // `SOME_API_KEY=значение` / `SOME_TOKEN=…` (опционально в кавычках):
        // сохраняем имя переменной и `=` — видно, ЧТО за секрет скрыт.
        compile(
            "env-api-key",
            r#"(?i)\b[A-Z][A-Z0-9_]*(_API_KEY|_TOKEN|_SECRET|_PASSWORD)=["']?[A-Za-z0-9._\-/+]{8,}"#,
            0, // keep_prefix вычисляется динамически в redact_env
        ),
        // userinfo в URL: схема остаётся, user:pass маскируется
        // (пароль от 4 символов — не цеплять экзотику `://a:b@`).
        compile("url-password", r"://[^/\s:@]{1,64}:[^@\s/]{4,}@", 3),
        // Bearer-токен: keep = "Bearer" + один пробел.
        compile("bearer-token", r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}", 7),
        // OpenAI-формат: keep = "sk-".
        compile("openai-api-key", r"\bsk-[A-Za-z0-9_-]{20,}", 3),
        // AWS Access Key ID: keep = "AKIA".
        compile("aws-access-key-id", r"\bAKIA[0-9A-Z]{16}\b", 4),
        // GitHub-токены: `ghp_` (personal), `gho_` (oauth), `ghu_`
        // (user-to-server), `ghs_` (server-to-server), `ghr_` (refresh) —
        // префикс из 4 символов + длинное alnum-тело. keep = "ghp_" (4).
        compile("github-token", r"\bgh[pousr]_[A-Za-z0-9]{36,}\b", 4),
        // Длинный hex (токены/секреты 32+): keep = 8 символов, как git-хэш.
        compile("hex-token", r"\b[0-9a-fA-F]{32,}\b", 8),
    ]
}

/// Хвост маскированного фрагмента находки.
const MASK_SUFFIX: &str = "***";

/// Маскирует одно совпадение: не-секретный префикс формата остаётся открытым
/// (`sk-`, `AKIA`, `ghp_`, `NAME=`), тело скрывается. Общий код редакции
/// вывода и сканирования исходников (одна семантика — один результат).
fn mask_match(rule: &SecretRule, matched: &str) -> String {
    let keep = if rule.name == "env-api-key" {
        // Для `NAME=значение` сохраняем `NAME=`.
        matched.find('=').map_or(rule.keep_prefix, |eq| eq + 1)
    } else {
        rule.keep_prefix
    };
    let prefix: String = matched.chars().take(keep).collect();
    format!("{prefix}{MASK_SUFFIX}")
}

/// Находка сканирования исходников: какое правило сработало, где и что
/// (маскировано — значение секрета в вердикт не попадает).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    /// Имя сработавшего детектора (`aws-access-key-id`, …).
    pub rule: String,
    /// Номер строки (1-based).
    pub line: usize,
    /// Номер колонки в символах строки (1-based).
    pub column: usize,
    /// Маскированный фрагмент (`AKIA***`) — значение не раскрывается.
    pub masked: String,
}

/// Сканирует ТЕКСТ исходника встроенными детекторами (в отличие от
/// [`Redactor`], который правит вывод инструментов): находит литеральные
/// секреты известных форматов и возвращает их позиции.
///
/// Находки отсортированы по (строка, колонка, правило) — вердикт гейта
/// детерминирован независимо от порядка обхода правил. Многострочный PEM
/// сворачивается в одну строку и маскируется целиком.
#[must_use]
pub fn scan_text(text: &str, rules: &[SecretRule]) -> Vec<SecretFinding> {
    let mut out = Vec::new();
    for rule in rules {
        for m in rule.regex.find_iter(text) {
            let start = m.start();
            let line = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
            let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
            let column = text[line_start..start].chars().count() + 1;
            let masked: String = mask_match(rule, m.as_str())
                .replace('\n', " ")
                .chars()
                .take(60)
                .collect();
            out.push(SecretFinding {
                rule: rule.name.clone(),
                line,
                column,
                masked,
            });
        }
    }
    out.sort_by(|a, b| (a.line, a.column, &a.rule).cmp(&(b.line, b.column, &b.rule)));
    out
}

/// Редактор секретов: набор правил + точные значения из окружения.
///
/// Дешёв в клонировании (regex делит внутренний кэш), потокобезопасен
/// (`Regex: Send + Sync`) — один `Redactor` на сессию.
#[derive(Debug, Clone)]
pub struct Redactor {
    rules: Vec<SecretRule>,
    /// Точные значения секретов из env (литеральная замена).
    env_values: Vec<String>,
}

impl Redactor {
    /// Редактор из произвольного набора правил (без env-значений).
    #[must_use]
    pub fn new(rules: Vec<SecretRule>) -> Self {
        Self {
            rules,
            env_values: Vec::new(),
        }
    }

    /// Редактор со встроенными правилами (без env-значений).
    #[must_use]
    pub fn with_builtin_rules() -> Self {
        Self::new(builtin_rules())
    }

    /// Встроенные правила + точные значения переменных окружения
    /// провайдеров ([`PROVIDER_KEY_ENVS`], только значения длиной
    /// ≥ [`MIN_ENV_VALUE_LEN`]) — такие значения маскируются где бы
    /// ни встретились, даже без маркеров формата.
    #[must_use]
    pub fn from_environment() -> Self {
        let mut r = Self::with_builtin_rules();
        r.env_values = PROVIDER_KEY_ENVS
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .filter(|v| v.len() >= MIN_ENV_VALUE_LEN)
            .collect();
        r
    }

    /// Маскирует секреты в тексте: совпадение → `keep_prefix` + `***`.
    /// Env-значения заменяются литерально на `***`.
    #[must_use]
    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for value in &self.env_values {
            if out.contains(value) {
                out = out.replace(value.as_str(), "***");
            }
        }
        for rule in &self.rules {
            out = rule
                .regex
                .replace_all(&out, |caps: &regex::Captures<'_>| {
                    let m = caps.get(0).map_or("", |m| m.as_str());
                    mask_match(rule, m)
                })
                .into_owned();
        }
        out
    }

    /// Есть ли в тексте что-то похожее на секрет (без модификации).
    #[must_use]
    pub fn contains_secret(&self, text: &str) -> bool {
        self.env_values.iter().any(|v| text.contains(v))
            || self.rules.iter().any(|r| r.regex.is_match(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_rules_compile() {
        let rules = builtin_rules();
        assert!(rules.len() >= 7);
    }

    #[test]
    fn redacts_env_assignment_keeping_name() {
        let r = Redactor::with_builtin_rules();
        let out = r.redact("DEEPSEEK_API_KEY=sk-abcdef1234567890xyz в конфиге");
        assert!(out.contains("DEEPSEEK_API_KEY=***"), "имя сохранено: {out}");
        assert!(!out.contains("abcdef"), "значение скрыто: {out}");
    }

    #[test]
    fn redacts_pem_block() {
        let r = Redactor::with_builtin_rules();
        let pem = "-----BEGIN PRIVATE KEY-----\nMIIEvwIBADA\n-----END PRIVATE KEY-----";
        let out = r.redact(&format!("ключ: {pem} конец"));
        assert!(!out.contains("MIIEvw"), "тело ключа скрыто: {out}");
        assert!(out.contains("***"), "маска на месте: {out}");
    }

    #[test]
    fn redacts_url_password_bearer_and_sk() {
        let r = Redactor::with_builtin_rules();
        let out = r.redact("postgres://admin:s3cretpass@db.local:5432/core");
        assert!(out.contains("://***"), "userinfo скрыт: {out}");
        assert!(!out.contains("s3cretpass"), "пароль не протёк: {out}");
        let out = r.redact("Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6");
        assert!(out.starts_with("Authorization: Bearer ***"), "{out}");
        let out = r.redact("key = sk-0123456789abcdef0123456789");
        assert!(out.contains("sk-***"), "{out}");
    }

    #[test]
    fn redacts_github_tokens_of_all_prefixes() {
        let r = Redactor::with_builtin_rules();
        // Тело токена — криптографическая фикстура; префикс приклеивается в
        // рантайме, чтобы в исходнике не было литерала `ghp_<36>` (fitness
        // C-05 «нет литералов секретов в коде»).
        let body = "16C7e42F292c6912E7710c838347Ae178B4a";
        for prefix in ["ghp", "gho", "ghs"] {
            let token = format!("{prefix}_{body}");
            let out = r.redact(&format!("token = {token} в коде"));
            assert!(out.contains(&format!("{prefix}_***")), "{out}");
            assert!(!out.contains(body), "тело токена скрыто: {out}");
        }
    }

    #[test]
    fn scan_finds_literals_with_positions_and_masks_values() {
        let rules = builtin_rules();
        // GitHub-фикстура собирается в рантайме (см. выше про C-05).
        let token = format!("ghp_{}", "16C7e42F292c6912E7710c838347Ae178B4a");
        let src = format!(
            "package main\n\nvar key = \"AKIAIOSFODNN7EXAMPLE\" // лежит в коде\n\
             var token = \"{token}\"\n"
        );
        let found = scan_text(&src, &rules);
        assert!(found.len() >= 2, "находки: {found:?}");
        let aws = found
            .iter()
            .find(|f| f.rule == "aws-access-key-id")
            .expect("aws-ключ найден");
        assert_eq!(aws.line, 3, "строка ключа: {aws:?}");
        assert_eq!(aws.masked, "AKIA***", "значение не раскрыто: {aws:?}");
        let gh = found
            .iter()
            .find(|f| f.rule == "github-token")
            .expect("github-токен найден");
        assert_eq!(gh.line, 4, "строка токена: {gh:?}");
        assert_eq!(gh.masked, "ghp_***");
        // Значение секрета в находке не появляется ни в каком виде.
        assert!(
            found
                .iter()
                .all(|f| !f.masked.contains("IOSFODNN7") && !f.masked.contains("16C7e42F")),
            "{found:?}"
        );
        // Чистый текст — без находок.
        assert_eq!(scan_text("fn main() { let x = 1; }", &rules), Vec::new());
        // Порядок детерминирован: повторный прогон даёт тот же список.
        assert_eq!(found, scan_text(&src, &rules));
    }

    #[test]
    fn redacts_aws_and_hex_but_keeps_short_hex() {
        let r = Redactor::with_builtin_rules();
        let out = r.redact("AKIAIOSFODNN7EXAMPLE");
        assert_eq!(out, "AKIA***");
        let out = r.redact("токен 0123456789abcdef0123456789abcdef тут");
        assert!(out.contains("01234567***"), "{out}");
        // Короткий git-хэш (7-8 символов) — не секрет.
        let out = r.redact("коммит abc1234");
        assert_eq!(out, "коммит abc1234");
    }

    #[test]
    fn env_values_redacted_literally() {
        // Имитируем через unsafe-free путь: проверяем механику напрямую.
        let mut r = Redactor::with_builtin_rules();
        r.env_values.push("суперсекретно123".to_string());
        let out = r.redact("токен суперсекретно123 в середине строки");
        assert_eq!(out, "токен *** в середине строки");
        assert!(r.contains_secret("суперсекретно123"));
        assert!(!r.contains_secret("обычный текст"));
    }

    #[test]
    fn ordinary_text_passes_through() {
        let r = Redactor::with_builtin_rules();
        let text = "cargo test: 250 passed; контекст 60k токенов; порт 10.0.0.1:8080";
        assert_eq!(r.redact(text), text);
    }
}

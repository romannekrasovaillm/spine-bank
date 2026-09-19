//! Паспорт вердикта (W1): одна страница, которая говорит, чего зелёный НЕ
//! означает.
//!
//! Главная претензия исследования к контуру контроля — «зелёный ≠ выпускать».
//! Паспорт превращает её из претензии в свойство продукта: рядом с любым
//! вердиктом (и зелёным, и красным) печатается страница из трёх блоков —
//! что механика проверила (с числами), что заявлено, но механикой не
//! проверяется, и что не проверено вовсе.
//!
//! Паспорт — ПРЕДСТАВЛЕНИЕ, а не новая проверка: он ничего не решает и не
//! меняет вердикт. Источники — уже собранный [`GateReport`] и файлы
//! репозитория (манифест бандла, реестр правил); аттестация вердикта от
//! паспорта не зависит.
//!
//! Отрицательные последствия (ADR-044): страница может устареть раньше
//! вердикта, если её читают отдельно от прогона; блок 2 выглядит как
//! оправдание и провоцирует «дописать эвристику» по смыслу текста. Поэтому
//! блок 2 собирается ТОЛЬКО из того, что репозиторий реально содержит
//! (правило на упоминание существует, отчёт рубрики есть, бандл есть) —
//! список не статичен и не превращается в универсальную отговорку.

use std::fmt::Write as _;
use std::path::Path;

use crate::gate::{GateReport, GateStatus};

/// Составляющая в блоке «Проверено»: имя, статус, сводка с числами и счёт
/// находок по важности.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// Имя составляющей гейта.
    pub name: String,
    /// `PASS` | `FAIL` | `SKIP`.
    pub status: String,
    /// Сводка составляющей (в ней и живут числа).
    pub detail: String,
    /// Обязательна ли составляющая для этого маршрута.
    pub required: bool,
    /// Находок `error`.
    pub errors: usize,
    /// Находок `warn`.
    pub warns: usize,
}

/// Утверждение, которое вердикт на себе несёт, но механикой не подтверждает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// Источник утверждения: имя составляющей, `бандл`, `решения`.
    pub source: String,
    /// Что заявлено и почему механика этого не доказывает.
    pub text: String,
    /// Что делать человеку вместо ожидания машинного ответа.
    pub instead: String,
}

/// Составляющая в блоке «Не проверено»: имя и причина, по которой входа нет.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotChecked {
    /// Имя составляющей.
    pub name: String,
    /// Причина (сводка составляющей).
    pub reason: String,
    /// Обязательна ли она для маршрута (тогда вердикт — INCOMPLETE, exit 3).
    pub required: bool,
}

/// Паспорт вердикта: три блока + аттестация и команда сверки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passport {
    /// Репозиторий в том написании, в каком его дал вызывающий.
    pub repo: String,
    /// Маршрут прогона (`Fast` | `Standard` | `Critical`).
    pub route: String,
    /// Итог вердикта (`PASS` | `FAIL` | `INCOMPLETE`).
    pub outcome: String,
    /// Exit-код канала.
    pub exit_code: i32,
    /// Заметка о маршруте (score и триггеры из диффа либо причина fail-safe).
    pub route_note: String,
    /// Блок 1: что проверено.
    pub checked: Vec<Checked>,
    /// Блок 2: что заявлено, но механикой не проверяется.
    pub claimed: Vec<Claim>,
    /// Блок 3: что не проверено.
    pub not_checked: Vec<NotChecked>,
    /// SHA-256 конверта вердикта.
    pub attestation: String,
    /// Команда сверки конверта с текущим состоянием дерева.
    pub verify_command: String,
}

impl Passport {
    /// Собирает паспорт по готовому вердикту; репозиторий подписывается
    /// своим путём.
    #[must_use]
    pub fn build(report: &GateReport, repo: &Path) -> Self {
        Self::build_labelled(report, repo, &repo.display().to_string())
    }

    /// То же, но с явной подписью репозитория: паспорт — документ для чтения,
    /// и в тестах/отчётах абсолютный путь песочницы только мешает (а в
    /// аттестации он и не участвует).
    #[must_use]
    pub fn build_labelled(report: &GateReport, repo: &Path, label: &str) -> Self {
        let checked: Vec<Checked> = report
            .components
            .iter()
            .map(|c| Checked {
                name: c.name.to_string(),
                status: c.status.label().to_string(),
                detail: c.detail.clone(),
                required: report.required.iter().any(|r| r == c.name),
                errors: c.findings.iter().filter(|f| f.severity == "error").count(),
                warns: c.findings.iter().filter(|f| f.severity != "error").count(),
            })
            .collect();
        let not_checked: Vec<NotChecked> = report
            .components
            .iter()
            .filter(|c| c.status == GateStatus::Skip)
            .map(|c| NotChecked {
                name: c.name.to_string(),
                reason: c.detail.clone(),
                required: report.not_checked.iter().any(|r| r == c.name),
            })
            .collect();
        Self {
            repo: label.to_string(),
            route: report.route.to_string(),
            outcome: report.outcome.label().to_string(),
            exit_code: report.outcome.exit_code(),
            route_note: report.route_note.clone(),
            checked,
            claimed: collect_claims(repo, report),
            not_checked,
            attestation: report.attestation.clone(),
            verify_command: format!(
                "arch-be gate --repo {label} --format json > verdict.json && \
                 arch-be gate --repo {label} --verify-envelope verdict.json"
            ),
        }
    }

    /// Машиночитаемая форма паспорта (`--format json`, MCP `verdict_explain`).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let claims = self
            .claimed
            .iter()
            .map(|c| {
                serde_json::json!({
                    "source": c.source,
                    "text": c.text,
                    "instead": c.instead,
                })
            })
            .collect::<Vec<_>>();
        let checked = self
            .checked
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "status": c.status,
                    "required": c.required,
                    "detail": c.detail,
                    "findings": {"error": c.errors, "warn": c.warns},
                })
            })
            .collect::<Vec<_>>();
        let not_checked = self
            .not_checked
            .iter()
            .map(|n| {
                serde_json::json!({
                    "name": n.name,
                    "reason": n.reason,
                    "required": n.required,
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "schema": "arch-be/verdict-passport/v1",
            "repo": self.repo,
            "route": self.route,
            "route_note": self.route_note,
            "verdict": self.outcome,
            "exit_code": self.exit_code,
            "checked": checked,
            "claimed_not_verified": claims,
            "not_checked": not_checked,
            "attestation": format!("sha256:{}", self.attestation),
            "verify_command": self.verify_command,
        })
    }

    /// Страница markdown (одна, рядом с любым вердиктом).
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Запись в String не может завершиться ошибкой — игноры безопасны.
        let _ = writeln!(out, "# Паспорт вердикта");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "**{outcome}** (exit {code}) · маршрут {route} · репозиторий `{repo}`  ",
            outcome = self.outcome,
            code = self.exit_code,
            route = self.route,
            repo = self.repo,
        );
        let _ = writeln!(out, "Маршрут: {}", self.route_note);
        let _ = writeln!(out);
        // Вступление зависит от итога: на красном «зелёный означает…» было бы
        // ложью. Красный тоже не всесилен — блоки 2 и 3 называют то, что не
        // поймала и механика.
        if self.outcome == "FAIL" {
            let _ = writeln!(
                out,
                "Красный здесь означает: механика нашла перечисленное в блоке 1. \
                 Блоки 2 и 3 говорят, чего не поймала и она, — на это не \
                 распространяется и красный."
            );
        } else {
            let _ = writeln!(
                out,
                "Зелёный здесь означает: механика проверила перечисленное в блоке 1 и \
                 нарушений не нашла. Он НЕ означает «выпускать» — блоки 2 и 3 говорят, \
                 на что зелёный не распространяется."
            );
        }
        let _ = writeln!(out);

        let _ = writeln!(out, "## 1. Проверено");
        let _ = writeln!(out);
        if self.checked.is_empty() {
            let _ = writeln!(out, "Составляющих не прогонялось.");
        } else {
            for c in &self.checked {
                let star = if c.required { " \\*" } else { "" };
                let _ = writeln!(out, "- **{}**{star} — {} — {}", c.name, c.status, c.detail);
                if c.errors > 0 || c.warns > 0 {
                    let _ = writeln!(out, "  находок: error {}, warn {}", c.errors, c.warns);
                }
            }
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "\\* — обязательна для маршрута {}; остальные — сверх неё.",
                self.route
            );
        }
        let _ = writeln!(out);

        let _ = writeln!(out, "## 2. Заявлено, но механикой не проверяется");
        let _ = writeln!(out);
        if self.claimed.is_empty() {
            let _ = writeln!(
                out,
                "Нечего назвать: репозиторий не содержит утверждений этого класса."
            );
        } else {
            for c in &self.claimed {
                let _ = writeln!(out, "- **{}** — {}", c.source, c.text);
                if !c.instead.is_empty() {
                    let _ = writeln!(out, "  → {}", c.instead);
                }
            }
        }
        let _ = writeln!(out);

        let _ = writeln!(out, "## 3. Не проверено");
        let _ = writeln!(out);
        if self.not_checked.is_empty() {
            let _ = writeln!(out, "Пропущенных составляющих нет — вход был у всех.");
        } else {
            for n in &self.not_checked {
                let tail = if n.required {
                    format!(
                        " (обязательна для маршрута {} — итог INCOMPLETE)",
                        self.route
                    )
                } else {
                    " (не обязательна для маршрута)".to_string()
                };
                let _ = writeln!(out, "- **{}** — {}{tail}", n.name, n.reason);
            }
        }
        let _ = writeln!(out);

        let _ = writeln!(out, "## Аттестация");
        let _ = writeln!(out);
        let _ = writeln!(out, "```");
        let _ = writeln!(out, "sha256:{}", self.attestation);
        let _ = writeln!(out, "{}", self.verify_command);
        let _ = writeln!(out, "```");
        out
    }
}

/// Блок 2: границы вердикта. Два источника, оба — факты о репозитории, а не
/// декларация:
///
/// 1. сами составляющие называют, чего их зелёный не означает
///    ([`crate::gate::GateComponent::not_verified`]);
/// 2. утверждения, которые вердикт несёт, но не проверяет ни одна
///    составляющая, — и они добавляются ТОЛЬКО если соответствующий артефакт
///    в репозитории есть.
fn collect_claims(repo: &Path, report: &GateReport) -> Vec<Claim> {
    let mut out: Vec<Claim> = Vec::new();
    for c in &report.components {
        for n in &c.not_verified {
            out.push(Claim {
                source: c.name.to_string(),
                text: n.clone(),
                instead: String::new(),
            });
        }
    }
    // Непринятые решения: если составляющая decision_quality не включена, о
    // качестве решений не говорит вообще ничто — и это надо назвать.
    if has_accepted_adr(repo) && !decision_quality_enabled(report) {
        out.push(Claim {
            source: "решения".to_string(),
            text: "качество принятых ADR механикой не оценивается: составляющая \
                   decision_quality не включена в [gate.required] этого маршрута"
                .to_string(),
            instead: "включите `decision_quality` и прогоните рубрику судьёй, \
                      отличным от автора документа"
                .to_string(),
        });
    }
    // Ревью: бандл хранит вердикт, но не подпись ревьюера.
    if bundle_has_artifact(repo, "adversarial_review") {
        out.push(Claim {
            source: "бандл".to_string(),
            text: "бандл хранит ВЕРДИКТ состязательного ревью, но не то, кто его \
                   написал: независимость ревьюера от автора изменения механикой \
                   не проверяется"
                .to_string(),
            instead: "подтвердите независимость ревьюера вне бандла (протокол \
                      ревью, PR-аппрув другого человека)"
                .to_string(),
        });
    }
    out
}

/// Есть ли в репозитории принятый прозаический ADR (`docs/adr/ADR-*.md`).
fn has_accepted_adr(repo: &Path) -> bool {
    let dir = repo.join("docs/adr");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md"))
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("ADR-"))
            && std::fs::read_to_string(&p).is_ok_and(|t| crate::gate::adr_is_accepted(&t))
    })
}

/// Включена ли составляющая `decision_quality` (не SKIP «не включена»).
fn decision_quality_enabled(report: &GateReport) -> bool {
    report
        .components
        .iter()
        .any(|c| c.name == "decision_quality" && c.status != GateStatus::Skip)
}

/// Есть ли в каком-нибудь бандле репозитория артефакт с таким ключом.
/// Нечитаемый манифест — «нет»: паспорт не гейт, он не имеет права падать.
fn bundle_has_artifact(repo: &Path, key: &str) -> bool {
    let mut dirs = vec![repo.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(repo.join("changes")) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.file_name().is_some_and(|n| n != "archive") {
                dirs.push(p);
            }
        }
    }
    dirs.iter().any(|d| {
        std::fs::read_to_string(d.join("EVIDENCE.yaml"))
            .ok()
            .and_then(|t| serde_yaml_ng::from_str::<crate::evidence::EvidenceBundle>(&t).ok())
            .is_some_and(|b| b.items.iter().any(|i| i.key == key))
    })
}

/// Строка-приглашение для `evidence verify`: паспорт печатается гейтом, но
/// искать его надо оттуда, где читатель только что видел бандл.
#[must_use]
pub fn hint_command(change_dir: &Path) -> String {
    format!("arch-be gate --repo {} --explain", change_dir.display())
}

/// Итог паспорта одной строкой — для встраивания в чужой вывод
/// (`evidence verify` печатает рядом с вердиктом бандла).
#[must_use]
pub fn summary_line(passport: &Passport) -> String {
    format!(
        "Паспорт вердикта: {} · проверено составляющих {} · заявлено без проверки {} · не проверено {}",
        passport.outcome,
        passport.checked.len(),
        passport.claimed.len(),
        passport.not_checked.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use crate::gate::{GateRequirements, run_with};
    use std::process::Command;

    /// git в каталоге с тестовой идентичностью коммиттера (образец —
    /// `src/gate.rs::tests::git`).
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Репозиторий с одним правилом (указано ли в нём несуществующее имя —
    /// этим задаётся зелёный или красный вердикт).
    fn make_repo(dir: &Path, required_file: &str) {
        std::fs::write(
            dir.join("CONSTRAINTS.yaml"),
            format!(
                "rules:\n  - name: spine_present\n    type: file_exists\n    \
                 path: \"{required_file}\"\n    severity: error\n  - name: \
                 adr_alternatives\n    type: each_file_must_contain\n    glob: \
                 \"docs/adr/*.md\"\n    pattern: \"Alternatives\"\n    severity: error\n"
            ),
        )
        .expect("constraints");
        std::fs::write(
            dir.join("ARCHITECTURE-SPINE.md"),
            "# Spine\n\n## AD-1. Контур\n\n- Binds: REQ-001\n- Prevents: обход гейта\n- Rule: C-001\n",
        )
        .expect("spine");
        // ADR без статуса Accepted: правило `each_file_must_contain` получает
        // непустой набор файлов (пустой набор — сам по себе находка), но
        // утверждения о качестве решений репозиторий ещё не несёт.
        std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir adr");
        std::fs::write(
            dir.join("docs/adr/ADR-001-demo.md"),
            "# ADR-001. Демонстрационное решение\n\n- Status: Proposed\n\n## Context\n\nПричина.\n\n## Alternatives\n\nВариант Б.\n\n## Consequences\n\nЦена.\n",
        )
        .expect("adr");
        git(dir, &["init", "-q"]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// Прогон гейта на фикстуре с паспортом под меткой `кейс`.
    fn passport_of(dir: &Path) -> Passport {
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        Passport::build_labelled(&report, dir, "кейс")
    }

    /// Все 64-символьные hex-строки → `H`: аттестация вердикта привязана к
    /// коммиту базы, а номер коммита песочницы невоспроизводим. Всё остальное
    /// в снапшоте обязано быть точным.
    fn scrub(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut run = String::new();
        for ch in text.chars() {
            if ch.is_ascii_digit() || ch.is_ascii_lowercase() {
                run.push(ch);
                continue;
            }
            flush_run(&mut out, &mut run);
            out.push(ch);
        }
        flush_run(&mut out, &mut run);
        out
    }

    /// Короткая серия hex-символов возвращается как есть (это слова и числа
    /// вердикта), длинная — сворачивается в `H`.
    fn flush_run(out: &mut String, run: &mut String) {
        if run.len() >= 16 {
            out.push('H');
        } else {
            out.push_str(run);
        }
        run.clear();
    }

    /// Страница зелёного кейса целиком, с точностью до пробелов. Снапшот
    /// сознательно буквальный: паспорт — документ, который читают люди
    /// (и агенты), и «примерно так» здесь означало бы, что вердикт и его
    /// описание разошлись.
    const GREEN_PAGE: &str = r"# Паспорт вердикта

**PASS** (exit 0) · маршрут Fast · репозиторий `кейс`  
Маршрут: явный --route Fast

Зелёный здесь означает: механика проверила перечисленное в блоке 1 и нарушений не нашла. Он НЕ означает «выпускать» — блоки 2 и 3 говорят, на что зелёный не распространяется.

## 1. Проверено

- **fitness** \* — PASS — Правил: 2, нарушений: 0 (error: 0, warn: 0); реестр: 2 правил (error: 2), отпечаток d8fef770 — файл: CONSTRAINTS.yaml
- **delta_guard** — PASS — изменённых файлов: 0, защищённых среди них: 0
- **rule_weakened** — PASS — реестр правил не ослаблен относительно HEAD — файл: CONSTRAINTS.yaml
- **spine_lint** \* — PASS — находок: 0 (error: 0)
- **trace_check** — SKIP — нет каталога model/
- **model_validate** — SKIP — нет каталога model/
- **decision_quality** — SKIP — не включена: добавьте 'decision_quality' в [gate.required] нужного маршрута

\* — обязательна для маршрута Fast; остальные — сверх неё.

## 2. Заявлено, но механикой не проверяется

- **fitness** — правил, судящих по ТЕКСТУ файла (наличие/запрет слова), — 2 из 2; они зеленеют и когда инвариант соблюдён, и когда о нём просто написали (исполняемых проверок поведения: 0)

## 3. Не проверено

- **trace_check** — нет каталога model/ (не обязательна для маршрута)
- **model_validate** — нет каталога model/ (не обязательна для маршрута)
- **decision_quality** — не включена: добавьте 'decision_quality' в [gate.required] нужного маршрута (не обязательна для маршрута)

## Аттестация

```
sha256:H
arch-be gate --repo кейс --format json > verdict.json && arch-be gate --repo кейс --verify-envelope verdict.json
```
";

    /// Зелёный кейс: страница целиком — снапшот. Паспорт обязан называть и
    /// проверенное, и границы вердикта, и пропущенное, не становясь при этом
    /// статичной отговоркой. Паспорт обязан называть и
    /// проверенное, и границы вердикта, и пропущенное, не становясь при этом
    /// статичной отговоркой.
    #[test]
    fn passport_snapshot_on_green_case() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_repo(dir, "ARCHITECTURE-SPINE.md");
        let passport = passport_of(dir);
        assert_eq!(passport.outcome, "PASS");
        assert_eq!(passport.exit_code, 0);
        let page = scrub(&passport.render());
        assert_eq!(page, GREEN_PAGE);
    }

    /// Страница красного кейса: тот же формат, но FAIL в блоке 1 — и
    /// вступление говорит о красном, а не о зелёном.
    const RED_PAGE: &str = r"# Паспорт вердикта

**FAIL** (exit 1) · маршрут Fast · репозиторий `кейс`  
Маршрут: явный --route Fast

Красный здесь означает: механика нашла перечисленное в блоке 1. Блоки 2 и 3 говорят, чего не поймала и она, — на это не распространяется и красный.

## 1. Проверено

- **fitness** \* — FAIL — Правил: 2, нарушений: 1 (error: 1, warn: 0); реестр: 2 правил (error: 2), отпечаток d8fef770 — файл: CONSTRAINTS.yaml
  находок: error 1, warn 0
- **delta_guard** — PASS — изменённых файлов: 0, защищённых среди них: 0
- **rule_weakened** — PASS — реестр правил не ослаблен относительно HEAD — файл: CONSTRAINTS.yaml
- **spine_lint** \* — PASS — находок: 0 (error: 0)
- **trace_check** — SKIP — нет каталога model/
- **model_validate** — SKIP — нет каталога model/
- **decision_quality** — SKIP — не включена: добавьте 'decision_quality' в [gate.required] нужного маршрута

\* — обязательна для маршрута Fast; остальные — сверх неё.

## 2. Заявлено, но механикой не проверяется

- **fitness** — правил, судящих по ТЕКСТУ файла (наличие/запрет слова), — 2 из 2; они зеленеют и когда инвариант соблюдён, и когда о нём просто написали (исполняемых проверок поведения: 0)

## 3. Не проверено

- **trace_check** — нет каталога model/ (не обязательна для маршрута)
- **model_validate** — нет каталога model/ (не обязательна для маршрута)
- **decision_quality** — не включена: добавьте 'decision_quality' в [gate.required] нужного маршрута (не обязательна для маршрута)

## Аттестация

```
sha256:H
arch-be gate --repo кейс --format json > verdict.json && arch-be gate --repo кейс --verify-envelope verdict.json
```
";

    /// Красный кейс: та же страница, но FAIL в блоке 1 и причина в блоке 3.
    /// Паспорт не «оправдывает» красный — он его описывает.
    #[test]
    fn passport_snapshot_on_red_case_names_failures() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_repo(dir, "НЕТ-ТАКОГО-ФАЙЛА.md");
        let passport = passport_of(dir);
        assert_eq!(passport.outcome, "FAIL");
        assert_eq!(passport.exit_code, 1);
        let page = scrub(&passport.render());
        assert_eq!(page, RED_PAGE);
        assert!(
            page.contains("Красный здесь означает"),
            "на красном вердикте страница не имеет права говорить о зелёном"
        );
    }

    /// Паспорт описывает ТОТ ЖЕ прогон: аттестация и итог совпадают с
    /// вердиктом, рядом с которым страница печатается.
    #[test]
    fn passport_does_not_change_the_verdict() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_repo(dir, "ARCHITECTURE-SPINE.md");
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            (1, 4),
            &GateRequirements::default(),
        )
        .expect("gate");
        let passport = Passport::build(&report, dir);
        assert_eq!(passport.attestation, report.attestation);
        assert_eq!(passport.outcome, report.outcome.label());
        assert_eq!(passport.exit_code, report.outcome.exit_code());
        assert_eq!(
            passport.checked.len(),
            report.components.len(),
            "блок 1 обязан перечислить все составляющие прогона"
        );
    }

    /// Блок 2 собирается из ФАКТОВ репозитория: нет каталога `docs/adr` и нет
    /// бандла — нет и утверждений о решениях и ревьюерах. Иначе страница
    /// превращается в универсальную отговорку.
    #[test]
    fn passport_claims_only_what_the_repo_actually_contains() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_repo(dir, "ARCHITECTURE-SPINE.md");
        let passport = passport_of(dir);
        assert!(
            !passport.claimed.iter().any(|c| c.source == "решения"),
            "нет docs/adr — нет утверждения о качестве решений: {:?}",
            passport.claimed
        );
        assert!(
            !passport.claimed.iter().any(|c| c.source == "бандл"),
            "нет бандла — нет утверждения о независимости ревьюера: {:?}",
            passport.claimed
        );
    }

    /// Принятый ADR без составляющей `decision_quality` — утверждение
    /// «решения механикой не оценены» появляется и называет выход.
    #[test]
    fn passport_names_unjudged_decisions() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_repo(dir, "ARCHITECTURE-SPINE.md");
        std::fs::create_dir_all(dir.join("docs/adr")).expect("mkdir");
        std::fs::write(
            dir.join("docs/adr/ADR-001-demo.md"),
            "# ADR-001. Решение\n\n- Status: Accepted\n\n## Context\n\nПричина.\n\n## Alternatives\n\nВариант Б.\n\n## Consequences\n\nЦена.\n",
        )
        .expect("adr");
        let passport = passport_of(dir);
        let claim = passport
            .claimed
            .iter()
            .find(|c| c.source == "решения")
            .expect("утверждение о неоценённых решениях");
        assert!(claim.text.contains("decision_quality"), "{claim:?}");
        assert!(
            !claim.instead.is_empty(),
            "у утверждения есть выход: {claim:?}"
        );
    }

    /// Машиночитаемая форма несёт те же три блока и ту же аттестацию — MCP и
    /// CLI не имеют права расходиться.
    #[test]
    fn json_carries_the_same_three_blocks() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_repo(dir, "ARCHITECTURE-SPINE.md");
        let passport = passport_of(dir);
        let json = passport.to_json();
        assert_eq!(json["schema"], "arch-be/verdict-passport/v1");
        assert_eq!(json["verdict"], "PASS");
        assert_eq!(
            json["checked"].as_array().map(Vec::len),
            Some(passport.checked.len())
        );
        assert_eq!(
            json["attestation"].as_str(),
            Some(format!("sha256:{}", passport.attestation).as_str())
        );
    }

    /// `evidence verify` ссылается на паспорт, а не на сам себя.
    #[test]
    fn evidence_verify_hint_points_at_the_gate() {
        let hint = hint_command(Path::new("changes/demo"));
        assert!(
            hint.contains("gate") && hint.contains("--explain"),
            "{hint}"
        );
    }
}

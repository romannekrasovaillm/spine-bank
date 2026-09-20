//! Метаморфный самотест вердикта (П8 ДКА).
//!
//! Тесты проверяют ОТВЕТЫ на входах; этот модуль проверяет СВОЙСТВА ответов:
//! «вердикт не меняется при преобразовании входа, которое не должно его
//! менять» и «более строгий вход не даёт более мягкий вердикт». Именно таких
//! проверок не хватило 0.3.2, где все шесть дефектов зелёного пути нашёл
//! внешний прогон, а не собственные 1200 тестов.
//!
//! Команда `arch-be selftest` собирает изолированную песочницу и прогоняет
//! инварианты; её может запустить и пилотный заказчик на своей машине.

use std::path::{Path, PathBuf};

use crate::control::Route;
use crate::error::Result;
use crate::gate::{self, GateOutcome};

/// Один инвариант вердикта.
#[derive(Debug, Clone)]
pub struct Invariant {
    /// Имя свойства.
    pub name: &'static str,
    /// Пройден.
    pub passed: bool,
    /// Что именно проверено/что сломалось.
    pub detail: String,
}

/// Отчёт самотеста.
#[derive(Debug, Clone)]
pub struct SelftestReport {
    /// Инварианты в порядке прогона.
    pub invariants: Vec<Invariant>,
}

impl SelftestReport {
    /// Все инварианты пройдены.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.invariants.iter().all(|i| i.passed)
    }

    /// Текстовый рендер: `[PASS|FAIL] имя — деталь`, итоговая строка.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "Метаморфный самотест вердикта ({} инвариантов)\n",
                self.invariants.len()
            ),
        );
        for i in &self.invariants {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "  [{}] {} — {}\n",
                    if i.passed { "PASS" } else { "FAIL" },
                    i.name,
                    i.detail
                ),
            );
        }
        let failed = self.invariants.iter().filter(|i| !i.passed).count();
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "Итог: {}\n",
                if failed == 0 {
                    "PASS — свойства вердикта держатся".to_string()
                } else {
                    format!("FAIL — нарушено инвариантов: {failed}")
                }
            ),
        );
        out
    }
}

/// Изолированная песочница самотеста (удаляется на Drop).
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root =
            std::env::temp_dir().join(format!("arch-be-selftest-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join(".arch-handoff"))?;
        std::fs::write(
            root.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )?;
        std::fs::write(root.join("ARCHITECTURE-SPINE.md"), "# Spine\n")?;
        // Git-база: без неё auto-маршрут даёт fail-safe Critical и инвариант
        // храповика проверялся бы тривиально. git недоступен — песочница
        // остаётся, инвариант деградирует до fail-safe ветки.
        let git = |args: &[&str]| {
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        git(&["init", "-q"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.email=selftest@example.invalid",
            "-c",
            "user.name=selftest",
            "commit",
            "-q",
            "-m",
            "selftest fixture",
        ]);
        Ok(Self { root })
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Игнорируем: песочница во временном каталоге, уборка best-effort.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Прогоняет инварианты и возвращает отчёт (никогда не падает: сбой
/// песочницы — FAIL инварианта с причиной).
#[must_use]
pub fn run() -> SelftestReport {
    let mut invariants = Vec::new();
    let Ok(fx) = Fixture::new() else {
        return SelftestReport {
            invariants: vec![Invariant {
                name: "fixture",
                passed: false,
                detail: "не удалось создать песочницу во временном каталоге".into(),
            }],
        };
    };
    let limits = (1usize, 4usize);
    let mut record = |name: &'static str, passed: bool, detail: String| {
        invariants.push(Invariant {
            name,
            passed,
            detail,
        });
    };

    // 1. Монотонность по маршруту: Critical-прогон без входа обязан дать
    //    INCOMPLETE (exit 3), а не зелёный PASS. Это Д1.
    match gate::run(fx.path(), Some(Route::Critical), None, None, limits) {
        Ok(r) => {
            let ok = r.outcome == GateOutcome::Incomplete && r.outcome.exit_code() == 3;
            record(
                "route_monotonicity_incomplete_not_green",
                ok,
                format!(
                    "Critical без model/evidence: итог {}, exit {}, не проверено: [{}]",
                    r.outcome.label(),
                    r.outcome.exit_code(),
                    r.not_checked.join(", ")
                ),
            );
        }
        Err(e) => record(
            "route_monotonicity_incomplete_not_green",
            false,
            format!("сбой: {e}"),
        ),
    }

    // 2. Зелёный возможен: на лёгком маршруте с полным входом гейт PASS.
    match gate::run(fx.path(), Some(Route::Fast), None, None, limits) {
        Ok(r) => record(
            "green_is_reachable",
            r.outcome == GateOutcome::Pass && r.outcome.exit_code() == 0,
            format!("Fast на чистом входе: итог {}", r.outcome.label()),
        ),
        Err(e) => record("green_is_reachable", false, format!("сбой: {e}")),
    }

    // 3. Чувствительность: засеянный дефект краснит гейт.
    let _ = std::fs::write(fx.path().join("bad.py"), "PAN = '4111 1111 1111 1111'\n");
    match gate::run(fx.path(), Some(Route::Fast), None, None, limits) {
        Ok(r) => record(
            "sensitivity_seeded_defect_reddens",
            r.outcome == GateOutcome::Fail && r.outcome.exit_code() == 1,
            format!("засеянный must_not_contain: итог {}", r.outcome.label()),
        ),
        Err(e) => record(
            "sensitivity_seeded_defect_reddens",
            false,
            format!("сбой: {e}"),
        ),
    }

    // 4. Храповик маршрута: ROUTE.lock не даёт чистому дереву «сползти» на Fast.
    let _ = std::fs::write(
        fx.path().join(".arch-handoff/ROUTE.lock"),
        "route: critical\ndecided_by: ADR-999\n",
    );
    match gate::run(fx.path(), None, None, None, limits) {
        Ok(r) => record(
            "route_ratchet_lock_floor",
            // Пол маршрута соблюдён: либо ROUTE.lock поднял (git-база есть),
            // либо auto уже дал fail-safe Critical (git недоступен).
            r.route == Route::Critical
                && (r.route_note.contains("ROUTE.lock") || r.route_note.contains("fail-safe")),
            format!(
                "auto на чистом дереве: маршрут {} ({})",
                r.route, r.route_note
            ),
        ),
        Err(e) => record("route_ratchet_lock_floor", false, format!("сбой: {e}")),
    }

    // 5. Идемпотентность: два прогона без изменений — один конверт.
    let first = gate::run(fx.path(), Some(Route::Fast), None, None, limits);
    let second = gate::run(fx.path(), Some(Route::Fast), None, None, limits);
    match (first, second) {
        (Ok(a), Ok(b)) => record(
            "idempotency_same_verdict",
            a.outcome == b.outcome && a.attestation == b.attestation,
            format!(
                "аттестация {} == {}",
                &a.attestation[..12],
                &b.attestation[..12]
            ),
        ),
        _ => record("idempotency_same_verdict", false, "сбой прогона".into()),
    }

    // 6. Путь не меняет вердикт: evidence verify «.» и абсолютный путь.
    for (rel, body) in [
        ("PROBLEM.md", "# Проблема\n"),
        ("SPEC.md", "## Критерии приёмки\n"),
        ("RISK.md", "Fast\n"),
        ("ROLLBACK.md", "git revert\n"),
    ] {
        let _ = std::fs::write(fx.path().join(rel), body);
    }
    let packed = crate::evidence::pack(fx.path(), Route::Fast);
    let abs = fx.path().canonicalize().ok();
    match (packed, abs) {
        (Ok(_), Some(abs)) => {
            let a = crate::evidence::verify(fx.path());
            let b = crate::evidence::verify(&abs);
            match (a, b) {
                (Ok(a), Ok(b)) => record(
                    "path_invariance_evidence",
                    a.passed == b.passed && a.tampered == b.tampered && a.missing == b.missing,
                    format!(
                        "'.' и абсолютный путь: подмен {} / {}",
                        a.tampered.len(),
                        b.tampered.len()
                    ),
                ),
                _ => record("path_invariance_evidence", false, "сбой verify".into()),
            }
        }
        _ => record("path_invariance_evidence", false, "сбой pack".into()),
    }

    // 7. Чувствительность аттестации (Н3, ADR-043): вердикт тот же, а
    //    состояние репозитория другое — аттестация обязана это показать.
    //    Раньше входом был только реестр правил, и вердикт «удостоверял» не
    //    то дерево, на котором получен.
    let git_commit = |msg: &str| {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(fx.path())
            .args([
                "-c",
                "user.email=selftest@example.invalid",
                "-c",
                "user.name=selftest",
            ])
            .args(["add", "-A"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(fx.path())
            .args([
                "-c",
                "user.email=selftest@example.invalid",
                "-c",
                "user.name=selftest",
            ])
            .args(["commit", "-q", "-m", msg])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    // Правка коммитится: иначе её поймал бы delta guard и сменился бы не
    // вход, а статус составляющей — инвариант проверял бы не то.
    std::fs::create_dir_all(fx.path().join("model")).ok();
    let entity = |note: &str| {
        format!(
            "---\nid: CMP-001\ntype: cmp\ntitle: \"Компонент\"\nstatus: \"designed\"\ndepends_on: []\n---\n\n{note}\n"
        )
    };
    std::fs::write(
        fx.path().join("model/CMP-001.md"),
        entity("Первая редакция."),
    )
    .ok();
    git_commit("model v1");
    let before = gate::run(fx.path(), Some(Route::Fast), None, None, limits);
    std::fs::write(
        fx.path().join("model/CMP-001.md"),
        entity("Вторая редакция: решение уточнено."),
    )
    .ok();
    git_commit("model v2");
    let after = gate::run(fx.path(), Some(Route::Fast), None, None, limits);
    match (before, after) {
        (Ok(a), Ok(b)) => record(
            "attestation_sensitivity",
            a.outcome == b.outcome && a.attestation != b.attestation,
            format!(
                "итог {} → {}, аттестация {} → {}",
                a.outcome.label(),
                b.outcome.label(),
                &a.attestation[..12],
                &b.attestation[..12]
            ),
        ),
        _ => record("attestation_sensitivity", false, "сбой прогона".into()),
    }

    // 8. T-01 (0.3.4): хук не должен угадывать, ГДЕ лежит реестр правил.
    //    Шаблон с гардой `[ -f .arch-handoff/CONSTRAINTS.yaml ]` молчал на
    //    кейсе, собранном `bootstrap` (реестр в корне), — гейт не вызывался
    //    вовсе. Свойство проверяется на СГЕНЕРИРОВАННОМ хуке: контур,
    //    который в 0.3.5 починили в шаблоне, обязан остаться починенным.
    let hooks_dir = fx.path().join("hooks-case");
    let _ = std::fs::create_dir_all(&hooks_dir);
    let connect_opts = crate::connect::ConnectOptions::new(crate::connect::Host::Claude, hooks_dir.clone());
    match crate::connect::connect(&connect_opts) {
        Ok(_) => {
            let settings = std::fs::read_to_string(hooks_dir.join(".claude/settings.json"))
                .unwrap_or_default();
            let guarded = settings.contains("[ -f .arch-handoff/CONSTRAINTS.yaml ]")
                || settings.contains("[ -f CONSTRAINTS.yaml ]");
            let calls_gate = settings.contains("gate");
            record(
                "hook_does_not_guess_registry_location",
                !guarded && calls_gate,
                format!(
                    "хук: гарда по расположению реестра {}, вызов гейта {}",
                    if guarded { "есть" } else { "нет" },
                    if calls_gate { "есть" } else { "НЕТ" }
                ),
            );
        }
        Err(e) => record(
            "hook_does_not_guess_registry_location",
            false,
            format!("connect не отработал: {e}"),
        ),
    }

    // 9. T-02 (0.3.4): две копии реестра не переключают гейт молча. Копии
    //    расходятся — это находка error, а не пометка в тексте PASS-строки.
    let two = fx.path().join("two-registries");
    let _ = std::fs::create_dir_all(two.join(".arch-handoff"));
    let rule = |name: &str| {
        format!("rules:\n  - name: {name}\n    type: file_exists\n    path: \"docs/ARCHITECTURE-SPINE.md\"\n    severity: error\n")
    };
    let _ = std::fs::write(two.join("CONSTRAINTS.yaml"), rule("root_rule"));
    let _ = std::fs::write(
        two.join(".arch-handoff/CONSTRAINTS.yaml"),
        format!("{}{}", rule("pack_rule"), rule("pack_rule_two")),
    );
    match gate::run(&two, None, None, None, limits) {
        Ok(r) => {
            let diverged = r.components.iter().any(|c| {
                c.findings
                    .iter()
                    .any(|f| f.rule.as_deref() == Some("registry_diverged"))
            });
            record(
                "divergent_registries_are_a_finding",
                diverged && r.outcome != GateOutcome::Pass,
                format!(
                    "две копии реестра: итог {}, находка registry_diverged {}",
                    r.outcome.label(),
                    if diverged { "есть" } else { "НЕТ" }
                ),
            );
        }
        Err(e) => record(
            "divergent_registries_are_a_finding",
            false,
            format!("сбой: {e}"),
        ),
    }

    // 10. T-03 (0.3.4): база диффа нормализуется один раз. Шаблоны передавали
    //     `A...HEAD`, гейт дописывал `...HEAD` второй раз — диапазон не
    //     вычислялся, и вердикт уходил в fail-safe Critical молча.
    let plain = crate::control::normalize_base_range("origin/main");
    let ranged = crate::control::normalize_base_range("origin/main...HEAD");
    let twice = crate::control::normalize_base_range(&ranged);
    record(
        "base_range_is_normalized_once",
        plain == "origin/main...HEAD" && ranged == plain && twice == plain,
        format!(
            "'origin/main' → {plain}; 'origin/main...HEAD' → {ranged}; повтор → {twice}"
        ),
    );

    SelftestReport { invariants }
}

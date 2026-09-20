//! Мутационное тестирование архитектурного пакета (`arch-be redteam`, W2 0.3.4).
//!
//! Отвечает на вопрос, который архитектор не может задать вручную: **насколько
//! мой пакет вообще защищён правилами?** Команда клонирует кейс во временный
//! каталог, засеивает по одному дефекту из каталога [`MUTATORS`], гоняет гейт и
//! печатает карту обнаружения: поймано гейтом / не поймано никем — с итоговой
//! долей.
//!
//! Свойства, которые обязан держать инструмент:
//! - **read-only к исходному кейсу** — работает только с копией во временном
//!   каталоге (удаляется на `Drop`);
//! - **без сети** и без LLM: только детерминированный контур контроля (AD-2);
//! - **детерминированность** — порядок мутаторов фиксирован, вердикт не
//!   зависит от времени и абсолютных путей;
//! - **честность** — дефекты, которые механика не должна ловить (семантика
//!   решения), названы такими в отчёте, а не спрятаны в знаменатель.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::control::Route;
use crate::error::{HarnessError, Result};
use crate::gate::{self, GateOptions, GateOutcome, GateReport, GateRequirements, GateStatus};

/// Что ожидается от мутатора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expectation {
    /// Дефект обязан быть пойман механикой.
    Caught,
    /// Дефект механикой не ловится и не должен — семантика решения
    /// (человеческое ревью, паспорт вердикта), а не проверка правил.
    Semantic,
    /// Контрольный мутатор: вердикт обязан остаться зелёным, но аттестация —
    /// измениться (иначе «зелёный» не привязан к состоянию).
    Control,
}

impl Expectation {
    /// Метка для отчёта.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Caught => "ловится",
            Self::Semantic => "семантика",
            Self::Control => "контроль",
        }
    }
}

/// Правка кейса-мутанта: `Ok(())` — применена, `Err(причина)` — вход не
/// найден (мутатор пропускается, а не считается «пойманным»).
pub type Mutation = fn(&Path) -> std::result::Result<(), String>;

/// Смысловой субъект мутанта: какую рубрику и какое досье брать, чтобы
/// дефект, не ловимымй механикой, измерил судья (ADR-051, S5).
///
/// Субъект задан парой «каталог + префикс имени», а не готовым путём: путь
/// зависит от кейса, а каталог мутанта — копия кейса. Резолвер берёт первый
/// подходящий файл в отсортированном порядке, поэтому прогон детерминирован.
pub struct SemanticSubject {
    /// Имя смысловой рубрики (файл в `assets/rubrics`).
    pub rubric: &'static str,
    /// Вид досье, которым собирается вход судьи.
    pub pack: crate::rubric_pack::PackKind,
    /// Каталог субъекта внутри кейса (`docs/adr`, `model`, `src/legacy`).
    pub dir: &'static str,
    /// Префикс имени файла субъекта (`ADR-`, `CMP-`, `payments.py`).
    pub prefix: &'static str,
}

/// Один мутатор: идентификатор, описание, ожидание и правка.
pub struct Mutator {
    /// Идентификатор из red-team набора (`D1`, `D11b`, `R`…).
    pub id: &'static str,
    /// Что засеваем.
    pub title: &'static str,
    /// Каким инструментом обязан ловиться (человеко-читаемая подсказка).
    pub by: &'static str,
    /// Ожидание.
    pub expected: Expectation,
    /// Входит ли мутатор в знаменатель доли обнаружения — в набор из 14
    /// позиций раздела 7 ТЗ (D1…D13 + D11b). `R` (ревью `NOT-READY`) и `D14`
    /// (контроль аттестации) стоят в таблице отдельными строками: их результат
    /// виден в карте обнаружения, но в критерий приёмки «≥ 11 из 14» не входит.
    pub in_ratio: bool,
    /// Смысловая рубрика, которой этот класс дефекта ловится (ADR-051, S5):
    /// у `D6`, `D10`, `D11` механика бессильна по построению, и измерение
    /// смыслового слоя — отдельная строка, в долю обнаружения не входящая.
    pub semantic: Option<SemanticSubject>,
    /// Правка кейса-мутанта.
    pub apply: Mutation,
}

/// Каталог мутаторов — раздел 7 задания 0.3.4. Порядок фиксирован: прогон
/// детерминирован.
///
/// Дефекты `D1…D13`, `D11b` образуют набор из 14 позиций раздела 7; `R`
/// (ревью `NOT-READY`) и `D14` (контроль аттестации) идут отдельными строками
/// и в долю обнаружения не входят — иначе она была бы несопоставима с
/// критерием приёмки релиза.
pub const MUTATORS: [Mutator; 16] = [
    Mutator {
        id: "D1",
        title: "бюджет hop'а больше цели p99",
        by: "nfr (budget)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d1,
    },
    Mutator {
        id: "D2",
        title: "цель доступности недостижима",
        by: "nfr (availability)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d2,
    },
    Mutator {
        id: "D3",
        title: "дефицит ёмкости",
        by: "nfr (capacity)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d3,
    },
    Mutator {
        id: "D4",
        title: "инвариант без правила",
        by: "trace_check",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d4,
    },
    Mutator {
        id: "D5",
        title: "битая ссылка модели",
        by: "model_validate",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d5,
    },
    Mutator {
        id: "D6",
        title: "ссылка «не на ту» сущность",
        by: "— (семантика ссылок)",
        expected: Expectation::Semantic,
        in_ratio: true,
        semantic: Some(SemanticSubject {
            rubric: "model_link_semantics",
            pack: crate::rubric_pack::PackKind::EntityLinks,
            dir: "model",
            prefix: "CMP-",
        }),
        apply: mutate_d6,
    },
    Mutator {
        id: "D7",
        title: "ослабление правила, закоммичено",
        by: "rule_weakened + база",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d7,
    },
    Mutator {
        id: "D8",
        title: "ADR без секции альтернатив",
        by: "fitness",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d8,
    },
    Mutator {
        id: "D9",
        title: "«картонный» ADR, секции есть",
        by: "decision_quality",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d9,
    },
    Mutator {
        id: "D10",
        title: "решение противоречит инварианту, слово на месте",
        by: "— (смысл решения)",
        expected: Expectation::Semantic,
        in_ratio: true,
        semantic: Some(SemanticSubject {
            rubric: "adr_spine_consistency",
            pack: crate::rubric_pack::PackKind::AdrVsSpine,
            dir: "docs/adr",
            prefix: "ADR-",
        }),
        apply: mutate_d10,
    },
    Mutator {
        id: "D11",
        title: "код нарушает инвариант, правил на код нет",
        by: "— (кандидат executable-invariants)",
        expected: Expectation::Semantic,
        in_ratio: true,
        semantic: Some(SemanticSubject {
            rubric: "code_invariant_conformance",
            pack: crate::rubric_pack::PackKind::CodeVsSpine,
            dir: "src/legacy",
            prefix: "payments.py",
        }),
        apply: mutate_d11,
    },
    Mutator {
        id: "D11b",
        title: "то же + правило на тесты",
        by: "fitness",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: Some(SemanticSubject {
            rubric: "code_invariant_conformance",
            pack: crate::rubric_pack::PackKind::CodeVsSpine,
            dir: "tests",
            prefix: "payments_test.py",
        }),
        apply: mutate_d11b,
    },
    Mutator {
        id: "D12",
        title: "NFR без способа проверки",
        by: "model_validate (Critical)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d12,
    },
    Mutator {
        id: "D13",
        title: "DECISION.md = «TODO»",
        by: "evidence (Н1)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        apply: mutate_d13,
    },
    Mutator {
        id: "R",
        title: "ревью NOT-READY в бандле",
        by: "evidence (Н1)",
        expected: Expectation::Caught,
        in_ratio: false,
        semantic: None,
        apply: mutate_r,
    },
    Mutator {
        id: "D14",
        title: "контроль: безвредная правка",
        by: "аттестация ≠ эталон, вердикт PASS",
        expected: Expectation::Control,
        in_ratio: false,
        semantic: None,
        apply: mutate_d14,
    },
];

// ---------------------------------------------------------------------------
// Правки кейса-мутанта
// ---------------------------------------------------------------------------

/// Читает файл мутанта.
fn read(root: &Path, rel: &str) -> std::result::Result<String, String> {
    std::fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))
}

/// Пишет файл мутанта.
fn write(root: &Path, rel: &str, text: &str) -> std::result::Result<(), String> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{rel}: {e}"))
}

/// Файлы каталога по префиксу имени (`model/NFR-` → все `NFR-*.md`),
/// в детерминированном порядке.
fn files_with_prefix(root: &Path, dir: &str, prefix: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root.join(dir)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with(prefix)
                && e.path().extension().is_some_and(|x| x == "md")
        })
        .map(|e| format!("{dir}/{}", e.file_name().to_string_lossy()))
        .collect();
    out.sort();
    out
}

/// Заменяет значение поля frontmatter (`field: …`) во всех файлах каталога.
/// Возвращает число изменённых файлов.
fn set_field_everywhere(
    root: &Path,
    dir: &str,
    prefix: &str,
    field: &str,
    value: &str,
) -> std::result::Result<usize, String> {
    let files = files_with_prefix(root, dir, prefix);
    if files.is_empty() {
        return Err(format!("нет файлов {dir}/{prefix}*.md"));
    }
    let mut changed = 0usize;
    for rel in &files {
        let text = read(root, rel)?;
        let mut found = false;
        let new: Vec<String> = text
            .lines()
            .map(|l| {
                if l.trim_start().starts_with(&format!("{field}:")) {
                    found = true;
                    format!("{field}: {value}")
                } else {
                    l.to_string()
                }
            })
            .collect();
        if found {
            write(root, rel, &format!("{}\n", new.join("\n")))?;
            changed += 1;
        }
    }
    if changed == 0 {
        return Err(format!("поле {field} не найдено в {dir}/{prefix}*.md"));
    }
    Ok(changed)
}

/// Заменяет поле в первом файле каталога по префиксу.
fn set_field_first(
    root: &Path,
    dir: &str,
    prefix: &str,
    field: &str,
    value: &str,
) -> std::result::Result<(), String> {
    let files = files_with_prefix(root, dir, prefix);
    let Some(rel) = files.first() else {
        return Err(format!("нет файлов {dir}/{prefix}*.md"));
    };
    let text = read(root, rel)?;
    if !text
        .lines()
        .any(|l| l.trim_start().starts_with(&format!("{field}:")))
    {
        return Err(format!("{rel}: нет поля {field}"));
    }
    let new: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with(&format!("{field}:")) {
                format!("{field}: {value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    write(root, rel, &format!("{}\n", new.join("\n")))
}

/// D1: бюджет hop'а больше цели p99 — сумма бюджетов INT превышает цель NFR.
fn mutate_d1(root: &Path) -> std::result::Result<(), String> {
    let target = nfr_target(root, "p99_target_ms")?;
    set_field_first(
        root,
        "model",
        "INT-",
        "latency_budget_ms",
        &format!("{}", target + 500.0),
    )
}

/// D2: цель доступности недостижима — доступность компонентов ниже цели.
fn mutate_d2(root: &Path) -> std::result::Result<(), String> {
    nfr_target(root, "availability_target")?;
    let n = set_field_everywhere(root, "model", "CMP-", "availability", "0.90")?;
    if n == 0 {
        return Err("нет CMP с полем availability".to_string());
    }
    Ok(())
}

/// D3: дефицит ёмкости — инстансов и RPS на инстанс не хватает на цель.
fn mutate_d3(root: &Path) -> std::result::Result<(), String> {
    nfr_target(root, "rps_target")?;
    set_field_everywhere(root, "model", "CMP-", "instances", "1")?;
    set_field_everywhere(root, "model", "CMP-", "rps_per_instance", "1").map(|_| ())
}

/// D4: инвариант без правила — новый AD в спайне, не покрытый реестром.
fn mutate_d4(root: &Path) -> std::result::Result<(), String> {
    let spine = read(root, "ARCHITECTURE-SPINE.md")?;
    let extra = "\n## AD-099 Незалогированное решение\n\n\
                 - Binds: раскрытие состава выплаты\n\
                 - Prevents: утечка персональных данных в журнал\n\
                 - Rule: правило обязано быть в CONSTRAINTS.yaml\n";
    write(root, "ARCHITECTURE-SPINE.md", &format!("{spine}{extra}"))
}

/// D5: битая ссылка модели — `depends_on` на несуществующую сущность.
fn mutate_d5(root: &Path) -> std::result::Result<(), String> {
    let files = files_with_prefix(root, "model", "CMP-");
    let rel = files
        .iter()
        .find(|rel| read(root, rel).is_ok_and(|t| t.lines().any(|l| l.starts_with("depends_on:"))))
        .ok_or_else(|| "нет CMP с depends_on".to_string())?;
    let text = read(root, rel)?;
    let new: Vec<String> = text
        .lines()
        .map(|l| {
            if l.starts_with("depends_on:") {
                "depends_on: [CMP-999]".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    write(root, rel, &format!("{}\n", new.join("\n")))
}

/// D6: ссылка «не на ту» сущность — связь остаётся валидной по ссылке, но
/// ведёт не туда. Механика этого не видит и не должна: разбирать смысл связи
/// может только человек.
fn mutate_d6(root: &Path) -> std::result::Result<(), String> {
    // Ссылка обязана остаться РАЗРЕШИМОЙ: дефект в том, что она ведёт не туда,
    // а не в том, что её нет.
    // Идентификатор — из имени файла: `CMP-002-журнал-операций.md` → `CMP-002`.
    let ids: Vec<String> = files_with_prefix(root, "model", "CMP-")
        .iter()
        .filter_map(|f| {
            let name = Path::new(f).file_name()?.to_string_lossy().into_owned();
            let parts: Vec<&str> = name.split('-').take(2).collect();
            (parts.len() == 2).then(|| parts.join("-"))
        })
        .collect();
    for rel in files_with_prefix(root, "model", "CMP-") {
        let text = read(root, rel.as_str())?;
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("depends_on: [") else {
                continue;
            };
            let Some(first) = rest.split(',').next() else {
                continue;
            };
            let first = first.trim();
            if first.is_empty() {
                continue;
            }
            // Чужая сущность: не сама сущность (иначе `dependency-cycle` —
            // это другой дефект, D6 не о нём) и не та, что уже в списке.
            let own: Vec<String> = Path::new(rel.as_str())
                .file_name()
                .map(|n| {
                    n.to_string_lossy()
                        .split('-')
                        .take(2)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let own = own.join("-");
            let other = ids
                .iter()
                .find(|id| id.as_str() != own && id.as_str() != first)
                .ok_or_else(|| "нет второй сущности".to_string())?;
            let new = text.replacen(
                &format!("depends_on: [{first}"),
                &format!("depends_on: [{other}"),
                1,
            );
            if new != text {
                return write(root, rel.as_str(), &new);
            }
        }
    }
    Err("нет CMP с depends_on".to_string())
}

/// D7: ослабление правила, закоммичено — severity понижается.
///
/// T-02: мутируется ТОТ реестр, который читает гейт, — им резолвер выбирает
/// пакетную копию (`.arch-handoff/CONSTRAINTS.yaml`) и лишь затем корневую.
/// Раньше предпочтение отдавалось корневому файлу: при двух копиях мутант
/// ослаблял не тот реестр, гейт этого не видел, и D7 считался не пойманным —
/// «дыра в защите» на ровном месте.
fn mutate_d7(root: &Path) -> std::result::Result<(), String> {
    let resolved = crate::control::resolve_constraints_path(root, None);
    let rel = resolved
        .as_deref()
        .and_then(|p| p.strip_prefix(root).ok())
        .map_or_else(
            || ".arch-handoff/CONSTRAINTS.yaml".to_string(),
            |p| p.display().to_string(),
        );
    let rel = rel.as_str();
    let text = read(root, rel)?;
    let mut lines: Vec<String> = Vec::new();
    let mut weakened = false;
    let mut in_rule = false;
    for line in text.lines() {
        if line.trim_start().starts_with("- id:") || line.trim_start().starts_with("- name:") {
            in_rule = true;
        }
        if in_rule && !weakened && line.trim_start().starts_with("severity:") {
            lines.push(line.replace("error", "warn").replace("critical", "warn"));
            weakened = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !weakened {
        return Err(format!("{rel}: нет правила с severity"));
    }
    write(root, rel, &format!("{}\n", lines.join("\n")))
}

/// D8: ADR без секции альтернатив — секция удаляется.
fn mutate_d8(root: &Path) -> std::result::Result<(), String> {
    let files = files_with_prefix(root, "docs/adr", "ADR-");
    for rel in &files {
        let text = read(root, rel.as_str())?;
        if let Some(pos) = text
            .find("## Alternatives")
            .or_else(|| text.find("## Альтернативы"))
        {
            let head = &text[..pos];
            let tail = &text[pos..];
            // Вырезаем секцию до следующего `## ` или конца файла.
            let body_start = tail.find('\n').map_or(tail.len(), |i| i + 1);
            let rest = &tail[body_start..];
            let cut = rest.find("\n## ").map_or(rest.len(), |i| i + 1);
            let new = format!("{head}{}", &rest[cut..]);
            if new.len() < 200 {
                return Err("после удаления секции ADR стал пустышкой".to_string());
            }
            return write(root, rel, &new);
        }
    }
    Err("нет ADR с секцией альтернатив".to_string())
}

/// D9: «картонный» ADR — секции на месте, содержания нет. Поймать это может
/// только оценка качества: механика секций не различает «написано» и «сказано
/// словами».
fn mutate_d9(root: &Path) -> std::result::Result<(), String> {
    let files = files_with_prefix(root, "docs/adr", "ADR-");
    let Some(rel) = files.first() else {
        return Err("нет ADR".to_string());
    };
    let text = read(root, rel)?;
    let header: Vec<&str> = text.lines().take_while(|l| !l.starts_with("## ")).collect();
    let cardboard = format!(
        "{}\n\
         ## Context\n\n\
         Контекст описан в общих чертах, детали раскрываются при необходимости и \
         уточняются по ходу работ. Содержательных свидетельств здесь нет, но \
         формально раздел заполнен и заглушек не содержит, поэтому механическая \
         проверка секций его пропускает. Именно на этом различии и построен \
         дефект: секции есть, решения нет.\n\
         \n\
         ## Decision\n\n\
         Решение принято, детали приведены выше и не требуют дополнительных \
         пояснений. Формулировка нейтральна и не содержит ни одного \
         проверяемого утверждения, которое можно было бы сверить с моделью.\n\
         \n\
         ## Alternatives\n\n\
         Альтернативы рассматривались на этапе обсуждения и были отклонены по \
         причинам, изложенным в протоколе встречи, который здесь не приводится.\n\
         \n\
         ## Consequences\n\n\
         Последствия ожидаются в пределах допустимого; при отклонении от \
         ожиданий решение будет пересмотрено в установленном порядке.\n",
        header.join("\n")
    );
    write(root, rel, &cardboard)
}

/// D10: решение противоречит инварианту, слово на месте — приписываем ADR
/// фразу, противоречащую спайну. Механика обязана этого не замечать.
fn mutate_d10(root: &Path) -> std::result::Result<(), String> {
    let files = files_with_prefix(root, "docs/adr", "ADR-");
    let Some(rel) = files.first() else {
        return Err("нет ADR".to_string());
    };
    let text = read(root, rel)?;
    let extra = "\nДополнительно: персональные данные клиента допускается \
                 выгружать в журнал приложения для упрощения разбора инцидентов.\n";
    write(root, rel.as_str(), &format!("{text}{extra}"))?;
    // Решение переоценено судьёй после правки — иначе дефект поймал бы
    // `rubric_report_stale`, то есть ПРИВЯЗКА ОТЧЁТА К СОДЕРЖИМОМУ, а не
    // понимание смысла. Противоречие инварианту остаётся невидимым механике.
    refresh_rubric_report(root, rel.as_str())
}

/// Обновляет `target_sha256` отчёта рубрики под текущее содержимое документа
/// (имитация повторного прогона судьи после правки).
fn refresh_rubric_report(root: &Path, adr_rel: &str) -> std::result::Result<(), String> {
    let stem = Path::new(adr_rel)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let rel = format!("{}/{stem}.json", crate::rubric::RUBRIC_REPORTS_DIR);
    let text = read(root, rel.as_str())?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{rel}: не JSON: {e}"))?;
    let sha = crate::hash::sha256_file(&root.join(adr_rel))
        .ok_or_else(|| format!("{adr_rel}: не читается"))?;
    value["target_sha256"] = serde_json::Value::String(sha);
    let out = serde_json::to_string_pretty(&value).map_err(|e| format!("{rel}: {e}"))?;
    write(root, rel.as_str(), &format!("{out}\n"))
}

/// D11: код нарушает инвариант, правил на код нет — нарушение кладём туда,
/// куда не смотрит ни одно правило реестра.
fn mutate_d11(root: &Path) -> std::result::Result<(), String> {
    write(
        root,
        "src/legacy/payments.py",
        "# Наследие: прямой доступ к карточным данным.\n\
         PAN = '4111 1111 1111 1111'\n\
         def charge(pan):\n    return pan\n",
    )
}

/// D11b: то же нарушение, но в каталоге, покрытом правилом реестра.
fn mutate_d11b(root: &Path) -> std::result::Result<(), String> {
    write(
        root,
        "tests/payments_test.py",
        "# Тест: нарушение инварианта «без PAN в коде».\n\
         PAN = '4111 1111 1111 1111'\n\
         def test_charge():\n    assert PAN\n",
    )
}

/// D12: NFR без способа проверки — поле `verification` очищается.
fn mutate_d12(root: &Path) -> std::result::Result<(), String> {
    set_field_first(root, "model", "NFR-", "verification", "\"\"")
}

/// D13: `DECISION.md` = «TODO» — запись решения не написана.
fn mutate_d13(root: &Path) -> std::result::Result<(), String> {
    if !root.join("DECISION.md").is_file() && !root.join("docs/DECISION.md").is_file() {
        return Err("нет DECISION.md".to_string());
    }
    let rel = if root.join("DECISION.md").is_file() {
        "DECISION.md"
    } else {
        "docs/DECISION.md"
    };
    write(root, rel, "TODO\n")
}

/// R: ревью в бандле с вердиктом `NOT-READY`.
fn mutate_r(root: &Path) -> std::result::Result<(), String> {
    let rel = ["docs/REVIEW.md", "REVIEW.md", "reports/review.md"]
        .iter()
        .find(|rel| root.join(rel).is_file())
        .ok_or_else(|| "нет файла ревью в бандле".to_string())?;
    let text = read(root, rel)?;
    let replaced = if text.contains("VERDICT: READY") {
        text.replace("VERDICT: READY", "VERDICT: NOT-READY")
    } else {
        format!(
            "# Состязательное ревью\n\n\
             Ревьюер нашёл расхождения, требующие решения до выпуска. Замечания \
             касаются обработки повторных списаний и полноты журнала: описанные \
             сценарии не покрывают повторный приход уведомления после таймаута, \
             а раздел про идемпотентность не отвечает на него вовсе. Пока эти \
             вопросы не разобраны, выпуск не подтверждается.\n\n\
             VERDICT: NOT-READY\n\n{text}"
        )
    };
    write(root, rel, &replaced)
}

/// D14 (контроль): безвредная правка ТЕЛА сущности — вердикт обязан остаться
/// зелёным, но аттестация обязана измениться.
fn mutate_d14(root: &Path) -> std::result::Result<(), String> {
    let files = files_with_prefix(root, "model", "CMP-");
    let Some(rel) = files.first() else {
        return Err("нет CMP".to_string());
    };
    let text = read(root, rel)?;
    write(
        root,
        rel,
        &format!("{text}\nУточнение формулировки без смены решения.\n"),
    )
}

/// Числовое поле цели NFR из первого файла, где оно есть.
fn nfr_target(root: &Path, field: &str) -> std::result::Result<f64, String> {
    for rel in files_with_prefix(root, "model", "NFR-") {
        let text = read(root, rel.as_str())?;
        for line in text.lines() {
            if let Some(rest) = line.trim_start().strip_prefix(&format!("{field}:")) {
                if let Ok(v) = rest.trim().parse::<f64>() {
                    return Ok(v);
                }
            }
        }
    }
    Err(format!("нет NFR с полем {field}"))
}

// ---------------------------------------------------------------------------
// Прогон
// ---------------------------------------------------------------------------

/// Результат одного мутатора.
#[derive(Debug, Clone)]
pub struct Detection {
    /// Идентификатор мутатора.
    pub id: String,
    /// Что засевали.
    pub title: String,
    /// Ожидание.
    pub expected: Expectation,
    /// Кем поймано: имена проваленных составляющих гейта; `None` — не поймано.
    pub caught_by: Option<String>,
    /// Почему мутатор пропущен (вход не найден) — честная причина вместо
    /// молчаливого «поймано».
    pub skipped: Option<String>,
    /// Каким инструментом ожидался (для строки «НЕ ПОЙМАН»).
    pub expected_by: String,
    /// Входит ли в набор из 14 позиций критерия приёмки.
    pub in_ratio: bool,
}

impl Detection {
    /// Пойман ли дефект.
    #[must_use]
    pub fn caught(&self) -> bool {
        self.caught_by.is_some()
    }
}

/// Отчёт мутационного прогона.
#[derive(Debug, Clone)]
pub struct RedteamReport {
    /// Кейс, который мутировали.
    pub case: PathBuf,
    /// Результаты в порядке каталога.
    pub detections: Vec<Detection>,
    /// Порог доли обнаружения, ниже которого прогон красный.
    pub min_detection: f64,
    /// Контрольный мутатор D14: аттестация изменилась при том же вердикте.
    pub control_ok: bool,
    /// Клоны смысловых мутантов, сохранённые `--keep-semantic` (ADR-051, S5):
    /// в них хост кладёт отчёты судьи, их читает `semantic-score`.
    pub semantic_kept: Vec<PathBuf>,
}

/// Сохранённый итог мутационного прогона (`.arch-handoff/redteam.json`,
/// пишет `redteam --save`): метрика доверия (`crate::trust`) читает ИЗМЕРЕННУЮ
/// долю, а не пересказ о ней — пересчитывать прогон при каждом `trust` было бы
/// и медленно, и нечестно (кейс мог измениться после измерения).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RedteamSummary {
    /// Схема файла.
    pub schema: String,
    /// Кейс, на котором измерено (подпись, в какой он был редакции).
    pub case: String,
    /// Момент измерения (RFC 3339).
    pub measured_at: String,
    /// Поймано дефектов (числитель доли).
    pub caught: usize,
    /// Дефектов в знаменателе.
    pub total: usize,
    /// Доля обнаружения `0..=1`.
    pub ratio: f64,
    /// Порог, при котором прогон считался пройденным.
    pub min_detection: f64,
    /// Контрольный мутатор: аттестация изменилась при том же вердикте.
    pub control_ok: bool,
}

impl RedteamSummary {
    /// Прогон прошёл порог и контроль аттестации.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.ratio >= self.min_detection && self.control_ok
    }
}

/// Что стало со смысловым мутантом в глазах судьи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticVerdict {
    /// Главный критерий ≤ 2 и обвинение подтверждено — дефект пойман судьёй.
    Caught,
    /// Главный критерий выше 2: судья противоречия не увидел.
    Missed,
    /// Критерий низкий, но цитат нет — обвинение не подтверждено механикой.
    Unconfirmed,
    /// Отчёта судьи в клоне нет.
    NoReport,
    /// Отчёт есть, но досье изменилось после оценки — судить по нему нельзя.
    Stale,
}

impl SemanticVerdict {
    /// Метка для вывода.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Caught => "пойман",
            Self::Missed => "не пойман",
            Self::Unconfirmed => "обвинение не подтверждено",
            Self::NoReport => "нет отчёта",
            Self::Stale => "отчёт устарел",
        }
    }

    /// Считается ли пойманным (числитель смысловой строки).
    #[must_use]
    pub fn caught(self) -> bool {
        matches!(self, Self::Caught)
    }
}

/// Разбор одного смыслового клона.
#[derive(Debug, Clone)]
pub struct SemanticCaseScore {
    /// Мутант (`D10`).
    pub mutant: String,
    /// Рубрика, которой судили.
    pub rubric: String,
    /// Субъект досье.
    pub subject: String,
    /// Вердикт.
    pub verdict: SemanticVerdict,
    /// Судья (из отчёта), если он есть.
    pub judge: Option<String>,
    /// Судья — автор документа (независимость не подтверждена).
    pub judge_is_author: bool,
}

/// Итог смыслового слоя (ADR-051, S5): отдельная строка, **не** входящая
/// ни в долю обнаружения, ни в порог.
#[derive(Debug, Clone)]
pub struct SemanticScore {
    /// Разбор по клонам.
    pub cases: Vec<SemanticCaseScore>,
}

impl SemanticScore {
    /// Сколько дефектов поймал судья.
    #[must_use]
    pub fn caught(&self) -> usize {
        self.cases.iter().filter(|c| c.verdict.caught()).count()
    }

    /// Строка отчёта: «смысловой слой: поймано k из n; судья: …; независим: …».
    #[must_use]
    pub fn render(&self) -> String {
        let judges: Vec<&str> = self
            .cases
            .iter()
            .filter_map(|c| c.judge.as_deref())
            .collect();
        let judge = if judges.is_empty() {
            "нет".to_string()
        } else {
            let mut uniq: Vec<&str> = judges.clone();
            uniq.sort_unstable();
            uniq.dedup();
            uniq.join(", ")
        };
        // Независимость — общее утверждение, а не по кейсу: если хоть где-то
        // судья совпал с автором, «да» было бы неправдой.
        let independent = !self.cases.is_empty()
            && self
                .cases
                .iter()
                .all(|c| c.judge.is_some() && !c.judge_is_author);
        let mut out = format!(
            "смысловой слой: поймано {} из {}; судья: {judge}; независим: {}",
            self.caught(),
            self.cases.len(),
            if independent { "да" } else { "нет" }
        );
        for c in &self.cases {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "\n  {} · {} · {} — {}",
                    c.mutant,
                    c.rubric,
                    c.subject,
                    c.verdict.label()
                ),
            );
        }
        out
    }
}

/// Читает отчёты судьи из сохранённых клонов и считает смысловую строку.
///
/// Пойман — главный критерий рубрики (`blocking`) с баллом ≤ 2 **и**
/// подтверждённым обвинением: отчёт без цитат механика сама исключает из
/// итога, и записывать это в поимку значило бы засчитывать выдуманное
/// свидетельство. Отчёт сверяется с досье по хэшу: изменился — «устарел».
///
/// # Errors
/// Каталог недоступен или задание `SEMANTIC-TODO.json` не разбирается.
pub fn semantic_score(dir: &Path, rubrics_dir: &Path) -> Result<SemanticScore> {
    if !dir.is_dir() {
        return Err(HarnessError::Control(format!(
            "каталог смысловых клонов недоступен: {}",
            dir.display()
        )));
    }
    let mut clones: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| HarnessError::io(dir, e))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SEMANTIC-TODO.json").is_file())
        .collect();
    clones.sort();
    let mut cases = Vec::new();
    for clone in &clones {
        let todo_path = clone.join("SEMANTIC-TODO.json");
        let text =
            std::fs::read_to_string(&todo_path).map_err(|e| HarnessError::io(&todo_path, e))?;
        let todo: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            HarnessError::Control(format!(
                "{}: задание не разбирается: {e}",
                todo_path.display()
            ))
        })?;
        let mutant = todo["mutant"].as_str().unwrap_or("?").to_string();
        let rubric_name = todo["rubric"].as_str().unwrap_or("").to_string();
        let subject = todo["subject"].as_str().unwrap_or("").to_string();
        cases.push(score_one_clone(
            clone,
            &mutant,
            &rubric_name,
            &subject,
            rubrics_dir,
        ));
    }
    if cases.is_empty() {
        return Err(HarnessError::Control(format!(
            "в {} нет сохранённых клонов с заданием SEMANTIC-TODO.json — \
             прогоните `redteam --keep-semantic <каталог>`",
            dir.display()
        )));
    }
    Ok(SemanticScore { cases })
}

/// Разбор одного клона: отчёт судьи по рубрике и субъекту, сверка с досье.
fn score_one_clone(
    clone: &Path,
    mutant: &str,
    rubric_name: &str,
    subject: &str,
    rubrics_dir: &Path,
) -> SemanticCaseScore {
    let base = SemanticCaseScore {
        mutant: mutant.to_string(),
        rubric: rubric_name.to_string(),
        subject: subject.to_string(),
        verdict: SemanticVerdict::NoReport,
        judge: None,
        judge_is_author: false,
    };
    let artifacts = crate::rubric::load_artifacts(clone);
    let Some(artifact) = artifacts
        .iter()
        .rev()
        .find(|a| a.rubric == rubric_name && a.subject.as_deref() == Some(subject))
    else {
        return base;
    };
    let judge = Some(artifact.judge_model.clone());
    let judge_is_author = artifact.author_model.as_deref() == Some(artifact.judge_model.as_str());
    let mut out = SemanticCaseScore {
        judge,
        judge_is_author,
        ..base
    };
    // Отчёт привязан к досье: пересобираем его в клоне и сверяем хэш — иначе
    // отчёт от прежней редакции читался бы как суждение о текущей. Отчёт БЕЗ
    // хэша досье (снят не по досье) устаревшим не объявляется: это не
    // расхождение, а отсутствие привязки, и судить о нём нечем.
    if let Some(want) = artifact.pack_sha256.as_deref() {
        let fresh =
            crate::rubric_pack::PackKind::parse(artifact.pack_kind.as_deref().unwrap_or_default())
                .ok()
                .and_then(|kind| crate::rubric_pack::build(clone, kind, subject).ok())
                .map(|packs| packs.iter().any(|p| p.sha256 == want));
        if fresh == Some(false) {
            out.verdict = SemanticVerdict::Stale;
            return out;
        }
    }
    // Главный критерий рубрики: без него измерять нечего.
    let Ok(rubric) = load_rubric_by_name(rubrics_dir, rubric_name) else {
        out.verdict = SemanticVerdict::NoReport;
        return out;
    };
    let Some(main) = rubric.criteria.iter().find(|c| c.blocking) else {
        out.verdict = SemanticVerdict::NoReport;
        return out;
    };
    out.verdict = match artifact_scores(artifact, &main.id) {
        None => SemanticVerdict::NoReport,
        Some((score, _)) if score > 2 => SemanticVerdict::Missed,
        Some((_, true)) => SemanticVerdict::Unconfirmed,
        Some(_) => SemanticVerdict::Caught,
    };
    out
}

/// Балл главного критерия и признак исключения из отчёта: обвинение без
/// подтверждённых цитат (`accusation_unconfirmed`) или без полного покрытия
/// (`coverage_incomplete`) механика исключает из итога — засчитывать это
/// поимкой значило бы записывать судье в заслугу выдуманное свидетельство.
fn artifact_scores(
    artifact: &crate::rubric::RubricArtifact,
    criterion_id: &str,
) -> Option<(u8, bool)> {
    let score = artifact
        .scores
        .iter()
        .find(|s| s.criterion_id == criterion_id)?;
    let excluded = score.flags.iter().any(|f| f.excludes_from_total());
    Some((score.score, excluded))
}

/// Рубрика по имени из каталога рубрик (ассеты конфига).
fn load_rubric_by_name(dir: &Path, name: &str) -> Result<crate::rubric::Rubric> {
    crate::rubric::load(&dir.join(format!("{name}.yaml")))
}

/// Записывает итог прогона в `<case>/.arch-handoff/redteam.json`.
///
/// # Errors
/// Каталог не создаётся либо файл не пишется.
pub fn save_summary(case: &Path, report: &RedteamReport) -> Result<PathBuf> {
    // Пишем в ИСХОДНЫЙ кейс, а не в `report.case`: прогон идёт в копии, и
    // сохранение «рядом с измерением» означало бы запись в каталог, который
    // тут же будет удалён. Метрика доверия читает `.arch-handoff/redteam.json`
    // именно исходного кейса — иначе `--save` выглядел бы рабочим, а
    // измерения не было бы ни у кого.
    let dir = case.join(".arch-handoff");
    std::fs::create_dir_all(&dir).map_err(|e| crate::error::HarnessError::io(&dir, e))?;
    let path = dir.join("redteam.json");
    let summary = RedteamSummary {
        schema: "arch-be/redteam/v1".to_string(),
        case: case.display().to_string(),
        measured_at: chrono::Local::now().to_rfc3339(),
        caught: report.scored_caught(),
        total: report.scored_total(),
        ratio: report.detection_ratio(),
        min_detection: report.min_detection,
        control_ok: report.control_ok,
    };
    let text = serde_json::to_string_pretty(&summary)
        .map_err(|e| crate::error::HarnessError::Config(format!("redteam: {e}")))?;
    std::fs::write(&path, text).map_err(|e| crate::error::HarnessError::io(&path, e))?;
    Ok(path)
}

/// Читает сохранённый итог; нет файла или он не разбирается — `None`
/// (метрика доверия не имеет права падать на чужом артефакте).
#[must_use]
pub fn load_summary(path: &Path) -> Option<RedteamSummary> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

impl RedteamReport {
    /// Дефекты, участвующие в доле обнаружения: входят в набор раздела 7 и
    /// не пропущены из-за отсутствия входа.
    fn scored(&self) -> Vec<&Detection> {
        self.detections
            .iter()
            .filter(|d| d.in_ratio && d.skipped.is_none())
            .collect()
    }

    /// Число дефектов в знаменателе доли.
    #[must_use]
    pub fn scored_total(&self) -> usize {
        self.scored().len()
    }

    /// Число пойманных дефектов (числитель доли): считаются только те, что
    /// обязаны ловиться, — случайно пойманная «семантика» долю не поднимает.
    #[must_use]
    pub fn scored_caught(&self) -> usize {
        self.scored()
            .iter()
            .filter(|d| d.expected == Expectation::Caught && d.caught())
            .count()
    }

    /// Доля обнаружения.
    ///
    /// Мутаторы, не нашедшие вход, из знаменателя исключаются — иначе
    /// неподходящий кейс выглядел бы «незащищённым» вместо «непроверенным».
    #[must_use]
    pub fn detection_ratio(&self) -> f64 {
        let total = self.scored_total();
        if total == 0 {
            return 1.0;
        }
        self.scored_caught() as f64 / total as f64
    }

    /// Прогон прошёл порог и контроль аттестации.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.detection_ratio() >= self.min_detection && self.control_ok
    }

    /// Текстовый рендер «карты обнаружения».
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Запись в String не может завершиться ошибкой — игноры безопасны.
        let _ = writeln!(
            out,
            "Мутационное тестирование пакета: {}",
            self.case.display()
        );
        let _ = writeln!(
            out,
            "Каталог мутаторов: {} (в долю входят только дефекты)",
            self.detections.len()
        );
        out.push('\n');
        for d in &self.detections {
            let (mark, who) = match (&d.caught_by, d.skipped.as_deref()) {
                (_, Some(reason)) => ("—", format!("пропущен: {reason}")),
                (Some(by), None) if d.expected == Expectation::Semantic => {
                    ("!", format!("пойман ({by}) — а не должен: это семантика"))
                }
                (Some(by), None) => ("✓", format!("пойман: {by}")),
                (None, None) if d.expected == Expectation::Semantic => {
                    ("·", "не пойман и не должен — работа ревьюера".to_string())
                }
                (None, None) if d.expected == Expectation::Control => (
                    "✗",
                    "контроль не сработал: аттестация не изменилась".to_string(),
                ),
                (None, None) => ("✗", format!("НЕ ПОЙМАН (ожидался: {})", d.expected_by)),
            };
            let _ = writeln!(out, "  [{mark}] {:<5} {:<52} {who}", d.id, d.title);
        }
        let _ = writeln!(
            out,
            "\nДоля обнаружения: {}/{} = {:.0}% (порог {:.0}%) · контроль аттестации: {}",
            self.scored_caught(),
            self.scored_total(),
            self.detection_ratio() * 100.0,
            self.min_detection * 100.0,
            if self.control_ok { "да" } else { "нет" }
        );
        let _ = writeln!(out, "Итог: {}", if self.passed() { "PASS" } else { "FAIL" });
        out
    }

    /// Машинный отчёт (`--format json`).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        let detections: Vec<serde_json::Value> = self
            .detections
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "title": d.title,
                    "expected": d.expected.label(),
                    "caught": d.caught(),
                    "caught_by": d.caught_by,
                    "skipped": d.skipped,
                })
            })
            .collect();
        serde_json::json!({
            "schema": "arch-be/redteam-report/v1",
            "case": self.case.display().to_string(),
            "detections": detections,
            "detection_ratio": self.detection_ratio(),
            "caught": self.scored_caught(),
            "total": self.scored_total(),
            "min_detection": self.min_detection,
            "control_ok": self.control_ok,
            "passed": self.passed(),
        })
    }
}

/// Markdown-рендер отчёта (`--format markdown`).
#[must_use]
pub fn render_markdown(report: &RedteamReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Мутационное тестирование пакета\n");
    let _ = writeln!(out, "Кейс: `{}`\n", report.case.display());
    let _ = writeln!(
        out,
        "**Доля обнаружения: {}/{} = {:.0}%** (порог {:.0}%), контроль аттестации: {}\n",
        report.scored_caught(),
        report.scored_total(),
        report.detection_ratio() * 100.0,
        report.min_detection * 100.0,
        if report.control_ok {
            "пройден"
        } else {
            "не пройден"
        }
    );
    let _ = writeln!(
        out,
        "| № | Дефект | Ожидание | Результат | Смысловая рубрика |\n|---|---|---|---|---|"
    );
    for d in &report.detections {
        let result = match (&d.caught_by, d.skipped.as_deref()) {
            (_, Some(reason)) => format!("пропущен: {reason}"),
            (Some(by), None) if d.expected == Expectation::Semantic => {
                format!("пойман ({by}) — не должен")
            }
            (Some(by), None) => format!("пойман: {by}"),
            (None, None) if d.expected == Expectation::Semantic => {
                "не пойман и не должен".to_string()
            }
            (None, None) if d.expected == Expectation::Control => {
                "контроль не сработал".to_string()
            }
            (None, None) => "**не пойман**".to_string(),
        };
        // Смысловая колонка (ADR-051, S5): у классов, где механика бессильна
        // по построению, названа рубрика, которой дефект ловится судьёй, —
        // иначе «не пойман и не должен» читается как приговор без выхода.
        let semantic = MUTATORS
            .iter()
            .find(|m| m.id == d.id)
            .and_then(|m| m.semantic.as_ref())
            .map_or_else(|| "—".to_string(), |s| s.rubric.to_string());
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            d.id,
            d.title,
            d.expected.label(),
            result,
            semantic
        );
    }
    if !report.semantic_kept.is_empty() {
        let _ = writeln!(
            out,
            "\nСмысловой слой (в долю обнаружения не входит): сохранено клонов — {}. \
             Прогоните судью по заданию `SEMANTIC-TODO.json` в каждом и затем \
             `arch-be redteam semantic-score <каталог>`.",
            report.semantic_kept.len()
        );
    }
    out
}

/// Песочница мутационного прогона: временный каталог, удаляется на `Drop`.
struct Fixture {
    root: PathBuf,
    case: PathBuf,
}

impl Fixture {
    fn new(case: &Path) -> Result<Self> {
        if !case.is_dir() {
            return Err(HarnessError::Control(format!(
                "кейс недоступен: {}",
                case.display()
            )));
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root =
            std::env::temp_dir().join(format!("arch-be-redteam-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).map_err(|e| HarnessError::io(&root, e))?;
        Ok(Self {
            root,
            case: case.to_path_buf(),
        })
    }

    /// Свежая копия кейса под мутанта `id`.
    fn mutant(&self, id: &str) -> Result<PathBuf> {
        let dest = self.root.join(id);
        if dest.exists() {
            std::fs::remove_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
        }
        std::fs::create_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
        copy_tree(&self.case, &dest)?;
        Ok(dest)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Игнорируем: песочница во временном каталоге, уборка best-effort.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Копирует дерево, пропуская `.git` (мутанту делается свой репозиторий).
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let rd = std::fs::read_dir(from).map_err(|e| HarnessError::io(from, e))?;
    for entry in rd.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if src.is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| HarnessError::io(&dst, e))?;
            copy_tree(&src, &dst)?;
        } else if src.is_file() {
            std::fs::copy(&src, &dst).map_err(|e| HarnessError::io(&dst, e))?;
        }
    }
    Ok(())
}

/// Первый подходящий субъект смысловой рубрики в клоне — относительный путь.
/// Порядок сортировки делает выбор детерминированным.
fn resolve_semantic_subject(root: &Path, subject: &SemanticSubject) -> Option<String> {
    let rd = std::fs::read_dir(root.join(subject.dir)).ok()?;
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(subject.prefix))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let name = names.into_iter().next()?;
    // Досье по сущностям адресуется ИДЕНТИФИКАТОРОМ (`CMP-001`), а не путём к
    // файлу: сборщик досье ищет сущность в модели и путь не разбирает (живой
    // прогон 2026-09-20 — `pack_subject_not_found` на файле).
    match subject.pack {
        crate::rubric_pack::PackKind::EntityLinks | crate::rubric_pack::PackKind::NfrMechanism => {
            let id: Vec<&str> = name.split('-').take(2).collect();
            (id.len() == 2).then(|| id.join("-"))
        }
        _ => Some(format!("{}/{}", subject.dir, name)),
    }
}

/// Копирует клон смыслового мутанта в `dest_root/<D-n>` и кладёт рядом
/// `SEMANTIC-TODO.json` — задание хосту (ADR-051, S5).
///
/// Смысловой дефект ловит не харнесс, а модель хоста, и ей нужен не только
/// клон, но и точный вход: вид досье и субъект. Задание собирается из
/// метаданных мутатора, а не пишется человеком, — иначе прогон судьи и
/// измерение разъезжались бы.
fn keep_semantic_clone(
    root: &Path,
    dest_root: &Path,
    m: &Mutator,
    subject: &SemanticSubject,
) -> Result<PathBuf> {
    let resolved = resolve_semantic_subject(root, subject).ok_or_else(|| {
        HarnessError::Control(format!(
            "{}: субъект смысловой рубрики '{}' не найден в клоне ({}/{}*) — \
             измерять нечего",
            m.id, subject.rubric, subject.dir, subject.prefix
        ))
    })?;
    let dest = dest_root.join(m.id);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
    }
    std::fs::create_dir_all(&dest).map_err(|e| HarnessError::io(&dest, e))?;
    copy_tree(root, &dest)?;
    let todo = serde_json::json!({
        "schema": "arch-be/semantic-todo/v1",
        "mutant": m.id,
        "title": m.title,
        "expected": m.expected.label(),
        "rubric": subject.rubric,
        "pack": subject.pack.as_str(),
        "subject": resolved,
        "clone": dest.display().to_string(),
        "instructions": "Судит модель хоста (в ядре LLM нет): rubric_prompt с pack/subject/root \
                         → k независимых ответов → rubric_verify под --rw (отчёт ляжет в \
                         reports/rubric/ клона). Затем `arch-be redteam semantic-score <каталог>`. \
                         Смысловая строка в долю обнаружения не входит (ADR-051).",
    });
    let path = dest.join("SEMANTIC-TODO.json");
    let text = serde_json::to_string_pretty(&todo)
        .map_err(|e| HarnessError::Config(format!("redteam: задание семантики: {e}")))?;
    std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
    Ok(dest)
}

/// git-команда мутанта (идентичность коммиттера задаём явно: CI без
/// `user.email` иначе падает на `git commit`).
fn git(dir: &Path, args: &[&str]) -> std::result::Result<(), String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "redteam")
        .env("GIT_AUTHOR_EMAIL", "redteam@example.invalid")
        .env("GIT_COMMITTER_NAME", "redteam")
        .env("GIT_COMMITTER_EMAIL", "redteam@example.invalid")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git не запустился: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Готовит мутанта: свой git-репозиторий, базовый коммит, правка, коммит
/// правки. База `HEAD~1` нужна, чтобы анти-ослабление правил видело правку
/// реестра, уже лежащую в коммите (Н5).
fn prepare(root: &Path) -> std::result::Result<(), String> {
    git(root, &["init", "-q"])?;
    git(root, &["add", "-A"])?;
    git(root, &["commit", "-q", "-m", "baseline"])?;
    Ok(())
}

/// Отчёт гейта по мутанту (для контроля D14 нужен вердикт и аттестация).
fn gate_report(root: &Path, decision_quality: bool) -> Result<GateReport> {
    let mut requirements = GateRequirements::default();
    if decision_quality {
        requirements.critical.push("decision_quality".to_string());
    }
    gate::run_opts(
        root,
        Some(Route::Critical),
        Some("HEAD~1"),
        None,
        (1, 4),
        &requirements,
        &GateOptions::default(),
    )
}

/// Мутационный прогон по кейсу.
///
/// # Errors
/// Кейс недоступен, git недоступен, гейт не смог отработать на эталоне.
/// Параметры мутационного прогона.
pub struct RedteamOptions {
    /// Порог доли обнаружения, ниже которого прогон красный.
    pub min_detection: f64,
    /// Учитывать ли составляющую `decision_quality` при прогоне гейта.
    pub decision_quality: bool,
    /// Куда сохранить клоны смысловых мутантов (ADR-051, S5): `None` — клоны
    /// удаляются, как раньше.
    pub keep_semantic: Option<PathBuf>,
}

impl Default for RedteamOptions {
    fn default() -> Self {
        Self {
            min_detection: 0.78,
            decision_quality: true,
            keep_semantic: None,
        }
    }
}

/// Прогон с параметрами по умолчанию (поведение 0.3.4).
///
/// # Errors
/// Кейс недоступен, не зелёный на маршруте Critical, git или гейт отказали.
pub fn run(case: &Path, min_detection: f64, decision_quality: bool) -> Result<RedteamReport> {
    run_with_options(
        case,
        &RedteamOptions {
            min_detection,
            decision_quality,
            keep_semantic: None,
        },
    )
}

/// Мутационный прогон с опциями.
///
/// # Errors
/// Кейс недоступен, не зелёный на маршруте Critical, git или гейт отказали.
pub fn run_with_options(case: &Path, options: &RedteamOptions) -> Result<RedteamReport> {
    let min_detection = options.min_detection;
    let decision_quality = options.decision_quality;
    let fixture = Fixture::new(case)?;
    // Эталон: кейс без правок. Нужен и как проверка «кейс вообще зелёный»
    // (иначе доля обнаружения мерила бы сломанный кейс), и как база для
    // контроля D14.
    let reference_root = fixture.mutant("reference")?;
    prepare(&reference_root).map_err(HarnessError::Control)?;
    // Второй коммит обязателен: база прогона — `HEAD~1`, а на репозитории с
    // одним коммитом git отказывает, и delta_guard краснеет не по делу.
    let _ = crate::evidence::pack(&reference_root, Route::Critical);
    if let Err(e) = git(&reference_root, &["add", "-A"])
        .and_then(|()| git(&reference_root, &["commit", "-q", "-m", "reference"]))
    {
        return Err(HarnessError::Control(format!("эталон: коммит: {e}")));
    }
    let reference = gate_report(&reference_root, decision_quality)?;
    if reference.outcome != GateOutcome::Pass {
        let failed: Vec<String> = reference
            .components
            .iter()
            .filter(|c| c.status == GateStatus::Fail)
            .map(|c| c.name.to_string())
            .collect();
        return Err(HarnessError::Control(format!(
            "кейс {} не зелёный на маршруте Critical — мутационный прогон мерил бы \
             сломанный пакет (провалены: {}); красноглазый эталон не даёт отличить \
             «дефект пойман» от «пакет уже сломан»",
            case.display(),
            failed.join(", ")
        )));
    }
    let mut detections = Vec::new();
    let mut control_ok = true;
    let mut kept: Vec<PathBuf> = Vec::new();
    for m in &MUTATORS {
        let root = fixture.mutant(m.id)?;
        if let Err(e) = prepare(&root) {
            return Err(HarnessError::Control(format!(
                "{}: подготовка мутанта: {e}",
                m.id
            )));
        }
        if let Err(reason) = (m.apply)(&root) {
            detections.push(Detection {
                id: m.id.to_string(),
                title: m.title.to_string(),
                expected: m.expected,
                caught_by: None,
                skipped: Some(reason),
                expected_by: m.by.to_string(),
                in_ratio: m.in_ratio,
            });
            continue;
        }
        // Бандл переупаковывается ПОСЛЕ правки: иначе любая правка удостоверенного
        // файла краснила бы evidence_verify как «изменён после упаковки», и
        // дефект ловился бы не тем инструментом, который проверяется.
        let _ = crate::evidence::pack(&root, Route::Critical);
        if let Err(e) = git(&root, &["add", "-A"])
            .and_then(|()| git(&root, &["commit", "-q", "-m", &format!("mutant {}", m.id)]))
        {
            return Err(HarnessError::Control(format!("{}: коммит: {e}", m.id)));
        }
        // Смысловой мутант: клон сохраняется для хоста вместе с заданием
        // (ADR-051, S5) — судить его будет модель хоста, а не харнесс.
        if let (Some(subject), Some(dest_root)) = (&m.semantic, options.keep_semantic.as_deref()) {
            let dest = keep_semantic_clone(&root, dest_root, m, subject)?;
            kept.push(dest);
        }
        let report = gate_report(&root, decision_quality)?;
        let failed: Vec<String> = report
            .components
            .iter()
            .filter(|c| c.status == GateStatus::Fail)
            .map(|c| c.name.to_string())
            .collect();
        if m.expected == Expectation::Control {
            // Контроль: вердикт обязан остаться зелёным, аттестация — смениться.
            let same_verdict = report.outcome == reference.outcome;
            let changed = report.attestation != reference.attestation;
            control_ok = control_ok && same_verdict && changed;
            detections.push(Detection {
                id: m.id.to_string(),
                title: m.title.to_string(),
                expected: m.expected,
                expected_by: m.by.to_string(),
                in_ratio: m.in_ratio,
                caught_by: if changed {
                    Some(format!(
                        "аттестация {} → {}",
                        &reference.attestation[..12],
                        &report.attestation[..12]
                    ))
                } else if same_verdict {
                    None
                } else {
                    Some("вердикт изменился".to_string())
                },
                skipped: None,
            });
            continue;
        }
        detections.push(Detection {
            id: m.id.to_string(),
            title: m.title.to_string(),
            expected: m.expected,
            expected_by: m.by.to_string(),
            in_ratio: m.in_ratio,
            caught_by: if failed.is_empty() {
                None
            } else {
                Some(failed.join(", "))
            },
            skipped: None,
        });
    }
    Ok(RedteamReport {
        case: case.to_path_buf(),
        detections,
        min_detection,
        control_ok,
        semantic_kept: kept,
    })
}

#[cfg(test)]
mod tests {
    /// W2×W4: `save_summary` пишет в УКАЗАННЫЙ каталог, а не в `report.case`
    /// (прогон идёт в копии — измерение принадлежит исходному кейсу).
    #[test]
    fn save_summary_writes_into_the_given_case() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        let measured = tmp.path().join("копия");
        let report = RedteamReport {
            case: measured,
            detections: Vec::new(),
            min_detection: 0.78,
            control_ok: true,
            semantic_kept: Vec::new(),
        };
        let path = save_summary(&case, &report).expect("save");
        assert!(
            path.starts_with(&case),
            "запись в исходный кейс: {}",
            path.display()
        );
        let back = load_summary(&path).expect("load");
        assert_eq!(back.case, case.display().to_string());
        assert!(back.control_ok);
    }

    use super::*;

    #[test]
    fn expectation_labels_are_stable() {
        assert_eq!(Expectation::Caught.label(), "ловится");
        assert_eq!(Expectation::Semantic.label(), "семантика");
        assert_eq!(Expectation::Control.label(), "контроль");
    }

    /// T-02: D7 ослабляет ТОТ реестр, который читает гейт. При двух копиях
    /// (пакетной и корневой) резолвер выбирает пакетную — если мутант правит
    /// корневую, гейт ослабления не видит, и D7 числится непойманным на
    /// ровном месте: «дыра в защите», которой нет.
    #[test]
    fn d7_weakens_the_registry_the_gate_reads() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(case.join(".arch-handoff")).expect("mkdir");
        let strong = "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n";
        std::fs::write(case.join(".arch-handoff/CONSTRAINTS.yaml"), strong).expect("реестр пакета");
        std::fs::write(
            case.join("CONSTRAINTS.yaml"),
            "rules:\n  - name: readme_exists\n    type: file_exists\n    path: \"README.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("корневой реестр");
        std::fs::write(case.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");

        mutate_d7(&case).expect("мутант D7");
        let packet =
            std::fs::read_to_string(case.join(".arch-handoff/CONSTRAINTS.yaml")).expect("пакет");
        assert!(
            packet.contains("severity: warn"),
            "ослабление обязано быть в реестре, который читает гейт: {packet}"
        );
        let root = std::fs::read_to_string(case.join("CONSTRAINTS.yaml")).expect("корень");
        assert!(
            root.contains("severity: error"),
            "корневая копия мутантом не трогается — её гейт не читает: {root}"
        );
    }

    #[test]
    fn catalog_covers_the_red_team_set() {
        let ids: Vec<&str> = MUTATORS.iter().map(|m| m.id).collect();
        for expected in [
            "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10", "D11", "D11b", "D12",
            "D13", "R", "D14",
        ] {
            assert!(ids.contains(&expected), "в каталоге нет {expected}");
        }
        // Доля обнаружения считается по 14 дефектам раздела 7 (D1…D13 + D11b);
        // R и D14 — отдельные строки.
        let scored = MUTATORS.iter().filter(|m| m.in_ratio).count();
        assert_eq!(scored, 14, "в наборе обязано быть 14 позиций раздела 7");
        // Обязаны ловиться 11: D1–D5, D7, D8, D9, D11b, D12, D13.
        let must_catch = MUTATORS
            .iter()
            .filter(|m| m.in_ratio && m.expected == Expectation::Caught)
            .count();
        assert_eq!(must_catch, 11, "критерий приёмки — 11 из 14");
        // Не ловятся и не должны: D6, D10, D11.
        let semantic = MUTATORS
            .iter()
            .filter(|m| m.in_ratio && m.expected == Expectation::Semantic)
            .count();
        assert_eq!(semantic, 3);
    }

    #[test]
    fn ratio_excludes_skipped_and_semantic() {
        let report = RedteamReport {
            case: PathBuf::from("case"),
            detections: vec![
                Detection {
                    id: "D1".into(),
                    title: "ловится".into(),
                    expected: Expectation::Caught,
                    caught_by: Some("nfr".into()),
                    skipped: None,
                    expected_by: "nfr".into(),
                    in_ratio: true,
                },
                Detection {
                    id: "D2".into(),
                    title: "пропущен".into(),
                    expected: Expectation::Caught,
                    caught_by: None,
                    skipped: Some("нет входа".into()),
                    expected_by: "nfr".into(),
                    in_ratio: true,
                },
                Detection {
                    id: "D6".into(),
                    title: "семантика".into(),
                    expected: Expectation::Semantic,
                    caught_by: None,
                    skipped: None,
                    expected_by: "—".into(),
                    in_ratio: true,
                },
            ],
            min_detection: 0.5,
            control_ok: true,
            semantic_kept: Vec::new(),
        };
        // Пропущенный (нет входа) выпадает из знаменателя, семантический —
        // остаётся: он обязан НЕ ловиться, и доля это учитывает.
        assert_eq!(report.scored_total(), 2);
        assert_eq!(report.scored_caught(), 1);
        assert!((report.detection_ratio() - 0.5).abs() < f64::EPSILON);
        assert!(report.passed());
        // Семантический дефект, пойманный по ошибке, долю не поднимает.
        let mut over = report.clone();
        over.detections[2].caught_by = Some("model_validate".into());
        assert_eq!(over.scored_caught(), 1);
        // Красный, когда поймано меньше порога.
        let mut low = report.clone();
        low.min_detection = 0.6;
        assert!(!low.passed());
        // Контроль аттестации краснит прогон сам по себе.
        let mut ncontrol = report.clone();
        ncontrol.control_ok = false;
        assert!(!ncontrol.passed());
    }

    // --- S5 (ADR-051): смысловой слой отдельной строкой ---------------------

    /// Мутатор по идентификатору из каталога.
    fn mutator(id: &str) -> &'static Mutator {
        MUTATORS.iter().find(|m| m.id == id).expect("мутатор")
    }

    /// Клон смыслового мутанта сохраняется вместе с заданием хосту: в клоне
    /// лежит правленый субъект, рядом — `SEMANTIC-TODO.json` с рубрикой, видом
    /// досье и субъектом. Без задания хост не знал бы, чем судить.
    #[test]
    fn semantic_clone_keeps_subject_and_todo() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("clone");
        std::fs::create_dir_all(root.join("model")).expect("mkdir");
        std::fs::write(
            root.join("model/CMP-001-jurnal.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Журнал\nstatus: designed\n---\n\nтело\n",
        )
        .expect("write");
        let m = mutator("D6");
        let subject = m.semantic.as_ref().expect("D6 — смысловой мутант");
        assert_eq!(subject.rubric, "model_link_semantics");

        let dest_root = tmp.path().join("kept");
        let dest = keep_semantic_clone(&root, &dest_root, m, subject).expect("клон");
        assert_eq!(dest, dest_root.join("D6"));
        assert!(
            dest.join("model/CMP-001-jurnal.md").is_file(),
            "правленый субъект в клоне"
        );
        let todo: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dest.join("SEMANTIC-TODO.json")).expect("todo"),
        )
        .expect("json");
        assert_eq!(todo["mutant"], "D6");
        assert_eq!(todo["rubric"], "model_link_semantics");
        assert_eq!(todo["pack"], "entity_links");
        assert_eq!(
            todo["subject"], "CMP-001",
            "досье по сущностям адресуется идентификатором, а не путём"
        );
        assert!(
            todo["instructions"]
                .as_str()
                .expect("инструкция")
                .contains("rubric_prompt"),
            "задание говорит, чем судить: {todo}"
        );
    }

    /// Резолвер субъекта берёт первый файл в отсортированном порядке — прогон
    /// детерминирован независимо от порядка файловой системы.
    #[test]
    fn semantic_subject_resolution_is_sorted() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("clone");
        std::fs::create_dir_all(root.join("docs/adr")).expect("mkdir");
        for name in ["ADR-002-b.md", "ADR-001-a.md"] {
            std::fs::write(root.join("docs/adr").join(name), "x").expect("write");
        }
        let m = mutator("D10");
        let subject = m.semantic.as_ref().expect("D10 — смысловой мутант");
        assert_eq!(
            resolve_semantic_subject(&root, subject).as_deref(),
            Some("docs/adr/ADR-001-a.md")
        );
    }

    /// Досье по сущностям адресуется идентификатором, а не путём к файлу:
    /// иначе `rubric run --pack entity_links` не находит субъекта.
    #[test]
    fn semantic_subject_for_entities_is_an_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("clone");
        std::fs::create_dir_all(root.join("model")).expect("mkdir");
        std::fs::write(
            root.join("model/CMP-001-orchestrator-vyplat.md"),
            "---\nid: CMP-001\ntype: cmp\ntitle: Оркестратор\nstatus: designed\n---\n\nтело\n",
        )
        .expect("write");
        let m = mutator("D6");
        let subject = m.semantic.as_ref().expect("D6 — смысловой мутант");
        assert_eq!(
            resolve_semantic_subject(&root, subject).as_deref(),
            Some("CMP-001")
        );
    }

    /// Что писать в отчёт судьи клона (для тестов смысловой строки).
    struct ReportSpec<'a> {
        main_score: u8,
        flags: &'a [&'a str],
        pack_sha256: Option<&'a str>,
        author: Option<&'a str>,
    }

    /// Отчёт судьи в клоне: пишем минимальный валидный артефакт рубрики.
    fn write_report(clone: &Path, rubric: &str, subject: &str, spec: &ReportSpec<'_>) {
        let ReportSpec {
            main_score,
            flags,
            pack_sha256,
            author,
        } = *spec;
        let dir = clone.join(crate::rubric::RUBRIC_REPORTS_DIR);
        std::fs::create_dir_all(&dir).expect("mkdir reports");
        let artifact = serde_json::json!({
            "schema": crate::rubric::RUBRIC_REPORT_SCHEMA,
            "rubric": rubric,
            "judge_model": "judge-x",
            "author_model": author,
            "weighted_total": 2.0,
            "verdict": "CONCERNS",
            "pack_kind": "adr_vs_spine",
            "subject": subject,
            "pack_sha256": pack_sha256,
            "scores": [{
                "criterion_id": "no_contradiction",
                "weight": 3.0,
                "score": main_score,
                "rationale": "Цитата subject: \"a\". Цитата reference: \"b\".",
                "samples": [main_score],
                "stdev": 0.0,
                "flags": flags,
                "evidence_unconfirmed_ratio": 0.0,
                "checked": ["AD-1"],
            }],
            "judged_at": "2026-09-20T10:00:00+03:00",
        });
        std::fs::write(
            dir.join("report.json"),
            serde_json::to_string_pretty(&artifact).expect("json"),
        )
        .expect("write report");
    }

    /// Клон с заданием и скелетом досье (`docs/adr` + спайн), чтобы отчёт
    /// можно было пересчитать и сверить хэш.
    fn score_clone(root: &Path) -> PathBuf {
        let clone = root.join("D10");
        std::fs::create_dir_all(clone.join("docs/adr")).expect("mkdir");
        std::fs::write(
            clone.join(ARCHITECTURE_SPINE_FOR_TEST),
            "## AD-1: Журнал только дописывается\n\n- **Rule**: строки журнала не правятся.\n",
        )
        .expect("spine");
        std::fs::write(
            clone.join("docs/adr/ADR-001-x.md"),
            "# ADR-001\n\nрешение\n",
        )
        .expect("adr");
        std::fs::write(
            clone.join("SEMANTIC-TODO.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "mutant": "D10",
                "rubric": "adr_spine_consistency",
                "pack": "adr_vs_spine",
                "subject": "docs/adr/ADR-001-x.md",
            }))
            .expect("json"),
        )
        .expect("todo");
        clone
    }

    /// Имя спайна в клоне — как у сборщика досье.
    const ARCHITECTURE_SPINE_FOR_TEST: &str = "ARCHITECTURE-SPINE.md";

    /// Каталог рубрик с одной смысловой рубрикой (главный критерий — `low`).
    fn rubrics_dir(root: &Path) -> PathBuf {
        let dir = root.join("rubrics");
        std::fs::create_dir_all(&dir).expect("mkdir rubrics");
        std::fs::write(
            dir.join("adr_spine_consistency.yaml"),
            "name: adr_spine_consistency\ndescription: d\nscale_max: 5\norigin: anchor\n\
             pack: adr_vs_spine\ncriteria:\n  - id: no_contradiction\n    name: n\n    \
             description: d\n    weight: 3.0\n    blocking: true\n    evidence_on: low\n",
        )
        .expect("rubric");
        dir
    }

    /// Смысловая строка: пойман — главный критерий ≤ 2 с подтверждённым
    /// обвинением; выше — пропуск; с меткой исключения — обвинение не
    /// подтверждено; с чужим хэшем досье — отчёт устарел.
    #[test]
    fn semantic_score_reads_verdicts_from_reports() {
        let cases: [(&str, ReportSpec<'_>, SemanticVerdict); 4] = [
            (
                "пойман",
                ReportSpec {
                    main_score: 1,
                    flags: &[],
                    pack_sha256: None,
                    author: Some("author-y"),
                },
                SemanticVerdict::Caught,
            ),
            (
                "пропуск",
                ReportSpec {
                    main_score: 4,
                    flags: &[],
                    pack_sha256: None,
                    author: Some("author-y"),
                },
                SemanticVerdict::Missed,
            ),
            (
                "не подтверждено",
                ReportSpec {
                    main_score: 1,
                    flags: &["accusation_unconfirmed"],
                    pack_sha256: None,
                    author: Some("author-y"),
                },
                SemanticVerdict::Unconfirmed,
            ),
            (
                "устарел",
                ReportSpec {
                    main_score: 1,
                    flags: &[],
                    pack_sha256: Some(
                        "0000000000000000000000000000000000000000000000000000000000000000",
                    ),
                    author: Some("author-y"),
                },
                SemanticVerdict::Stale,
            ),
        ];
        for (name, spec, want) in cases {
            let tmp = tempfile::tempdir().expect("tmp");
            let clone = score_clone(tmp.path());
            write_report(
                &clone,
                "adr_spine_consistency",
                "docs/adr/ADR-001-x.md",
                &spec,
            );
            let scored = semantic_score(tmp.path(), &rubrics_dir(tmp.path())).expect("score");
            assert_eq!(scored.cases.len(), 1, "{name}");
            assert_eq!(scored.cases[0].verdict, want, "{name}: {scored:?}");
            // Человеческая строка называет и судью, и независимость.
            let text = scored.render();
            if want == SemanticVerdict::Caught {
                assert!(text.contains("смысловой слой: поймано 1 из 1"), "{text}");
                assert!(text.contains("судья: judge-x"), "{text}");
                assert!(text.contains("независим: да"), "{text}");
            }
        }
    }

    /// Клон без отчёта судьи — «нет отчёта», а не «не пойман»: разница
    /// принципиальна, иначе непрогнанный кейс считался бы провалом судьи.
    #[test]
    fn semantic_score_without_report_says_so() {
        let tmp = tempfile::tempdir().expect("tmp");
        let _ = score_clone(tmp.path());
        let scored = semantic_score(tmp.path(), &rubrics_dir(tmp.path())).expect("score");
        assert_eq!(scored.cases[0].verdict, SemanticVerdict::NoReport);
        assert_eq!(scored.caught(), 0);
        assert!(scored.render().contains("независим: нет"));
    }

    /// Пустой каталог — явная ошибка с подсказкой, а не «поймано 0 из 0».
    #[test]
    fn semantic_score_empty_dir_is_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = semantic_score(tmp.path(), &rubrics_dir(tmp.path())).expect_err("пусто");
        assert!(err.to_string().contains("keep-semantic"), "{err}");
    }

    /// Смысловая колонка в карте обнаружения называет рубрику: без неё
    /// «не пойман и не должен» читается как приговор без выхода.
    #[test]
    fn detection_map_names_semantic_rubric() {
        let report = RedteamReport {
            case: PathBuf::from("case"),
            detections: vec![Detection {
                id: "D10".into(),
                title: "решение противоречит инварианту".into(),
                expected: Expectation::Semantic,
                caught_by: None,
                skipped: None,
                expected_by: "—".into(),
                in_ratio: true,
            }],
            min_detection: 0.78,
            control_ok: true,
            semantic_kept: Vec::new(),
        };
        let md = render_markdown(&report);
        assert!(
            md.contains("| D10 |") && md.contains("adr_spine_consistency"),
            "колонка смысловой рубрики: {md}"
        );
        assert!(md.contains("не пойман и не должен"), "{md}");
    }
}

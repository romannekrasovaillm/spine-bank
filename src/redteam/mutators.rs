//! Каталог мутаторов redteam и правки кейса-мутанта (выделено из
//! `redteam.rs` при разбиении 0.3.14 — лимит длины продуктового файла).
//! Прогон, отчёты и доли обнаружения — в [`super`].

use std::fmt::Write as _;
use std::path::Path;

use super::{Expectation, Layer, Mutator, SemanticSubject};

/// Каталог мутаторов — раздел 7 задания 0.3.4. Порядок фиксирован: прогон
/// детерминирован.
///
/// Дефекты `D1…D13`, `D11b` образуют набор из 14 позиций раздела 7; `R`
/// (ревью `NOT-READY`), `D14` (контроль аттестации) и `D15` (нарушение
/// инварианта в реализации скелета) идут отдельными строками и в долю
/// обнаружения не входят — иначе она была бы несопоставима с критерием
/// приёмки релиза.
///
/// `D16` и `D17` (красный угол 0.3.5, ADR-048) — про происхождение оценки:
/// поднятый рукой балл в отчёте рубрики и подменённая метка автора в шапке ADR
/// после оценки. Они применимы только к кейсам, где есть отчёты с сырыми
/// ответами; там, где их нет, мутатор честно пропускается (как `skipped`), а не
/// считается пойманным или непойманным. В знаменатель доли не входят: набор
/// раздела 7 не меняется.
///
/// Номера `D16`/`D17`, а не `D15`/`D16`: `D15` в этом каталоге занят
/// нарушением инварианта в скелете (ADR-050) — при слиянии ветка среза
/// происхождения уступила занятый номер, чтобы не переименовывать уже
/// влитый мутатор.
///
/// `D18` (волна B, B2, ADR-065) — лексический обход: в коде скелета проверка
/// удаляется, а ключевое слово `pattern` остаётся в комментарии; ожидание —
/// по составу правил кейса: `Semantic`, если инвариант охраняет только
/// `must_contain` (слово на месте — правило зелёное), `Caught` для правила из
/// шаблона (тест падает на отсутствующей проверке).
///
/// `D19`–`D24` (волна E, E1) — кодовые классы корпуса реальных нарушений
/// агентов (`experiments/openspec-vs-spine/`, 180 генераций, разрез по
/// правилам): f64 для денег, `unwrap` в денежном пути, ошибки строками, персональные данные
/// в логах, секрет литералом, обработчик без ключа идемпотентности. Ожидание
/// динамическое: правило класса есть в реестре и покрывает файл — `Caught`,
/// нет — `Semantic` (не вина механики). Все — отдельные строки вне
/// знаменателя доли (как `D15`): они меряют кодовый слой (E2), а не набор
/// раздела 7.
pub const MUTATORS: [Mutator; 26] = [
    Mutator {
        id: "D1",
        title: "бюджет hop'а больше цели p99",
        by: "nfr (budget)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d1,
    },
    Mutator {
        id: "D2",
        title: "цель доступности недостижима",
        by: "nfr (availability)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d2,
    },
    Mutator {
        id: "D3",
        title: "дефицит ёмкости",
        by: "nfr (capacity)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d3,
    },
    Mutator {
        id: "D4",
        title: "инвариант без правила",
        by: "trace_check",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d4,
    },
    Mutator {
        id: "D5",
        title: "битая ссылка модели",
        by: "model_validate",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
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
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d6,
    },
    Mutator {
        id: "D7",
        title: "ослабление правила, закоммичено",
        by: "rule_weakened + база",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d7,
    },
    Mutator {
        id: "D8",
        title: "ADR без секции альтернатив",
        by: "fitness",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d8,
    },
    Mutator {
        id: "D9",
        title: "«картонный» ADR, секции есть",
        by: "decision_quality",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
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
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d10,
    },
    Mutator {
        id: "D11",
        title: "код нарушает инвариант, правил на код нет",
        by: "— (кандидат executable-invariant:AD-N)",
        expected: Expectation::Semantic,
        in_ratio: true,
        semantic: Some(SemanticSubject {
            rubric: "code_invariant_conformance",
            pack: crate::rubric_pack::PackKind::CodeVsSpine,
            dir: "src/legacy",
            prefix: "payments.py",
        }),
        layer: Layer::Code,
        expected_in: None,
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
        layer: Layer::Code,
        expected_in: None,
        apply: mutate_d11b,
    },
    Mutator {
        id: "D12",
        title: "NFR без способа проверки",
        by: "model_validate (Critical)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d12,
    },
    Mutator {
        id: "D13",
        title: "DECISION.md = «TODO»",
        by: "evidence (Н1)",
        expected: Expectation::Caught,
        in_ratio: true,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d13,
    },
    Mutator {
        id: "R",
        title: "ревью NOT-READY в бандле",
        by: "evidence (Н1)",
        expected: Expectation::Caught,
        in_ratio: false,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_r,
    },
    Mutator {
        id: "D14",
        title: "контроль: безвредная правка",
        by: "аттестация ≠ эталон, вердикт PASS",
        expected: Expectation::Control,
        in_ratio: false,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d14,
    },
    Mutator {
        id: "D15",
        title: "нарушение инварианта в реализации скелета",
        by: "fitness",
        expected: Expectation::Caught,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: None,
        apply: mutate_d15,
    },
    Mutator {
        id: "D16",
        title: "балл в отчёте рубрики поднят вручную",
        by: "decision_quality (rubric_report_inconsistent)",
        expected: Expectation::Caught,
        in_ratio: false,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d16,
    },
    Mutator {
        id: "D17",
        title: "метка автора в шапке ADR заменена после оценки",
        by: "decision_quality (rubric_report_stale)",
        expected: Expectation::Caught,
        in_ratio: false,
        semantic: None,
        layer: Layer::DocsModel,
        expected_in: None,
        apply: mutate_d17,
    },
    Mutator {
        id: "D18",
        title: "слово на месте, логики нет (лексический обход must_contain)",
        by: "шаблонное command_succeeds; текстовое правило — нет",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d18),
        apply: mutate_d18,
    },
    Mutator {
        id: "D19",
        title: "деньги в f64 (корпус S-01)",
        by: "must_not_contain \\bf(64|32)\\b",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d19),
        apply: mutate_d19,
    },
    Mutator {
        id: "D20",
        title: "unwrap в денежном пути (корпус S-03)",
        by: "must_not_contain unwrap/expect/panic",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d20),
        apply: mutate_d20,
    },
    Mutator {
        id: "D21",
        title: "ошибки строками вместо типизированных (корпус S-05)",
        by: "must_not_contain Err(\"…\")",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d21),
        apply: mutate_d21,
    },
    Mutator {
        id: "D22",
        title: "ПДн в логах: номер карты (корпус S-04)",
        by: "must_not_contain card_number в log",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d22),
        apply: mutate_d22,
    },
    Mutator {
        id: "D23",
        title: "секрет литералом в коде (корпус S-08)",
        by: "must_not_contain api_key = \"…\"",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d23),
        apply: mutate_d23,
    },
    Mutator {
        id: "D24",
        title: "обработчик без ключа идемпотентности (корпус S-02)",
        by: "each_file_must_contain idempotency",
        expected: Expectation::Semantic,
        in_ratio: false,
        semantic: None,
        layer: Layer::Code,
        expected_in: Some(expect_d24),
        apply: mutate_d24,
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
pub(super) fn mutate_d7(root: &Path) -> std::result::Result<(), String> {
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

/// D16: балл в отчёте рубрики поднят вручную — ровно то, что до 0.3.5 было
/// незаметно. Ловится пересборкой отчёта из сырых ответов судьи
/// (`rubric_report_inconsistent`). Кейс без отчётов и сырых ответов —
/// неприменим: правка несуществующего отчёта ничего не проверяет.
fn mutate_d16(root: &Path) -> std::result::Result<(), String> {
    let reports = root.join("reports/rubric");
    let Some(rel) = std::fs::read_dir(&reports)
        .map_err(|e| format!("нет каталога отчётов: {e}"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
    else {
        return Err("нет отчётов рубрики".to_string());
    };
    let slug = rel
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !reports.join("raw").join(&slug).is_dir() {
        return Err("у отчёта нет сырых ответов — сверка неприменима".to_string());
    }
    let text = std::fs::read_to_string(&rel).map_err(|e| e.to_string())?;
    let mut artifact: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    // Поднимаем балл первого критерия и взвешенный итог: отчёт расходится с
    // ответами, из которых объявлен собранным.
    if let Some(score) = artifact
        .get_mut("scores")
        .and_then(|s| s.as_array_mut())
        .and_then(|a| a.first_mut())
    {
        score["score"] = serde_json::json!(5);
    } else {
        return Err("в отчёте нет баллов по критериям".to_string());
    }
    artifact["weighted_total"] = serde_json::json!(5.0);
    let out = serde_json::to_string_pretty(&artifact).map_err(|e| e.to_string())?;
    std::fs::write(&rel, out).map_err(|e| e.to_string())
}

/// D17: метка автора в шапке ADR заменена после оценки. Отчёт привязан к
/// редакции документа, поэтому правка шапки делает его устаревшим
/// (`rubric_report_stale`) — «вспомнить» автора задним числом нельзя.
fn mutate_d17(root: &Path) -> std::result::Result<(), String> {
    let adrs = files_with_prefix(root, "docs/adr", "ADR-");
    for rel in adrs {
        let text = read(root, rel.as_str())?;
        if !text.contains("Модель-автор:") && !text.contains("Author-model:") {
            continue;
        }
        // Меняем ЗНАЧЕНИЕ метки на другую модель: редакция документа другая.
        // Подстановка по одной строке, а не цепочкой replace — цепочка
        // превратила бы последовательные замены в одну и уехала бы дальше.
        let mut replaced = String::with_capacity(text.len());
        let mut done = false;
        for line in text.lines() {
            let is_author = line.contains("Модель-автор:") || line.contains("Author-model:");
            if is_author && !done {
                let (head, _) = line.split_once(':').expect("поле с двоеточием");
                let new_label = if line.contains("human") {
                    "claude-opus-4"
                } else {
                    "glm-5.2"
                };
                let _ = write!(replaced, "{head}: {new_label}");
                done = true;
            } else {
                replaced.push_str(line);
            }
            replaced.push('\n');
        }
        if !done {
            return Err(format!("метку автора в {rel} заменить не удалось"));
        }
        return write(root, rel.as_str(), &replaced);
    }
    // Шапки без поля автора: добавляем строку — документ тоже меняется.
    let adrs = files_with_prefix(root, "docs/adr", "ADR-");
    let Some(rel) = adrs.first() else {
        return Err("нет ADR".to_string());
    };
    let text = read(root, rel.as_str())?;
    let Some((head, tail)) = text.split_once("\n\n") else {
        return Err("шапка ADR не распознана".to_string());
    };
    write(
        root,
        rel.as_str(),
        &format!("{head}\n- Модель-автор: claude-opus-4\n\n{tail}"),
    )
}

// ---------------------------------------------------------------------------
// D19–D24 (волна E, E1): кодовые классы корпуса openspec-vs-spine
// ---------------------------------------------------------------------------
//
// Корпус `experiments/openspec-vs-spine/` (180 генераций, разрез по правилам
// CONSTRAINTS.massrun.yaml): классы нарушений, которые агенты реально пишут в
// код: f64 для денег (S-01/M-01), unwrap в денежном пути (S-03/M-03),
// отсутствие ключа идемпотентности (S-02/M-02), ошибки строками (S-05/M-09),
// ПДн в логах (S-04/M-04), секрет литералом (S-08/M-08). Каждый мутатор
// кладёт в скелет файл с дефектом того же вида; ожидание динамическое
// (`expected_in`): правило класса есть в реестре с severity error и покрывает
// файл — `Caught`, иначе — `Semantic` (не вина механики гейта).

/// Путь и содержимое файла-мутанта кодового класса (E1).
struct CodeDefect {
    /// Относительный путь в скелете кейса.
    rel: &'static str,
    /// Содержимое с одним засеянным дефектом класса.
    content: &'static str,
}

/// D19: деньги в f64 — дрейф округления в денежном пути (S-01 корпуса).
const D19: CodeDefect = CodeDefect {
    rel: "skeleton/redteam/d19_money_f64.rs",
    content: "// D19: сумма платежа в f64 — дрейф округления в денежном пути (класс S-01).\n\
              /// Итог по частям платежа в рублях.\n\
              pub fn total_rub(parts: &[f64]) -> f64 {\n    parts.iter().sum()\n}\n",
};

/// D20: unwrap в денежном пути — паника вместо типизированной ошибки (S-03).
const D20: CodeDefect = CodeDefect {
    rel: "skeleton/redteam/d20_unwrap_money.rs",
    content: "// D20: unwrap в денежном пути — паника на вводе вместо отказа (класс S-03).\n\
              /// Разбор суммы поручения в копейках.\n\
              pub fn parse_minor_units(raw: &str) -> u64 {\n    raw.trim().parse().unwrap()\n}\n",
};

/// D21: ошибка строкой — вызывающий не может различить отказы (S-05/M-09).
const D21: CodeDefect = CodeDefect {
    rel: "skeleton/redteam/d21_string_errors.rs",
    content: "// D21: ошибка строкой — отказ не разобрать машинно (класс S-05).\n\
              pub fn authorize(limit: u64) -> Result<u64, String> {\n    \
              if limit == 0 {\n        return Err(\"лимит исчерпан\".to_string());\n    }\n    \
              Ok(limit)\n}\n",
};

/// D22: персональные данные в логах — номер карты в журнале приложения (S-04/M-04).
const D22: CodeDefect = CodeDefect {
    rel: "skeleton/redteam/d22_pii_in_logs.rs",
    content: "// D22: ПДн в логах: номер карты в журнале приложения (класс S-04).\n\
              pub fn log_auth_attempt(card_number: &str) {\n    \
              log::info!(\"auth attempt card_number={card_number}\");\n}\n",
};

/// D23: секрет литералом в коде (S-08/M-08).
const D23: CodeDefect = CodeDefect {
    rel: "skeleton/redteam/d23_hardcoded_secret.rs",
    content: "// D23: секрет литералом в коде (класс S-08).\n\
              pub const PLATFORM_API_KEY: &str = \"sk-live-0123456789abcdef\";\n",
};

/// D24: обработчик уведомления без ключа идемпотентности — повторная доставка
/// создаёт второй эффект (S-02). Ловится `each_file_must_contain` по
/// обязательному маркеру в каждом обработчике; `must_contain` новый файл без
/// маркера не видит (остальные файлы совпадают) — это и есть слепая зона.
const D24: CodeDefect = CodeDefect {
    rel: "skeleton/redteam/d24_no_idempotency_key.rs",
    content: "// D24: обработчик уведомления о платеже: дедупликации нет — повторная\n\
              // доставка создаёт второй эффект (класс S-02).\n\
              pub struct Notification {\n    pub payment_id: String,\n    pub amount_minor: u64,\n}\n\n\
              pub fn on_payment_notify(n: &Notification) -> u64 {\n    \
              ledger::append(&n.payment_id, n.amount_minor)\n}\n",
};

/// Правка кодового мутатора корпуса: файл с дефектом кладётся в скелет.
fn mutate_code_defect(root: &Path, defect: &CodeDefect) -> std::result::Result<(), String> {
    write(root, defect.rel, defect.content)
}

/// Ожидание кодового мутатора корпуса по составу правил кейса: правило
/// класса (error-severity `must_not_contain` по содержимому либо
/// `each_file_must_contain` по отсутствию маркера) покрывает файл → `Caught`.
fn expect_code_defect(root: &Path, defect: &CodeDefect) -> Expectation {
    match crate::control::teeth::catching_rule(root, defect.rel, defect.content) {
        Some(_) => Expectation::Caught,
        None => Expectation::Semantic,
    }
}

fn mutate_d19(root: &Path) -> std::result::Result<(), String> {
    mutate_code_defect(root, &D19)
}
fn expect_d19(root: &Path) -> Expectation {
    expect_code_defect(root, &D19)
}

fn mutate_d20(root: &Path) -> std::result::Result<(), String> {
    mutate_code_defect(root, &D20)
}
fn expect_d20(root: &Path) -> Expectation {
    expect_code_defect(root, &D20)
}

fn mutate_d21(root: &Path) -> std::result::Result<(), String> {
    mutate_code_defect(root, &D21)
}
fn expect_d21(root: &Path) -> Expectation {
    expect_code_defect(root, &D21)
}

fn mutate_d22(root: &Path) -> std::result::Result<(), String> {
    mutate_code_defect(root, &D22)
}
fn expect_d22(root: &Path) -> Expectation {
    expect_code_defect(root, &D22)
}

fn mutate_d23(root: &Path) -> std::result::Result<(), String> {
    mutate_code_defect(root, &D23)
}
fn expect_d23(root: &Path) -> Expectation {
    expect_code_defect(root, &D23)
}

fn mutate_d24(root: &Path) -> std::result::Result<(), String> {
    mutate_code_defect(root, &D24)
}
fn expect_d24(root: &Path) -> Expectation {
    expect_code_defect(root, &D24)
}

// ---------------------------------------------------------------------------
// D18 (волна B, B2): лексический обход — слово на месте, логики нет
// ---------------------------------------------------------------------------

/// Цель D18: реализация применённого шаблона (кейс подписан на зубья
/// библиотеки и взял её правило в реестр) либо файл, охраняемый текстовым
/// `must_contain`. Выбор ветки НЕ читает содержимого файлов (только lock,
/// реестр, наличие файла): ожидание, вычисленное на мутанте после правки,
/// обязано совпадать с веткой самой правки.
enum D18Target {
    /// Файл реализации применённого шаблона (rel-путь) и слово-маркер,
    /// остающееся в комментарии.
    Template {
        /// Относительный путь файла реализации.
        rel: String,
        /// Ключевое слово, остающееся в комментарии.
        keyword: String,
    },
    /// Файл с совпадением `must_contain` (rel-путь) и сам pattern.
    Text {
        /// Относительный путь файла с проверкой.
        rel: String,
        /// Pattern правила (слово обязано остаться находимым).
        pattern: String,
    },
}

/// Есть ли у кейса проводимое правило из шаблона: lock-запись, шаблон в
/// сборке и правило `command_succeeds` с той же командой в реестре.
fn d18_template_entry(root: &Path) -> Option<(crate::rule_templates::LockEntry, String)> {
    let lock_rel = crate::rule_templates::LOCK_REL;
    let text = std::fs::read_to_string(root.join(lock_rel)).ok()?;
    let lock: TemplateLock = serde_yaml_ng::from_str(&text).ok()?;
    let constraints = crate::control::resolve_constraints_path(root, None)?;
    let cards = crate::control::rule_cards(&constraints).ok()?;
    for entry in lock.templates {
        let Ok(Some(t)) = crate::rule_templates::template(&entry.id) else {
            continue;
        };
        if t.manifest.version != entry.version {
            continue;
        }
        let wired = cards.iter().any(|c| {
            c.kind == "command_succeeds" && c.command.as_deref() == Some(entry.command.as_str())
        });
        if !wired {
            continue;
        }
        let lang = crate::rule_templates::Lang::parse(&entry.lang)
            .unwrap_or(crate::rule_templates::Lang::Both);
        let swaps = t.violating_for(lang);
        let Some(swap) = swaps.first() else {
            continue;
        };
        let rel = if entry.dir.is_empty() {
            format!(
                "{}/{}/{}",
                crate::rule_templates::TARGET_REL,
                entry.id,
                swap.to
            )
        } else {
            format!("{}/{}", entry.dir, swap.to)
        };
        if root.join(&rel).is_file() {
            return Some((entry, rel));
        }
    }
    None
}

/// Выбор цели D18: шаблонная ветка при наличии проведённого правила из
/// шаблона, иначе — первый (по реестру и алфавиту файлов) `must_contain`,
/// чей pattern реально встречается в файле набора.
fn d18_target(root: &Path) -> Option<D18Target> {
    if let Some((entry, rel)) = d18_template_entry(root) {
        // Слово-маркер: если текстовое правило покрывает тот же файл, берём
        // его образец (оно останется зелёным — в этом и демонстрация); иначе
        // имя шаблона.
        let keyword = crate::control::teeth::first_must_contain_match(root, &rel).map_or_else(
            || entry.id.replace('-', "_"),
            |(_, pattern)| crate::control::teeth::regex_specimen(&pattern).unwrap_or(pattern),
        );
        return Some(D18Target::Template { rel, keyword });
    }
    crate::control::teeth::first_must_contain_match(root, "")
        .map(|(rel, pattern)| D18Target::Text { rel, pattern })
}

/// Комментарийный префикс по расширению файла (`//` для C-семейства).
fn comment_prefix(rel: &str) -> &'static str {
    match rel.rsplit('.').next() {
        Some("rs" | "go" | "java" | "ts" | "js" | "c" | "cpp" | "h") => "//",
        _ => "#",
    }
}

/// D18: в коде скелета проверка удаляется, а ключевое слово остаётся в
/// комментарии. Для шаблонной ветки — реализация заменяется файлом-комментарием
/// (тест шаблона обязан упасть: `Caught`); для текстовой — строки с
/// совпадением удаляются, а в конец файла дописывается комментарий с образцом,
/// по-прежнему совпадающим с pattern (`must_contain` остаётся зелёным:
/// `Semantic` — текстовое правило обходится лексически).
fn mutate_d18(root: &Path) -> std::result::Result<(), String> {
    match d18_target(root) {
        Some(D18Target::Template { rel, keyword }) => write(
            root,
            &rel,
            &format!(
                "{} {keyword}: проверка выполняется здесь — слово на месте, логики нет (D18)\n",
                comment_prefix(&rel)
            ),
        ),
        Some(D18Target::Text { rel, pattern }) => {
            let text = read(root, &rel)?;
            let re = regex::Regex::new(&pattern).map_err(|e| format!("{pattern}: {e}"))?;
            let specimen = crate::control::teeth::regex_specimen(&pattern)
                .ok_or_else(|| format!("pattern '{pattern}': образец не синтезируется"))?;
            let kept: Vec<&str> = text.lines().filter(|l| !re.is_match(l)).collect();
            if kept.len() == text.lines().count() {
                return Err(format!("{rel}: совпадений по строкам нет (мультилиния?)"));
            }
            let mut new = kept.join("\n");
            let _ = writeln!(
                new,
                "\n{} {specimen} — проверка удалена: слово на месте, логики нет (D18)",
                comment_prefix(&rel)
            );
            write(root, &rel, &new)
        }
        None => Err(
            "нет цели: ни применённого шаблона с проведённым правилом, ни must_contain с совпадением в коде"
                .to_string(),
        ),
    }
}

/// Ожидание D18 по составу правил кейса: `Caught` для правила из шаблона
/// (проверка исполнением падает на отсутствующей логике), `Semantic` — если
/// инвариант охраняет только текст (слово на месте — правило зелёное).
fn expect_d18(root: &Path) -> Expectation {
    match d18_target(root) {
        Some(D18Target::Template { .. }) => Expectation::Caught,
        _ => Expectation::Semantic,
    }
}

/// Лок-файл применённых шаблонов, разобранный на стороне мутатора: в
/// [`crate::rule_templates`] тип лока и его чтение приватны, а править
/// библиотеку шаблонов ради мутатора нельзя — D15 обязан быть её
/// потребителем, как и любой другой вызов `arch-be rules template`.
#[derive(Debug, serde::Deserialize)]
struct TemplateLock {
    #[serde(default)]
    templates: Vec<crate::rule_templates::LockEntry>,
}

/// D15: нарушение инварианта в реализации скелета.
///
/// Вход — применённый шаблон (`.arch-handoff/rule-templates.lock`): кейс взял
/// библиотечное правило `command_succeeds` и подписался на его зубы. Мутант
/// делает ровно то, что обязана ловить проверка зубов, но уже в самом кейсе:
/// подменяет эталонную реализацию (`reference_impl.py`) нарушающей из ТОЙ ЖЕ
/// библиотеки, взятой по `id`/`version` из лока. Тест шаблона обязан упасть,
/// правило — стать красным, то есть дефект обязан поймать `fitness`.
///
/// Модель при этом не правится: инвариант в `model/` описывает свойство,
/// которое реализация теперь нарушает, — это и есть засеянный дефект. Правка
/// модели убрала бы само противоречие, которое мутант сеет.
///
/// Честная граница ожидания: правило обязано ПОКРАСНЕТЬ (команда шаблона
/// падает), но вердикт видит его только в `error`-severity — находка `warn`
/// не переводит `FitnessReport.passed` в `false`, а гейт считает составляющую
/// `fitness` упавшей ровно по `passed` (см. `src/gate.rs`). Кейс, оставивший
/// применённое правило предупреждением (таково умолчание фрагмента `apply`),
/// получит в карте «не пойман», и это утверждение о решении кейса, а не о
/// механике: инвариант, взятый шаблоном, там не защищает вердикт.
///
/// Без лока вход не найден (шаблоны не применялись — подменять нечего):
/// `Err(причина)`, мутатор пропускается и в знаменатель доли не входит.
fn mutate_d15(root: &Path) -> std::result::Result<(), String> {
    let rel = crate::rule_templates::LOCK_REL;
    if !root.join(rel).is_file() {
        return Err(format!("нет {rel} — шаблоны не применены, нарушать нечего"));
    }
    let lock: TemplateLock = serde_yaml_ng::from_str(&read(root, rel)?)
        .map_err(|e| format!("{rel}: не разбирается: {e}"))?;
    if lock.templates.is_empty() {
        return Err(format!("{rel}: применённых шаблонов нет"));
    }
    let mut applied = 0_usize;
    for entry in &lock.templates {
        let lang = crate::rule_templates::Lang::parse(&entry.lang)
            .map_err(|e| format!("{}: {e}", entry.id))?;
        let Some(t) =
            crate::rule_templates::template(&entry.id).map_err(|e| format!("{}: {e}", entry.id))?
        else {
            continue; // шаблона нет в этой сборке — подменять нечем
        };
        if t.manifest.version != entry.version {
            continue; // применена другая версия — нарушающая реализация не та
        }
        let dir = if entry.dir.is_empty() {
            format!("{}/{}", crate::rule_templates::TARGET_REL, entry.id)
        } else {
            entry.dir.clone()
        };
        // Куда `apply` положил файлы, туда же кладётся и подмена (тот же
        // `dir`, то же `to`, что и у `ViolatingSwap`).
        for swap in t.violating_for(lang) {
            let content = t
                .file(&swap.from)
                .ok_or_else(|| format!("{}: нет файла '{}'", entry.id, swap.from))?;
            write(root, &format!("{dir}/{}", swap.to), content)?;
            applied += 1;
        }
    }
    if applied == 0 {
        return Err(format!(
            "{rel}: нет позиции, для которой есть нарушающая реализация"
        ));
    }
    Ok(())
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

#[cfg(test)]
mod corner_tests {
    use std::path::PathBuf;

    use super::*;
    use crate::redteam::{Detection, RedteamReport, load_summary, save_summary};

    /// Кладёт в кейс применённый шаблон библиотеки (`files_for(Python)`) и
    /// возвращает запись лока на него — так выглядит кейс, взявший шаблон
    /// `arch-be rules template apply`.
    fn apply_python_template(case: &Path, id: &str) -> String {
        let t = crate::rule_templates::template(id)
            .expect("сборка")
            .expect("шаблон есть в сборке");
        let dir = format!("{}/{}", crate::rule_templates::TARGET_REL, id);
        let mut lock = format!(
            "  - id: {id}\n    version: {}\n    ad: AD-1\n    lang: python\n    dir: {dir}\n    \
             command: 'python3 -m pytest -q -p no:cacheprovider {dir}/test_idempotency_key.py'\n    \
             files:\n",
            t.manifest.version
        );
        for f in t.files_for(crate::rule_templates::Lang::Python) {
            let content = t.file(&f.from).expect("файл шаблона");
            let rel = format!("{dir}/{}", f.to);
            std::fs::create_dir_all(case.join(&dir)).expect("mkdir");
            std::fs::write(case.join(&rel), content).expect("write");
            let _ = write!(
                lock,
                "      - path: {rel}\n        sha256: {}\n",
                crate::hash::sha256_hex(content.as_bytes())
            );
        }
        lock
    }

    /// D15 (вход): без лока мутатор не применим — шаблоны не применялись,
    /// нарушать нечего. Пропуск честный: `Err(причина)`, а не «поймано» и не
    /// «дыра в защите». Ровно этот случай — эталонные кейсы репозитория.
    #[test]
    fn d15_is_not_applicable_without_a_lock() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        let reason = mutate_d15(&case).expect_err("без лока вход не найден");
        assert!(reason.contains(crate::rule_templates::LOCK_REL), "{reason}");
        // Каталог при этом держит позицию вне знаменателя доли: она видна в
        // карте обнаружения, но критерий приёмки «≥ 11 из 14» не двигает.
        let d15 = MUTATORS
            .iter()
            .find(|m| m.id == "D15")
            .expect("D15 в каталоге");
        assert_eq!(d15.expected, Expectation::Caught);
        assert!(!d15.in_ratio);
    }

    /// D15 (правка): эталонная реализация применённого шаблона заменяется
    /// нарушающей из той же библиотеки — тест шаблона обязан на ней упасть,
    /// то есть правило `command_succeeds` обязано покраснеть (`fitness`).
    /// Проверяется сама подмена: прогон правила требует интерпретатора.
    #[test]
    fn d15_replaces_the_reference_impl_with_the_violating_one() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        let entry = apply_python_template(&case, "idempotency-key");
        let lock_rel = crate::rule_templates::LOCK_REL;
        let lock_path = case.join(lock_rel);
        std::fs::create_dir_all(lock_path.parent().expect("каталог лока")).expect("mkdir");
        std::fs::write(&lock_path, format!("templates:\n{entry}")).expect("lock");
        let dir = format!("{}/idempotency-key", crate::rule_templates::TARGET_REL);
        let before =
            std::fs::read_to_string(case.join(format!("{dir}/reference_impl.py"))).expect("эталон");
        mutate_d15(&case).expect("мутант D15");
        let after = std::fs::read_to_string(case.join(format!("{dir}/reference_impl.py")))
            .expect("подмена");
        let t = crate::rule_templates::template("idempotency-key")
            .expect("сборка")
            .expect("шаблон");
        let violating = t
            .file("python/violating_impl.py")
            .expect("нарушающая реализация");
        assert_eq!(after, violating, "реализация обязана стать нарушающей");
        assert_ne!(before, after, "подмена обязана что-то изменить");
        assert!(
            after.contains("дедупликации нет"),
            "взята именная нарушающая реализация, а не любая: {after}"
        );
        // Остальные файлы шаблона не тронуты: дефект ровно один.
        let test = std::fs::read_to_string(case.join(format!("{dir}/test_idempotency_key.py")))
            .expect("тест");
        assert_eq!(
            test,
            t.file("python/test_idempotency_key.py")
                .expect("тест шаблона")
        );
        // На этом файле тест шаблона обязан упасть — иначе правило беззубое
        // (та же посылка, что у `executable_rule_toothless`).
        assert!(
            violating.contains("send(key, amount)"),
            "нарушающая реализация шлёт повторный эффект"
        );
    }

    /// Отчёт рубрики с сырыми ответами: ровно то, к чему применимы мутаторы
    /// красного угла.
    fn case_with_report(root: &Path) {
        write(
            root,
            "reports/rubric/ADR-001-demo.json",
            "{\n  \"schema\": \"arch-be/rubric-report/v1\",\n  \"rubric\": \"adr_quality\",\n  \
             \"judge_model\": \"glm-5.2\",\n  \"weighted_total\": 4.0,\n  \"verdict\": \"годно\",\n  \
             \"scores\": [{\"criterion_id\": \"context\", \"score\": 4, \"flags\": []}]\n}\n",
        )
        .expect("отчёт");
        write(
            root,
            "reports/rubric/raw/ADR-001-demo/sample-1.json",
            "{\"text\": \"ответ\"}\n",
        )
        .expect("сырой ответ");
    }

    /// D16 поднимает балл критерия и взвешенный итог — расхождение с сырыми
    /// ответами, из которых отчёт объявлен собранным (ADR-048).
    #[test]
    fn d16_raises_the_recorded_score() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        case_with_report(root);
        mutate_d16(root).expect("мутация применима");
        let text =
            std::fs::read_to_string(root.join("reports/rubric/ADR-001-demo.json")).expect("отчёт");
        let artifact: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(artifact["scores"][0]["score"], 5, "{artifact}");
        assert_eq!(artifact["weighted_total"], 5.0, "{artifact}");
    }

    /// Без сырых ответов сверять не с чем: мутатор честно неприменим, а не
    /// «пойман».
    #[test]
    fn d16_is_inapplicable_without_raw_answers() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "reports/rubric/ADR-001-demo.json",
            "{\"schema\": \"arch-be/rubric-report/v1\", \"weighted_total\": 4.0}\n",
        )
        .expect("отчёт");
        assert!(
            mutate_d16(root).is_err(),
            "без сырых ответов мутатор обязан пропускаться"
        );
        assert!(
            mutate_d16(tmp.path()).is_err(),
            "без отчётов мутатор тоже неприменим"
        );
    }

    /// D17 меняет метку автора в шапке ADR: редакция документа другая, отчёт
    /// от прежней становится устаревшим (ADR-048).
    #[test]
    fn d17_rewrites_the_author_header() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        write(
            root,
            "docs/adr/ADR-001-demo.md",
            "# ADR-001. Решение\n\n- Статус: Accepted\n- Модель-автор: human\n\n## Context\n\nПричина.\n",
        )
        .expect("ADR");
        mutate_d17(root).expect("мутация применима");
        let text = std::fs::read_to_string(root.join("docs/adr/ADR-001-demo.md")).expect("ADR");
        assert!(
            text.contains("Модель-автор: claude-opus-4"),
            "метка заменена: {text}"
        );
        assert!(!text.contains("human"), "старой метки нет: {text}");
    }

    /// Оба мутатора стоят вне знаменателя доли обнаружения: набор раздела 7
    /// (11 из 14) не меняется, они видны отдельными строками — как R и D14.
    #[test]
    fn corner_mutators_are_outside_the_ratio() {
        for id in ["D16", "D17"] {
            let m = MUTATORS.iter().find(|m| m.id == id).expect("мутатор");
            assert!(!m.in_ratio, "{id} не должен входить в долю обнаружения");
            assert_eq!(m.expected, Expectation::Caught, "{id} обязан ловиться");
        }
    }

    // --- B2: D18 «слово на месте, логики нет» ---------------------------------

    /// D18 в каталоге: кодовый слой, вне знаменателя (как D15 — он измеряет
    /// ловлю по классу дефекта, а не набор раздела 7), ожидание динамическое.
    #[test]
    fn d18_is_in_the_catalog_as_a_dynamic_code_mutator() {
        let m = MUTATORS.iter().find(|m| m.id == "D18").expect("D18");
        assert_eq!(m.layer, Layer::Code);
        assert!(!m.in_ratio, "D18 — отдельная строка, не знаменатель");
        assert!(m.expected_in.is_some(), "ожидание по составу правил кейса");
        // Статический дефолт честен: на кейсе без правила класса — не ловится.
        assert_eq!(m.expected, Expectation::Semantic);
    }

    /// Ветка «только текст»: файл, охраняемый одним `must_contain`. После D18
    /// строки-проверки удалены, слово осталось в комментарии — правило зелёное
    /// (гейт дефект не видит), ожидание `Semantic`.
    #[test]
    fn d18_lexical_bypass_keeps_text_rule_green() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        write(
            case,
            "CONSTRAINTS.yaml",
            "rules:\n  - id: C-001\n    name: idem_present\n    type: must_contain\n    \
             glob: 'skeleton/**/*.py'\n    pattern: 'idempotency_key'\n    severity: error\n",
        )
        .expect("реестр");
        write(
            case,
            "skeleton/api.py",
            "def take(idempotency_key, amount):\n    if idempotency_key in seen:\n        return seen[idempotency_key]\n    return send(amount)\n",
        )
        .expect("скелет");
        let constraints = case.join("CONSTRAINTS.yaml");
        let before = crate::control::check(case, &constraints).expect("гейт до");
        assert!(before.passed, "до мутации зелёный: {:?}", before.issues);

        mutate_d18(case).expect("мутация применима");
        let text = std::fs::read_to_string(case.join("skeleton/api.py")).expect("файл");
        assert!(
            !text.contains("if idempotency_key in seen"),
            "проверка удалена: {text}"
        );
        assert!(
            text.contains("# idempotency_key"),
            "слово осталось в комментарии: {text}"
        );
        // Текстовое правило остаётся зелёным — дефект оно не видит.
        let after = crate::control::check(case, &constraints).expect("гейт после");
        assert!(
            after.passed,
            "must_contain не различает код и комментарий: {:?}",
            after.issues
        );
        assert_eq!(expect_d18(case), Expectation::Semantic);
    }

    /// Ветка шаблона: правило из библиотеки (команда следит за конструкцией
    /// проверки, а не за словом). После D18 команда падает — `Caught`.
    #[test]
    fn d18_is_caught_when_guarded_by_a_template_rule() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        let t = crate::rule_templates::template("idempotency-key")
            .expect("сборка")
            .expect("шаблон");
        let dir = format!("{}/idempotency-key", crate::rule_templates::TARGET_REL);
        let command = format!("grep -q _answers {dir}/reference_impl.py");
        let mut files_yaml = String::new();
        for f in t.files_for(crate::rule_templates::Lang::Python) {
            let content = t.file(&f.from).expect("файл шаблона");
            let rel = format!("{dir}/{}", f.to);
            write(case, &rel, content).expect("файл");
            let _ = write!(
                files_yaml,
                "      - path: {rel}\n        sha256: {}\n",
                crate::hash::sha256_hex(content.as_bytes())
            );
        }
        write(
            case,
            crate::rule_templates::LOCK_REL,
            &format!(
                "templates:\n  - id: idempotency-key\n    version: {}\n    ad: AD-1\n    \
                 lang: python\n    dir: {dir}\n    command: '{command}'\n    files:\n{files_yaml}",
                t.manifest.version
            ),
        )
        .expect("lock");
        write(
            case,
            "CONSTRAINTS.yaml",
            &format!(
                "rules:\n  - id: C-100\n    name: idempotency_key_enforced\n    \
                 type: command_succeeds\n    command: '{command}'\n    severity: error\n"
            ),
        )
        .expect("реестр");
        let constraints = case.join("CONSTRAINTS.yaml");
        let before = crate::control::check(case, &constraints).expect("гейт до");
        assert!(before.passed, "до мутации зелёный: {:?}", before.issues);
        assert_eq!(expect_d18(case), Expectation::Caught, "правило из шаблона");

        mutate_d18(case).expect("мутация применима");
        let gutted =
            std::fs::read_to_string(case.join(format!("{dir}/reference_impl.py"))).expect("файл");
        assert!(gutted.contains("слово на месте, логики нет"), "{gutted}");
        let after = crate::control::check(case, &constraints).expect("гейт после");
        assert!(
            !after.passed,
            "правило из шаблона обязано покраснеть на пустышке: {:?}",
            after.issues
        );
        assert!(
            after
                .issues
                .iter()
                .any(|i| i.rule == "idempotency_key_enforced"),
            "поймавшее правило названо: {:?}",
            after.issues
        );
    }

    /// Ни шаблона, ни `must_contain` с совпадением — D18 честно неприменим.
    #[test]
    fn d18_is_skipped_without_a_target() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path();
        write(case, "CONSTRAINTS.yaml", "rules: []\n").expect("реестр");
        let reason = mutate_d18(case).expect_err("цели нет");
        assert!(reason.contains("нет цели"), "{reason}");
    }

    // --- E1: кодовые мутаторы корпуса openspec-vs-spine (D19–D24) -----------

    /// Шесть классов корпуса в каталоге: кодовый слой, вне знаменателя доли,
    /// динамическое ожидание (честно по составу правил кейса).
    #[test]
    fn corpus_mutators_are_in_the_catalog_with_dynamic_expectation() {
        for id in ["D19", "D20", "D21", "D22", "D23", "D24"] {
            let m = MUTATORS.iter().find(|m| m.id == id).expect("мутатор");
            assert_eq!(m.layer, Layer::Code, "{id}");
            assert!(!m.in_ratio, "{id} — отдельная строка, не знаменатель");
            assert!(m.expected_in.is_some(), "{id}: ожидание по правилам кейса");
            assert_eq!(m.expected, Expectation::Semantic, "{id}: дефолт честный");
        }
    }

    /// Кейс с одним правилом (паттерны зеркалят замороженный
    /// `experiments/openspec-vs-spine/CONSTRAINTS.massrun.yaml`).
    fn corpus_case(rules_yaml: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tmp");
        write(tmp.path(), "CONSTRAINTS.yaml", rules_yaml).expect("реестр");
        tmp
    }

    /// (мутатор, pattern правила класса, тип правила) — pattern `None` у D24:
    /// дефект «нет ключа идемпотентности» ловится ОТСУТСТВИЕМ маркера.
    const CORPUS: [(&str, Option<&str>, &str); 6] = [
        ("D19", Some(r"\bf(64|32)\b"), "must_not_contain"),
        (
            "D20",
            Some(r"\.unwrap\(\)|\.expect\(|panic!|unreachable!|todo!|unimplemented!"),
            "must_not_contain",
        ),
        (
            "D21",
            Some(r#"Err\(\s*"|Box<dyn\s+(std::)?error::Error"#),
            "must_not_contain",
        ),
        (
            "D22",
            Some(
                r"(?i)(println!|print!|log::(info|debug|warn|error|trace)!|tracing::(info|debug|warn|error)!|eprintln!)[^\n]*(card_number|full_pan|\bpan\b|holder|cardholder|фио|full_name|card_num)",
            ),
            "must_not_contain",
        ),
        (
            "D23",
            Some(
                r#"(?i)(password|api_key|secret_key|secret|token)\s*(:\s*&str)?\s*=\s*"[^"]{6,}""#,
            ),
            "must_not_contain",
        ),
        ("D24", None, "each_file_must_contain"),
    ];

    /// Каждый мутатор сеет дефект ровно своего класса корпуса: содержимое
    /// совпадает с шаблоном класса (D24 — наоборот: маркера в файле нет).
    #[test]
    fn corpus_mutants_seed_their_corpus_class() {
        for (id, pattern, _) in CORPUS {
            let m = MUTATORS.iter().find(|m| m.id == id).expect("мутатор");
            let tmp = tempfile::tempdir().expect("tmp");
            (m.apply)(tmp.path()).expect("мутация применима");
            let rel = match id {
                "D19" => D19.rel,
                "D20" => D20.rel,
                "D21" => D21.rel,
                "D22" => D22.rel,
                "D23" => D23.rel,
                _ => D24.rel,
            };
            let content = std::fs::read_to_string(tmp.path().join(rel)).expect("файл мутанта");
            let class_pattern = match id {
                "D24" => "(?i)idempotenc",
                _ => pattern.expect("pattern класса"),
            };
            let re = regex::Regex::new(class_pattern).expect("regex класса");
            if id == "D24" {
                assert!(
                    !re.is_match(&content),
                    "{id}: маркера в обработчике быть не должно"
                );
            } else {
                assert!(
                    re.is_match(&content),
                    "{id}: дефект обязан совпадать с классом {class_pattern}"
                );
            }
        }
    }

    /// Ожидание честно по составу правил кейса: с правилом класса (severity
    /// error, glob покрывает файл) — `Caught` и гейт красный; без правила —
    /// `Semantic` и гейт зелёный.
    #[test]
    fn corpus_mutants_expectation_follows_the_registry() {
        for (id, pattern, kind) in CORPUS {
            let m = MUTATORS.iter().find(|m| m.id == id).expect("мутатор");
            let class_pattern = pattern.unwrap_or("(?i)idempotenc");
            // Кейс с правилом класса: severity error, glob покрывает скелет.
            let with = corpus_case(&format!(
                "rules:\n  - id: C-100\n    name: corpus_guard\n    type: {kind}\n    \
                 glob: '**/*.rs'\n    pattern: '{class_pattern}'\n    severity: error\n"
            ));
            let expect = m.expected_in.expect("динамическое ожидание");
            // Ожидание вычисляется на мутанте ПОСЛЕ правки (как в прогоне).
            (m.apply)(with.path()).expect("мутация применима");
            assert_eq!(expect(with.path()), Expectation::Caught, "{id} с правилом");
            let report = crate::control::check(with.path(), &with.path().join("CONSTRAINTS.yaml"))
                .expect("гейт");
            assert!(
                !report.passed && report.issues.iter().any(|i| i.rule == "corpus_guard"),
                "{id}: правило класса обязано покраснеть: {:?}",
                report.issues
            );

            // Кейс без правила класса: честный Semantic, гейт зелёный.
            let without = corpus_case(
                "rules:\n  - id: C-101\n    name: readme_present\n    type: file_exists\n    \
                 path: 'README.md'\n    severity: error\n",
            );
            write(without.path(), "README.md", "x\n").expect("readme");
            (m.apply)(without.path()).expect("мутация применима");
            assert_eq!(
                expect(without.path()),
                Expectation::Semantic,
                "{id} без правила"
            );
            let report =
                crate::control::check(without.path(), &without.path().join("CONSTRAINTS.yaml"))
                    .expect("гейт");
            assert!(report.passed, "{id}: гейт зелёный — дефект никто не ловит");
        }
    }

    /// warn-правило класса дефекта поимкой не считается: оно не краснит гейт.
    #[test]
    fn corpus_mutants_warn_rule_does_not_count_as_catching() {
        let m = MUTATORS.iter().find(|m| m.id == "D19").expect("мутатор");
        let case = corpus_case(
            "rules:\n  - id: C-102\n    name: soft_guard\n    type: must_not_contain\n    \
             glob: '**/*.rs'\n    pattern: '\\bf(64|32)\\b'\n    severity: warn\n",
        );
        (m.apply)(case.path()).expect("мутация применима");
        let expect = m.expected_in.expect("динамическое ожидание");
        assert_eq!(
            expect(case.path()),
            Expectation::Semantic,
            "warn-правило не краснит гейт — поимкой не считается"
        );
    }

    // --- E2: две доли вместо одной ---------------------------------------------

    /// Детекция для фикстуры долей.
    fn det(id: &str, layer: Layer, expected: Expectation, caught: bool) -> Detection {
        Detection {
            id: id.into(),
            title: id.into(),
            expected,
            layer,
            caught_by: caught.then(|| "fitness".to_string()),
            skipped: None,
            expected_by: "fitness".into(),
            in_ratio: true,
        }
    }

    /// Доли по слоям считаются раздельно: документы+модель и код — по своим
    /// знаменателям; семантические позиции в слоевые доли не входят.
    #[test]
    fn layer_ratios_are_computed_separately() {
        let report = RedteamReport {
            case: PathBuf::from("case"),
            detections: vec![
                det("D1", Layer::DocsModel, Expectation::Caught, true),
                det("D2", Layer::DocsModel, Expectation::Caught, true),
                det("D11", Layer::Code, Expectation::Semantic, false),
                det("D11b", Layer::Code, Expectation::Caught, true),
                det("D19", Layer::Code, Expectation::Caught, false),
            ],
            min_detection: 0.5,
            min_code_detection: None,
            control_ok: true,
            control_note: None,
            semantic_kept: Vec::new(),
            corpus: None,
        };
        assert_eq!(report.layer_counts(Layer::DocsModel), (2, 2));
        assert_eq!(
            report.layer_counts(Layer::Code),
            (1, 2),
            "D11 — семантика, не в знаменателе"
        );
        assert_eq!(report.layer_ratio(Layer::DocsModel), Some(1.0));
        assert_eq!(report.layer_ratio(Layer::Code), Some(0.5));
        // Суммарная доля — по набору раздела 7, как прежде.
        let rendered = report.render();
        assert!(
            rendered.contains("доля по слою «документы+модель»: 2/2 = 100%"),
            "{rendered}"
        );
        assert!(
            rendered.contains("доля по слою «код»: 1/2 = 50%"),
            "{rendered}"
        );
        assert!(
            rendered.contains("порог кодовой доли: не задан"),
            "{rendered}"
        );
        // JSON несёт обе доли аддитивным ключом.
        let json = report.to_json();
        assert_eq!(json["layers"]["code"]["ratio"], 0.5);
        assert_eq!(json["layers"]["docs_model"]["caught"], 2);
        assert_eq!(json["detections"][0]["layer"], "docs_model");
    }

    /// Слой без ловимых дефектов — «не измерялось», а не 100 %; заданный порог
    /// кодовой доли при неизмеренном слое не проходит.
    #[test]
    fn code_threshold_gates_only_when_configured() {
        let mut report = RedteamReport {
            case: PathBuf::from("case"),
            detections: vec![det("D1", Layer::DocsModel, Expectation::Caught, true)],
            min_detection: 0.5,
            min_code_detection: None,
            control_ok: true,
            control_note: None,
            semantic_kept: Vec::new(),
            corpus: None,
        };
        assert_eq!(report.layer_ratio(Layer::Code), None);
        assert!(report.passed(), "без порога кодовой доли — как прежде");
        report.min_code_detection = Some(0.5);
        assert!(
            !report.passed(),
            "порог задан, а кодовый слой не измерен — требование не подтверждено"
        );
        // С измеренным слоем: порог решает по кодовой доле.
        report
            .detections
            .push(det("D11b", Layer::Code, Expectation::Caught, true));
        assert!(report.passed(), "кодовая доля 100 % ≥ 50 %");
        report.min_code_detection = Some(0.75);
        report
            .detections
            .push(det("D19", Layer::Code, Expectation::Caught, false));
        assert!(
            !report.passed(),
            "кодовая доля 50 % ниже порога 75 % — прогон красный"
        );
    }

    /// `--save` пишет обе доли; файл прежней редакции (без них) читается —
    /// доли просто неизвестны (аддитивная схема, обратная совместимость).
    #[test]
    fn summary_carries_layer_ratios_and_reads_old_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let case = tmp.path().join("case");
        std::fs::create_dir_all(&case).expect("mkdir");
        let report = RedteamReport {
            case: case.clone(),
            detections: vec![
                det("D1", Layer::DocsModel, Expectation::Caught, true),
                det("D11b", Layer::Code, Expectation::Caught, true),
                det("D19", Layer::Code, Expectation::Caught, false),
            ],
            min_detection: 0.5,
            min_code_detection: None,
            control_ok: true,
            control_note: None,
            semantic_kept: Vec::new(),
            corpus: None,
        };
        let path = save_summary(&case, &report).expect("сохранение");
        let back = load_summary(&path).expect("чтение");
        assert_eq!(back.docs_model_ratio, Some(1.0));
        assert_eq!(back.code_ratio, Some(0.5));
        assert_eq!(back.code_caught, Some(1));
        assert_eq!(back.code_total, Some(2));
        assert!(back.min_code_detection.is_none());

        // Файл прежней редакции (полей слоёв нет): читается, доли None.
        std::fs::write(
            &path,
            "{\"schema\":\"arch-be/redteam/v1\",\"case\":\"кейс\",\
             \"measured_at\":\"2026-10-01T00:00:00+00:00\",\"caught\":11,\"total\":14,\
             \"ratio\":0.79,\"min_detection\":0.78,\"control_ok\":true}",
        )
        .expect("старая редакция");
        let old = load_summary(&path).expect("старая редакция читается");
        assert!(old.docs_model_ratio.is_none() && old.code_ratio.is_none());
        assert!(
            old.passed(),
            "старая редакция: вердикт по сумме, как прежде"
        );
    }
}

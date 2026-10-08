//! Принятие и отклонение предложений архитектурного диффа (K5, ADR-064).
//!
//! `accept <n…>` оформляет принятые пронумерованные предложения дельтой
//! `changes/<name>/DELTA.md` (реюз [`crate::delta`]): модель меняется ТОЛЬКО
//! через дельту — сама команда файлы `model/` не правит, а дельта покрывает
//! последующую правку для `delta guard`. Предложение с ⚠ (противоречит
//! инварианту AD) принимается, но конфликт фиксируется в дельте явно.
//! При `[delta] sources = ["openspec"]` дельта НЕ пишется — подсказка
//! оформить change `OpenSpec` (правило 9, ADR-062).
//!
//! `reject <n> --reason "…"` пишет отказ в журнал решений
//! (`.arch-handoff/arch-diff-decisions.json`, [`super::decisions`]):
//! повторно ребро не предлагается, пока не изменятся его основания
//! (`файл:строка` → `grounds_hash`). Принятые рёбра журналируются так же
//! (decision accept + имя дельты).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::decisions::{self, Decision, DecisionEntry, DecisionJournal, DecisionSource};
use super::diff::{ArchDiff, ArchDiffInput, ModelProposal, arch_diff};
use crate::control::DiffGlobs;
use crate::error::{HarnessError, Result};

/// Предел авто-суффикса имени дельты (`arch-diff-<head8>-N`): выше — просим
/// имя явно (`--name`), сто одинаковых дельт — признак петли, а не работы.
const MAX_NAME_SUFFIX: u32 = 99;

/// Вход решений: база/голова диффа и параметры маршрута (те же, что у
/// просмотра; заявленные триггеры на предложения не влияют — не принимаются).
pub struct DecideInput<'a> {
    /// База диффа (та же, что у прогона с предложениями).
    pub base: &'a str,
    /// Голова диффа (по умолчанию `HEAD`).
    pub head: Option<&'a str>,
    /// Пороги маршрута (`fast_max`, `standard_max`) из `[significance]`.
    pub limits: (usize, usize),
    /// Глобы детекторов (T-05).
    pub globs: &'a DiffGlobs,
}

/// Итог `accept`: созданная дельта и принятые предложения.
#[derive(Debug)]
pub struct AcceptReport {
    /// Имя созданной дельты (`changes/<name>/`).
    pub delta_name: String,
    /// Путь к `DELTA.md`.
    pub delta_path: PathBuf,
    /// Принятые предложения (в порядке номеров).
    pub accepted: Vec<ModelProposal>,
    /// Путь журнала решений, куда записаны accept-записи.
    pub journal_path: PathBuf,
}

/// Итог `reject`: запись журнала.
#[derive(Debug)]
pub struct RejectReport {
    /// Записанное решение.
    pub entry: DecisionEntry,
    /// Путь журнала решений.
    pub journal_path: PathBuf,
}

/// Пересчитывает дифф, как его видел пользователь (та же база → те же
/// номера предложений, уже без решённых журналом).
fn compute_diff(repo: &Path, input: &DecideInput) -> Result<ArchDiff> {
    arch_diff(
        repo,
        &ArchDiffInput {
            base: input.base,
            head: input.head,
            declared: BTreeMap::new(),
            limits: input.limits,
            globs: input.globs,
        },
    )
}

/// Выбирает предложения по номерам текущего прогона (дедуп, порядок ввода).
fn pick<'a>(diff: &'a ArchDiff, numbers: &[usize]) -> Result<Vec<&'a ModelProposal>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(numbers.len());
    for n in numbers {
        if !seen.insert(*n) {
            continue; // повтор номера — не дважды в одну дельту
        }
        let Some(p) = diff.proposals.iter().find(|p| p.n == *n) else {
            return Err(HarnessError::Control(format!(
                "предложения №{n} нет в текущем диффе (всего предложений: {}). \
                 Номера — из свежего вывода: arch-be arch-diff --base {}; \
                 по решённому ребру предложение не показывается — решения: {}",
                diff.proposals.len(),
                input_base_hint(diff),
                decisions::DECISIONS_PATH
            )));
        };
        out.push(p);
    }
    Ok(out)
}

/// Короткая метка базы диффа для сообщений об ошибке.
fn input_base_hint(diff: &ArchDiff) -> String {
    diff.base[..diff.base.len().min(12)].to_string()
}

/// Свободное имя дельты: явное `--name` или `arch-diff-<head8>`; занято —
/// суффикс `-2`, `-3`, … до [`MAX_NAME_SUFFIX`].
fn allocate_delta_name(repo: &Path, name: Option<&str>, head: &str) -> Result<String> {
    let base = match name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => format!("arch-diff-{}", &head[..head.len().min(8)]),
    };
    if crate::delta::name_free(repo, &base) {
        return Ok(base);
    }
    if name.is_some() {
        return Err(HarnessError::Control(format!(
            "имя дельты '{base}' занято (активная или архивная) — задайте другое --name"
        )));
    }
    for i in 2..=MAX_NAME_SUFFIX {
        let candidate = format!("{base}-{i}");
        if crate::delta::name_free(repo, &candidate) {
            return Ok(candidate);
        }
    }
    Err(HarnessError::Control(format!(
        "имена {base}…{base}-{MAX_NAME_SUFFIX} заняты — задайте имя явно: --name <имя>"
    )))
}

/// Основания ребра (`файл:строка`) для предложения по ребру; у узловых
/// предложений оснований нет.
fn evidence_of<'a>(diff: &'a ArchDiff, p: &ModelProposal) -> Option<&'a [String]> {
    diff.added_edges
        .iter()
        .map(|c| &c.edge)
        .find(|e| p.edge_id == super::diff::proposal_edge_id(e.kind, &e.from, &e.to))
        .map(|e| e.evidence.as_slice())
}

/// Суффикс «(основание: …)» для строки дельты, если у предложения есть ребро.
fn evidence_suffix(diff: &ArchDiff, p: &ModelProposal) -> String {
    evidence_of(diff, p).map_or_else(String::new, |ev| format!(" (основание: {})", ev.join("; ")))
}

/// Суффикс «⚠ противоречит …» для строки дельты.
fn conflict_suffix(p: &ModelProposal) -> String {
    if p.conflicts.is_empty() {
        String::new()
    } else {
        format!("  ⚠ противоречит {}", p.conflicts.join(", "))
    }
}

/// Тело дельты принятия: все обязательные секции `delta validate` без
/// заглушек; пути файлов модели — дословно (по ним `delta guard` засчитывает
/// покрытие правки), конфликты с инвариантами — отдельной секцией (явно).
fn render_delta_body(name: &str, diff: &ArchDiff, picked: &[&ModelProposal]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let date = chrono::Local::now().format("%Y-%m-%d");
    let base8 = &diff.base[..diff.base.len().min(8)];
    let head8 = &diff.head[..diff.head.len().min(8)];
    let ns: Vec<String> = picked.iter().map(|p| p.n.to_string()).collect();
    let _ = writeln!(out, "# Дельта: {name}\n");
    let _ = writeln!(out, "- Route: Standard");
    let _ = writeln!(out, "- Created: {date}");
    let _ = writeln!(
        out,
        "- Источник: `arch-be arch-diff accept {}` (дифф {base8}..{head8}, K5, ADR-064)\n",
        ns.join(" ")
    );
    let _ = writeln!(
        out,
        "## Проблема\n\nАрхитектурный дифф {base8}..{head8} показал изменения кода, не \
         отражённые в модели. Принятые предложения ниже вносят их в `model/` одним решением; \
         сама модель правится под этой дельтой (модель меняется только через дельту — \
         `delta guard` действует как обычно).\n"
    );
    let _ = writeln!(out, "## ADDED\n");
    let mut added = false;
    for p in picked {
        if p.kind == super::diff::ProposalKind::NewEntity {
            let _ = writeln!(
                out,
                "- Создать `{}` — {}{}{}",
                p.file,
                p.summary,
                conflict_suffix(p),
                evidence_suffix(diff, p)
            );
            added = true;
        }
    }
    if !added {
        let _ = writeln!(out, "- (нет)");
    }
    let _ = writeln!(out, "\n## MODIFIED\n");
    let mut modified = false;
    for p in picked {
        if p.kind == super::diff::ProposalKind::AddDependsOn {
            // summary сам начинается с пути файла (`{file}: depends_on += …`) —
            // дословно, по нему `delta guard` засчитывает покрытие правки.
            let _ = writeln!(
                out,
                "- {}{}{}",
                p.summary,
                conflict_suffix(p),
                evidence_suffix(diff, p)
            );
            modified = true;
        }
    }
    if !modified {
        let _ = writeln!(out, "- (нет)");
    }
    let _ = writeln!(out, "\n## REMOVED\n\n- (нет)\n");
    let conflicts: Vec<&&ModelProposal> =
        picked.iter().filter(|p| !p.conflicts.is_empty()).collect();
    if !conflicts.is_empty() {
        let _ = writeln!(out, "## Конфликты с инвариантами\n");
        for p in &conflicts {
            let _ = writeln!(
                out,
                "- Предложение №{} (`{}`) противоречит {} — принято с явным конфликтом; \
                 решение архитектора фиксируется этой дельтой, а не обходом.",
                p.n,
                p.file,
                p.conflicts.join(", ")
            );
        }
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "## План отката\n\nОткатить правки перечисленных файлов `model/` (git restore), \
         дельту — в `changes/archive/`; кодовая часть PR этой дельтой не затрагивается.\n"
    );
    let files: Vec<String> = picked.iter().map(|p| format!("`{}`", p.file)).collect();
    let _ = writeln!(
        out,
        "## Критерии приёмки\n\n\
         - [ ] Файлы модели приведены к принятым предложениям: {}\n\
         - [ ] `arch-be arch-diff --base {base8}` не предлагает принятые правки повторно\n\
         - [ ] `arch-be delta guard` зелёный (правки `model/` покрыты этой дельтой)",
        files.join(", ")
    );
    out
}

/// `arch-diff accept <n…>`: оформляет принятые предложения дельтой и пишет
/// accept-записи в журнал решений.
///
/// Порядок побочных эффектов: сначала дельта, потом журнал — при сбое
/// журнала дельта остаётся (она валидна), об ошибке сообщаем.
///
/// # Errors
/// Пустой список номеров; неизвестный номер; `[delta] sources = ["openspec"]`
/// (дельта не пишется — см. [`crate::delta::openspec_only_hint`]); имя дельты
/// занято; ошибки записи; ошибки диффа и журнала.
pub fn accept_proposals(
    repo: &Path,
    input: &DecideInput,
    numbers: &[usize],
    name: Option<&str>,
    source: DecisionSource,
) -> Result<AcceptReport> {
    if numbers.is_empty() {
        return Err(HarnessError::Control(
            "arch-diff accept: укажите номера предложений — их печатает \
             `arch-be arch-diff --base <база>` (секция «Предложения модели»)"
                .to_string(),
        ));
    }
    // Правило 9 (ADR-062): при sources = ["openspec"] DELTA.md не создаём —
    // изменение оформляется change'ом OpenSpec средствами самого OpenSpec.
    if let Some(hint) = crate::delta::openspec_only_hint(repo)? {
        return Err(HarnessError::Control(hint));
    }
    let diff = compute_diff(repo, input)?;
    let picked = pick(&diff, numbers)?;
    let delta_name = allocate_delta_name(repo, name, &diff.head)?;
    let body = render_delta_body(&delta_name, &diff, &picked);
    let delta_path = crate::delta::new_with_body(repo, &delta_name, &body)?;
    let mut journal_path = decisions::decisions_path(repo);
    for p in &picked {
        journal_path = decisions::append(
            repo,
            DecisionEntry {
                edge_id: p.edge_id.clone(),
                decision: Decision::Accept,
                reason: String::new(),
                grounds_hash: p.grounds_hash.clone(),
                decided_at: decisions::now_stamp(),
                source,
                delta: Some(delta_name.clone()),
            },
        )?;
    }
    Ok(AcceptReport {
        delta_name,
        delta_path,
        accepted: picked.into_iter().cloned().collect(),
        journal_path,
    })
}

/// `arch-diff reject <n> --reason "…"`: записывает отказ в журнал решений;
/// повторно ребро не предлагается, пока его основания не изменились.
///
/// # Errors
/// Пустая причина; неизвестный номер (в т.ч. по уже решённому ребру);
/// ошибки диффа и журнала.
pub fn reject_proposal(
    repo: &Path,
    input: &DecideInput,
    n: usize,
    reason: &str,
    source: DecisionSource,
) -> Result<RejectReport> {
    if reason.trim().is_empty() {
        return Err(HarnessError::Control(
            "arch-diff reject: причина пуста — журнал решений без причины бессмыслен; \
             укажите --reason \"…\""
                .to_string(),
        ));
    }
    let diff = compute_diff(repo, input)?;
    let picked = pick(&diff, &[n])?;
    let p = picked[0];
    let entry = DecisionEntry {
        edge_id: p.edge_id.clone(),
        decision: Decision::Reject,
        reason: reason.trim().to_string(),
        grounds_hash: p.grounds_hash.clone(),
        decided_at: decisions::now_stamp(),
        source,
        delta: None,
    };
    let journal_path = decisions::append(repo, entry.clone())?;
    Ok(RejectReport {
        entry,
        journal_path,
    })
}

/// Читает журнал решений (для печати в CLI; обёртка над
/// [`decisions::load`], чтобы CLI не знал устройство файла).
///
/// # Errors
/// Как у [`decisions::load`].
pub fn load_journal(repo: &Path) -> Result<DecisionJournal> {
    decisions::load(repo)
}

#[cfg(test)]
mod tests {
    use super::super::diff::tests::{commit_all, fixture_agent_change, fixture_case};
    use super::super::snapshot::tests::{git_in, git_repo, write_file};
    use super::*;
    use crate::arch_diff::{DECISIONS_SCHEMA, Decision};

    /// Вход решений демо-фикстуры (база — тег `base0` до правки агента).
    fn demo_input(globs: &DiffGlobs) -> DecideInput<'_> {
        DecideInput {
            base: "base0",
            head: None,
            limits: (1, 4),
            globs,
        }
    }

    /// Демо-фикстура раздела 9: кейс + тег базы + правка агента (прямая
    /// запись в ядро в обход оркестратора + строка подключения к хранилищу).
    fn demo_repo(dir: &Path) -> PathBuf {
        let repo = dir.join("case");
        std::fs::create_dir_all(&repo).expect("mkdir");
        fixture_case(&repo);
        git_repo(&repo);
        git_in(&repo, &["tag", "base0"]);
        fixture_agent_change(&repo);
        commit_all(&repo, "agent/direct-ledger-write");
        repo
    }

    /// Единственная дельта в changes/ (имя и путь к DELTA.md).
    fn the_only_delta(repo: &Path) -> (String, PathBuf) {
        let deltas = crate::delta::list(repo);
        assert_eq!(deltas.len(), 1, "ожидалась ровно одна дельта: {deltas:?}");
        (deltas[0].name.clone(), deltas[0].path.clone())
    }

    /// Приёмка K5: `accept 1` на демо-фикстуре создаёт дельту с правкой CMP
    /// (`depends_on += …`), ⚠-конфликт с AD зафиксирован в дельте явно, запись
    /// accept ложится в версионированный журнал.
    #[test]
    fn accept_creates_delta_with_cmp_edit_and_journals() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        let report = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1],
            None,
            DecisionSource::Human,
        )
        .expect("accept");

        let (name, path) = the_only_delta(&repo);
        assert_eq!(name, report.delta_name);
        assert!(name.starts_with("arch-diff-"), "{name}");
        let body = std::fs::read_to_string(&path).expect("DELTA.md");
        // Правка CMP дословно: путь файла (по нему guard засчитает покрытие),
        // depends_on += цель, основание файл:строка.
        assert!(body.contains("model/CMP-001-intake.md"), "{body}");
        assert!(body.contains("depends_on += CMP-004"), "{body}");
        assert!(body.contains("skeleton/intake/writer.py:2"), "{body}");
        // ⚠-конфликт зафиксирован явно.
        assert!(body.contains("## Конфликты с инвариантами"), "{body}");
        assert!(body.contains("AD-2"), "{body}");
        // Дельта валидна по правилам `delta validate` (без error-находок).
        let issues = crate::delta::validate(&repo, &name).expect("validate");
        assert!(
            issues.iter().all(|i| i.severity != "error"),
            "дельта обязана быть валидной: {issues:?}"
        );

        // Журнал: версионированная запись accept с именем дельты.
        let raw: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(decisions::decisions_path(&repo)).expect("журнал"),
        )
        .expect("json");
        assert_eq!(raw["schema"], DECISIONS_SCHEMA, "журнал версионирован");
        let journal = decisions::load(&repo).expect("load");
        assert_eq!(journal.entries.len(), 1);
        let e = &journal.entries[0];
        assert_eq!(e.edge_id, "import:CMP-001->CMP-004");
        assert_eq!(e.decision, Decision::Accept);
        assert_eq!(e.delta.as_deref(), Some(name.as_str()));
        assert_eq!(e.source, DecisionSource::Human);
        assert_eq!(e.grounds_hash.len(), 64, "sha256 hex: {e:?}");
        assert_ne!(e.decided_at, "");
    }

    /// Принятое ребро не предлагается повторно (пока основания те же);
    /// номера оставшихся СОХРАНЯЮТСЯ (решённое исчезает, дырки — след
    /// решений), чтобы accept/reject одного вывода работали в одном сеансе;
    /// повторный accept при занятом имени по умолчанию получает суффикс `-2`.
    #[test]
    fn accepted_edge_not_reproposed_and_name_suffix() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1],
            None,
            DecisionSource::Unknown,
        )
        .expect("accept 1");

        // Повторный прогон: ребра CMP-001→CMP-004 среди предложений нет;
        // предложение по хранилищу сохранило свой номер (№2).
        let diff = compute_diff(&repo, &demo_input(&globs)).expect("дифф");
        assert_eq!(diff.proposals.len(), 1, "{:?}", diff.proposals);
        assert_eq!(diff.proposals[0].n, 2, "номер сохранён, а не перенумерован");
        assert!(diff.proposals[0].summary.contains("ledger-db"));

        // Принимаем и его (по сохранённому номеру) — имя по умолчанию занято,
        // получаем суффикс -2.
        let second = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[2],
            None,
            DecisionSource::Agent,
        )
        .expect("accept second");
        assert!(
            second.delta_name.ends_with("-2"),
            "суффикс занятого имени: {}",
            second.delta_name
        );
        let journal = decisions::load(&repo).expect("load");
        assert_eq!(journal.entries.len(), 2, "обе accept-записи в журнале");
        assert_eq!(
            journal.entries[1].delta.as_deref(),
            Some(second.delta_name.as_str())
        );
        // Предложений больше нет.
        let diff = compute_diff(&repo, &demo_input(&globs)).expect("дифф");
        assert!(diff.proposals.is_empty(), "{:?}", diff.proposals);
    }

    /// Приёмка K5: `reject 2 --reason …` убирает предложение из следующего
    /// прогона arch-diff; основания (файл:строка) изменились — ребро
    /// предлагается снова.
    #[test]
    fn reject_suppresses_until_grounds_change() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        let before = compute_diff(&repo, &demo_input(&globs)).expect("дифф");
        assert_eq!(before.proposals.len(), 2);

        let report = reject_proposal(
            &repo,
            &demo_input(&globs),
            2,
            "хранилище осознанно вне модели (legacy-шина)",
            DecisionSource::Agent,
        )
        .expect("reject");
        assert_eq!(report.entry.decision, Decision::Reject);
        assert_eq!(
            report.entry.edge_id,
            "connect:CMP-001->store:postgres://ledger-db:5432"
        );
        assert_eq!(
            report.entry.reason,
            "хранилище осознанно вне модели (legacy-шина)"
        );

        // Следующий прогон: предложения по хранилищу нет.
        let after = compute_diff(&repo, &demo_input(&globs)).expect("дифф");
        assert_eq!(after.proposals.len(), 1, "{:?}", after.proposals);
        assert!(after.proposals[0].summary.contains("depends_on"));

        // Основания изменились (dsn переехал на другую строку) — ребро снова
        // предлагается, несмотря на прошлый reject.
        write_file(
            &repo,
            "skeleton/intake/config.yaml",
            "# конфиг приёма\ndsn: \"postgres://ledger-db:5432/ledger\"\n",
        );
        commit_all(&repo, "move-dsn-line");
        let reproposed = compute_diff(&repo, &demo_input(&globs)).expect("дифф");
        assert!(
            reproposed
                .proposals
                .iter()
                .any(|p| p.summary.contains("ledger-db")),
            "после смены оснований ребро обязано предлагаться снова: {:?}",
            reproposed.proposals
        );
    }

    /// Приёмка K5: правка модели без дельты по-прежнему ловится `delta_guard`;
    /// после `accept` правка покрыта созданной дельтой.
    #[test]
    fn guard_catches_bare_model_edit_but_accepts_delta_covered() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        let model_file = "model/CMP-001-intake.md";
        let original = std::fs::read_to_string(repo.join(model_file)).expect("чтение модели");
        let edited = original.replace("depends_on: [CMP-009]", "depends_on: [CMP-009, CMP-004]");

        // Без дельты правка model/ — нарушение гейта прямых правок спайна.
        write_file(&repo, model_file, &edited);
        let bare = crate::delta::guard(&repo, None, &[]).expect("guard");
        assert!(!bare.passed, "{bare:?}");
        assert!(bare.violations.contains(&model_file.to_string()));

        // Откат правки, accept 1 — и та же правка покрыта дельтой.
        write_file(&repo, model_file, &original);
        accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1],
            None,
            DecisionSource::Human,
        )
        .expect("accept");
        write_file(&repo, model_file, &edited);
        let covered = crate::delta::guard(&repo, None, &[]).expect("guard");
        assert!(covered.passed, "{covered:?}");
        assert!(
            covered
                .covered
                .iter()
                .any(|(f, d)| f == model_file && d.starts_with("delta:arch-diff-")),
            "покрытие дельтой accept: {:?}",
            covered.covered
        );
    }

    /// Правило 9 (ADR-062): при `[delta] sources = ["openspec"]` accept НЕ
    /// пишет DELTA.md, а подсказывает создать change `OpenSpec`; журнал тоже
    /// пуст (решение не состоялось).
    #[test]
    fn openspec_only_sources_refuse_delta_write() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        write_file(
            &repo,
            "arch-harness.toml",
            "[delta]\nsources = [\"openspec\"]\n",
        );
        let globs = DiffGlobs::default();
        let err = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1],
            None,
            DecisionSource::Unknown,
        )
        .expect_err("отказ писать дельту");
        let msg = err.to_string();
        assert!(msg.contains("OpenSpec"), "{msg}");
        assert!(msg.contains("openspec/changes"), "{msg}");
        assert!(!repo.join("changes").exists(), "changes/ не создаётся");
        assert!(
            !decisions::decisions_path(&repo).exists(),
            "журнал не пишется — решения не было"
        );
    }

    /// Ошибки края: пустой список номеров, неизвестный номер, пустая причина
    /// reject, занятое явное имя дельты.
    #[test]
    fn decide_rejects_bad_input_with_russian_errors() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        let err = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[],
            None,
            DecisionSource::Unknown,
        )
        .expect_err("пустой список");
        assert!(err.to_string().contains("укажите номера"), "{err}");

        let err = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[99],
            None,
            DecisionSource::Unknown,
        )
        .expect_err("нет такого номера");
        assert!(err.to_string().contains("№99"), "{err}");

        let err = reject_proposal(&repo, &demo_input(&globs), 1, "  ", DecisionSource::Unknown)
            .expect_err("пустая причина");
        assert!(err.to_string().contains("причина пуста"), "{err}");

        accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1],
            Some("my-delta"),
            DecisionSource::Unknown,
        )
        .expect("accept");
        // Явное имя занято: предложение №2 существует, но дельта не создаётся.
        let err = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[2],
            Some("my-delta"),
            DecisionSource::Unknown,
        )
        .expect_err("имя занято");
        assert!(err.to_string().contains("my-delta"), "{err}");
        // А повторный accept решённого ребра невозможен уже по номеру:
        // предложение №1 решено и из списка убрано.
        let err = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1],
            Some("other-delta"),
            DecisionSource::Unknown,
        )
        .expect_err("решённое не принимается повторно");
        assert!(err.to_string().contains("№1"), "{err}");
    }

    /// Аддитивная эволюция `arch-be/arch-diff/v1` (ADR-063: добавление поля —
    /// минорно): предложения несут `edge_id` и `grounds_hash` — ключ журнала
    /// решений K5 (ADR-064).
    #[test]
    fn proposals_carry_decision_keys_in_json() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        let diff = compute_diff(&repo, &demo_input(&globs)).expect("дифф");
        let json = super::super::report::render_json(&diff).expect("json");
        let v: serde_json::Value = serde_json::from_str(&json).expect("разбор");
        assert_eq!(v["schema"], super::super::types::ARCH_DIFF_SCHEMA);
        let p0 = &v["proposals"][0];
        assert_eq!(p0["edge_id"], "import:CMP-001->CMP-004", "{p0}");
        assert!(
            p0["grounds_hash"].as_str().is_some_and(|h| h.len() == 64),
            "{p0}"
        );
    }

    /// Несколько номеров одним accept — одна дельта, по записи журнала на
    /// каждое ребро; конфликты не блокируют принятие.
    #[test]
    fn accept_multiple_proposals_in_one_delta() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = demo_repo(tmp.path());
        let globs = DiffGlobs::default();
        let report = accept_proposals(
            &repo,
            &demo_input(&globs),
            &[1, 2],
            Some("intake-ledger"),
            DecisionSource::Unknown,
        )
        .expect("accept обоих");
        assert_eq!(report.accepted.len(), 2);
        let body = std::fs::read_to_string(&report.delta_path).expect("DELTA.md");
        assert!(body.contains("model/CMP-001-intake.md"), "{body}");
        assert!(body.contains("model/SYS-001-ledger-db.md"), "{body}");
        assert!(body.contains("accept 1 2"), "{body}");
        let journal = decisions::load(&repo).expect("load");
        assert_eq!(journal.entries.len(), 2);
        assert!(
            journal
                .entries
                .iter()
                .all(|e| e.delta.as_deref() == Some("intake-ledger"))
        );
    }
}

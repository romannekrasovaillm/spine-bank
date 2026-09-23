//! Конверт вердикта (B1, П7/ADR-043): расхождение конверта с текущим
//! состоянием дерева и его механическая сверка (`--verify-envelope`).

use std::collections::BTreeMap;
use std::path::Path;

use super::git::GitProbe;
use super::verdict::collect_inputs;
use crate::control;
use crate::error::{HarnessError, Result};

/// Расхождение между вердиктом из конверта и текущим состоянием дерева.
#[derive(Debug, Clone)]
pub struct EnvelopeDrift {
    /// Входы, изменившиеся с момента вердикта: `(вход, было, стало)`.
    pub changed: Vec<(String, String, String)>,
    /// Входы, которые были в вердикте, но пропали с дерева.
    pub missing: Vec<String>,
    /// Вердикт относится к текущему состоянию.
    pub same: bool,
}

/// Сверяет конверт вердикта с текущим состоянием репозитория: аттестация
/// привязывает зелёный к ВХОДАМ (Н3, ADR-043), поэтому «воспроизводим по
/// конверту» — это механическая проверка, а не обещание.
///
/// Сравниваются входы (реестр правил, спайн, модель, бандлы, `ROUTE.lock`,
/// коммит базы). Состав находок не пересчитывается: для этого нужен прогон
/// `arch-be gate` с теми же флагами.
///
/// # Errors
/// Файл конверта не читается, не JSON или не конверт `gate-verdict`.
pub fn verify_envelope(
    repo: &Path,
    envelope_path: &Path,
    limits: (usize, usize),
) -> Result<EnvelopeDrift> {
    let text =
        std::fs::read_to_string(envelope_path).map_err(|e| HarnessError::io(envelope_path, e))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| HarnessError::Config(format!("{}: не JSON ({e})", envelope_path.display())))?;
    let declared = value
        .get("inputs")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            HarnessError::Config(format!(
                "{}: нет секции `inputs` — это не конверт arch-be/gate-verdict",
                envelope_path.display()
            ))
        })?;
    // Базу диффа берём из самого конверта, если она там сохранена; иначе —
    // HEAD текущего дерева (конверт и дерево в одном репозитории).
    let base_ref: Option<String> = match value
        .get("inputs")
        .and_then(|i| i.get("base"))
        .and_then(|b| b.as_str())
    {
        Some(sha) if sha != "absent" => Some(sha.to_string()),
        _ => None,
    };
    let current = collect_inputs_for_verify(repo, base_ref.as_deref(), limits);
    let mut changed = Vec::new();
    let mut missing = Vec::new();
    for (name, was) in declared {
        let was = was.as_str().unwrap_or_default().to_string();
        match current.get(name) {
            Some(now) if *now == was => {}
            Some(now) => {
                if was == "absent" {
                    missing.push(name.clone());
                } else {
                    changed.push((name.clone(), was, now.clone()));
                }
            }
            None => missing.push(name.clone()),
        }
    }
    Ok(EnvelopeDrift {
        same: changed.is_empty() && missing.is_empty(),
        changed,
        missing,
    })
}

/// Входы текущего дерева для сверки с конвертом: те же, что у прогона, с тем
/// же резолвом `CONSTRAINTS.yaml` (единый резолвер E2).
fn collect_inputs_for_verify(
    repo: &Path,
    base: Option<&str>,
    _limits: (usize, usize),
) -> BTreeMap<String, String> {
    let constraints = control::resolve_constraints_path_detailed(repo, None)
        .map_or_else(|| repo.join(control::HANDOFF_CONSTRAINTS_PATH), |r| r.path);
    let git = GitProbe::probe(repo);
    collect_inputs(repo, &constraints, base, &git)
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::Route;
    use crate::gate::testkit::*;
    use crate::gate::{GateRequirements, GateStatus, run, run_with};
    use std::path::PathBuf;

    // --- Н3: аттестация различает состояния репозитория (ADR-043) ----------

    /// Два РАЗНЫХ FAIL одной составляющей дают разные аттестации.
    #[test]
    fn attestation_differs_for_different_findings() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        let limits = (1, 4);
        // Два разных нарушения одного и того же правила `no_pan`.
        std::fs::create_dir_all(dir.join("a")).expect("mkdir");
        std::fs::write(dir.join("a/one.py"), "PAN").expect("py");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "a"]);
        let first = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert!(!first.passed, "{:?}", first.not_checked);
        std::fs::create_dir_all(dir.join("b")).expect("mkdir");
        std::fs::write(dir.join("b/two.py"), "PAN").expect("py");
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "b"]);
        let second = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert!(!second.passed);
        // Оба — FAIL составляющей fitness, но находки разные.
        assert_eq!(status_of(&first, "fitness"), GateStatus::Fail);
        assert_eq!(status_of(&second, "fitness"), GateStatus::Fail);
        assert_ne!(
            first.attestation, second.attestation,
            "разные дефекты обязаны давать разные аттестации"
        );
        let d1 = first.envelope_json()["components"]
            .as_array()
            .expect("components")
            .iter()
            .find(|c| c["name"] == "fitness")
            .expect("fitness")["findings_digest"]
            .clone();
        let d2 = second.envelope_json()["components"]
            .as_array()
            .expect("components")
            .iter()
            .find(|c| c["name"] == "fitness")
            .expect("fitness")["findings_digest"]
            .clone();
        assert_ne!(d1, d2, "свёртки находок обязаны различаться");
    }

    /// Правка модели меняет аттестацию — раньше входом был только реестр.
    #[test]
    fn attestation_changes_when_model_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        write_model(dir, &[("CMP-001", "depends_on: []")]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "model"]);
        let limits = (1, 4);
        let before = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        // Безвредная правка ТЕКСТА сущности: вердикт тот же, аттестация — нет.
        // Правка коммитится, иначе её поймает delta_guard и сменит не вход, а
        // статус составляющей — сравнение было бы не о том.
        {
            use std::io::Write as _;
            let f = std::fs::OpenOptions::new()
                .append(true)
                .open(dir.join("model/CMP-001.md"))
                .expect("open");
            let mut f = f;
            f.write_all("\n\nУточнение формулировки без смены решения.\n".as_bytes())
                .expect("append");
        }
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "текстовая правка"]);
        let after = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(before.outcome, after.outcome);
        assert_ne!(
            before.attestation, after.attestation,
            "аттестация обязана следовать за состоянием model/"
        );
    }

    /// Аттестация не зависит от того, как записан путь и где лежит репозиторий.
    #[test]
    fn attestation_stable_across_path_spelling() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        let limits = (1, 4);
        let abs = run_with(
            &dir.canonicalize().expect("canonicalize"),
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        let trailing = PathBuf::from(format!("{}/", dir.display()));
        let rel = run_with(
            &trailing,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        assert_eq!(abs.attestation, rel.attestation);
        assert_eq!(abs.inputs_map(), rel.inputs_map());
    }

    /// `--verify-envelope`: тот же вход — «относится», правка — «изменилось».
    #[test]
    fn verify_envelope_detects_drift() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        make_gate_repo(dir);
        write_model(dir, &[("CMP-001", "depends_on: []")]);
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "model"]);
        let limits = (1, 4);
        let report = run_with(
            dir,
            Some(Route::Fast),
            None,
            None,
            limits,
            &GateRequirements::default(),
        )
        .expect("gate");
        let envelope = dir.join("verdict.json");
        std::fs::write(
            &envelope,
            serde_json::to_string_pretty(&report.envelope_json()).expect("json"),
        )
        .expect("write");
        let same = verify_envelope(dir, &envelope, limits).expect("verify");
        assert!(same.same, "{:?} {:?}", same.changed, same.missing);
        // Правка спайна — состояние разошлось, вход назван.
        std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n\nAD-1 …\n").expect("spine");
        let drifted = verify_envelope(dir, &envelope, limits).expect("verify");
        assert!(!drifted.same);
        assert!(
            drifted.changed.iter().any(|(name, _, _)| name == "spine"),
            "{:?}",
            drifted.changed
        );
        // Не конверт — честная ошибка оператора, а не «всё совпало».
        std::fs::write(&envelope, "{\"hello\":1}").expect("write");
        assert!(verify_envelope(dir, &envelope, limits).is_err());
    }
    /// П7: конверт вердикта стабилен (идемпотентность) и честно называет
    /// непроверенное; exit-код INCOMPLETE — 3.
    #[test]
    fn envelope_attestation_is_stable_and_lists_not_checked() {
        let tmp = tempfile::tempdir().expect("tmp");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        make_gate_repo(&repo);
        let first = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        let second = run(&repo, Some(Route::Standard), None, None, (1, 4)).expect("гейт");
        let a = first.envelope_json();
        let b = second.envelope_json();
        assert_eq!(
            a["attestation"], b["attestation"],
            "тот же вход — та же аттестация"
        );
        assert_eq!(a["verdict"], "INCOMPLETE");
        assert_eq!(a["exit_code"], 3);
        let not_checked = a["not_checked"].as_array().expect("not_checked");
        assert!(
            not_checked.iter().any(|v| v == "trace_check" || v == "nfr"),
            "{a}"
        );
    }
}

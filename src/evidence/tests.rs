//! Тесты модуля `evidence` (вынесены из `evidence.rs`: лимит модуля 3000
//! строк, правило `prod_file_length_limit`; прецеденты — `gate/testkit.rs`,
//! `contract_diff/testkit.rs`).

use super::*;

fn put(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    std::fs::write(p, content).expect("write");
}

#[test]
fn fast_route_packs_minimal_bundle() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", "# Проблема\n");
    put(
        dir,
        "SPEC.md",
        "## Проблема\n## Критерии приёмки\n## Риски\n",
    );
    put(dir, "RISK.md", "Fast: 0 триггеров\n");
    put(dir, "ROLLBACK.md", "git revert\n");
    let (bundle, verdict) = pack(dir, Route::Fast).expect("pack");
    assert!(verdict.passed, "missing: {:?}", verdict.missing);
    assert!(bundle.items.len() >= 5, "items: {}", bundle.items.len());
    assert!(dir.join("EVIDENCE.yaml").is_file());
    // Проверка чиста сразу после упаковки.
    let v = verify(dir).expect("verify");
    assert!(v.passed, "{:?} {:?}", v.missing, v.tampered);
}

#[test]
fn critical_route_requires_a3_and_spine() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", "x");
    let (_bundle, verdict) = pack(dir, Route::Critical).expect("pack");
    assert!(!verdict.passed);
    assert!(verdict.missing.contains(&"decision_a3".to_string()));
    assert!(verdict.missing.contains(&"spine".to_string()));
    assert!(verdict.missing.contains(&"adversarial_review".to_string()));
    // Critical требует и evidence репетиции отката (гейт A4).
    assert!(verdict.missing.contains(&"rollback_rehearsal".to_string()));
}

#[test]
fn verify_detects_tampering_after_pack() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", "исходная проблема");
    put(dir, "SPEC.md", "спека");
    put(dir, "RISK.md", "r");
    put(dir, "ROLLBACK.md", "rb");
    pack(dir, Route::Fast).expect("pack");
    // Подмена артефакта после упаковки.
    put(dir, "SPEC.md", "ТИХО ПЕРЕПИСАЛИ");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    assert!(
        v.tampered.iter().any(|t| t.contains("spec_or_delta")),
        "{:?}",
        v.tampered
    );
}

// --- П2: канонический вход, рекурсия, SHA-256, миграция формата --------

/// Вердикт не зависит от написания пути, каталог хэшируется рекурсивно,
/// хэши — SHA-256.
#[test]
fn hash_is_path_invariant_and_recursive() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", "p");
    put(dir, "SPEC.md", "s\n## Критерии приёмки\n");
    put(dir, "RISK.md", "r");
    put(dir, "ROLLBACK.md", "rb");
    put(dir, "docs/adr/ADR-001.md", "a1");
    put(dir, "docs/adr/archive/ADR-002.md", "a2");
    put(dir, "VALIDATION.md", "v");
    put(dir, "reports/fitness.md", "f");
    let (bundle, verdict) = pack(dir, Route::Standard).expect("pack");
    assert!(verdict.passed, "missing: {:?}", verdict.missing);
    assert_eq!(bundle.hash_alg, HASH_ALG_SHA256);
    assert!(
        bundle.items.iter().all(|i| i.hash.len() == 64),
        "хэши обязаны быть SHA-256 hex: {:?}",
        bundle
            .items
            .iter()
            .map(|i| i.hash.len())
            .collect::<Vec<_>>()
    );
    // Инвариантность к написанию пути (Д2): «.», абсолютный, с «/».
    let trailing = PathBuf::from(format!("{}/", dir.display()));
    let abs = dir.canonicalize().expect("canonicalize");
    let v1 = verify(dir).expect("verify");
    let v2 = verify(&trailing).expect("verify trailing");
    let v3 = verify(&abs).expect("verify abs");
    assert!(v1.passed && v2.passed && v3.passed, "{v1:?} {v2:?} {v3:?}");
    assert_eq!(v1.tampered, v2.tampered);
    assert_eq!(v1.tampered, v3.tampered);
    // Рекурсия: правка во вложенном подкаталоге каталога-артефакта видна.
    put(dir, "docs/adr/archive/ADR-002.md", "a2 ИЗМЕНЁН");
    let v4 = verify(&abs).expect("verify");
    assert!(!v4.passed);
    assert!(
        v4.tampered.iter().any(|t| t.contains("adr_or_pattern")),
        "{:?}",
        v4.tampered
    );
}

/// Бандл старого формата (без `hash_alg`) проверяется старой свёрткой
/// и получает предупреждение о переупаковке.
#[test]
fn legacy_bundle_verified_with_old_alg_and_warned() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", "p");
    put(dir, "SPEC.md", "s");
    put(dir, "RISK.md", "r");
    put(dir, "ROLLBACK.md", "rb");
    let (mut bundle, _v) = pack(dir, Route::Fast).expect("pack");
    bundle.hash_alg = HASH_ALG_LEGACY.to_string();
    for item in &mut bundle.items {
        let p = dir.join(&item.path);
        let (h, s) = hash_artifact_legacy(&p).expect("legacy hash");
        item.hash = h;
        item.size = s;
    }
    // Эмулируем старый манифест: поля hash_alg в нём не было.
    let text = serde_yaml_ng::to_string(&bundle).expect("yaml");
    let text = text
        .lines()
        .filter(|l| !l.starts_with("hash_alg:"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("EVIDENCE.yaml"), text).expect("write");
    let v = verify(dir).expect("verify");
    assert!(
        v.passed,
        "missing: {:?}, tampered: {:?}",
        v.missing, v.tampered
    );
    assert!(
        v.warnings.iter().any(|w| w.contains("старого формата")),
        "{:?}",
        v.warnings
    );
}

// --- инструменты evidence_pack / evidence_verify ------------------------

/// Тестовый контекст без LLM.
fn tool_ctx(dir: &Path) -> ToolContext {
    ToolContext::new(
        dir.to_path_buf(),
        Arc::new(crate::config::Config::default()),
    )
}

#[tokio::test]
async fn evidence_pack_then_verify_tools_roundtrip_passes() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    // Содержательные артефакты: с 0.3.4 пустышка — находка (Н1, ADR-041),
    // и «чисто сразу после упаковки» проверяется на написанном бандле.
    put(dir, "PROBLEM.md", &body("Проблема"));
    put(dir, "SPEC.md", &body("Спецификация"));
    put(dir, "RISK.md", &body("Риск"));
    put(dir, "ROLLBACK.md", &body("Откат"));
    let ctx = tool_ctx(dir);
    // pack (fast) → manifest записан, полнота ок.
    let out = EvidencePackTool
        .call(json!({"change_dir": ".", "route": "fast"}), &ctx)
        .await
        .expect("вызов");
    assert!(!out.is_error, "{}", out.content);
    let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
    assert_eq!(v["passed"], true, "{v}");
    assert!(dir.join("EVIDENCE.yaml").is_file());
    // verify сразу после pack — чист.
    let out = EvidenceVerifyTool
        .call(json!({"change_dir": "."}), &ctx)
        .await
        .expect("вызов");
    let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
    assert_eq!(v["passed"], true, "{v}");
    assert_eq!(v["issues"], json!([]));

    // Подмена артефакта → verify passed=false, находка kind=tampered.
    put(dir, "SPEC.md", "ПЕРЕПИСАНО");
    let out = EvidenceVerifyTool
        .call(json!({"change_dir": "."}), &ctx)
        .await
        .expect("вызов");
    let v: Value = serde_json::from_str(&out.content).expect("JSON-вердикт");
    assert_eq!(v["passed"], false, "{v}");
    assert!(
        v["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .any(|i| i["kind"] == "tampered"),
        "{v}"
    );
}

// --- Н1: семантика артефакта («есть» ≠ «написан», ADR-041) -------------

/// Содержательное наполнение: длиннее порога 200 б и без маркеров-заглушек.
fn body(title: &str) -> String {
    format!(
        "# {title}\n\n{}\n",
        "Содержательный раздел решения с обоснованием, альтернативами и \
         последствиями. "
            .repeat(4)
    )
}

/// Полный и СОДЕРЖАТЕЛЬНЫЙ бандл маршрута Critical.
fn put_complete_critical(dir: &Path) {
    put(dir, "PROBLEM.md", &body("Проблема"));
    put(dir, "SPEC.md", &body("Спецификация"));
    put(dir, "RISK.md", &body("Риск"));
    put(dir, "ROLLBACK.md", &body("Откат"));
    put(dir, "docs/adr/ADR-001.md", &body("Решение"));
    put(dir, "ARCHITECTURE-SPINE.md", &body("Инварианты"));
    put(
        dir,
        "DECISION.md",
        &format!(
            "# Решение A3\n\n- **choice**: {}\n- **rationale**: {}\n- \
             **rejected**: {}\n- **expiry**: 2099-12-31\n- **decided_by**: Архитектор ДКА\n",
            body("вариант"),
            body("обоснование"),
            body("отвергнутое")
        ),
    );
    put(
        dir,
        "WALKING-SKELETON.md",
        &format!(
            "# Walking skeleton\n\n{}\n\nИтог: PASS (8 из 8)\n",
            body("Сквозной прогон")
        ),
    );
    put(
        dir,
        "docs/REVIEW.md",
        &format!(
            "# Ревью\n\nВердикт.\n\nVERDICT: READY\n\nВопросы разобраны: {}",
            body("итог")
        ),
    );
    // Д8: «пройдено» обязано опираться на шаги. Отчёт без шагов (каким его
    // писала заготовка `bootstrap` до 0.3.5) — не аттестация, и держать его
    // в фикстуре «полного бандла» значило бы требовать от гейта слепоты.
    put(
        dir,
        ".arch-handoff/REHEARSAL.json",
        r#"{"kind":"rollback_rehearsal","gate":"A4","passed":true,
                "baseline_commit":"abc123","rehearsed_at":"2026-09-19T10:00:00Z",
                "duration_secs":1.5,
                "steps":[{"name":"якорь-доступен","status":"pass","exit_code":0,
                          "detail":"commit"}],
                "verify":null,
                "log":["репетиция отката прошла"]}"#,
    );
    put(
        dir,
        ".arch-handoff/ROLLBACK.yaml",
        "baseline_commit: abc123\nsteps:\n  - name: revert\n    run: git revert --no-edit HEAD\n",
    );
    put(
        dir,
        "VALIDATION.md",
        &format!("# Валидация\n\n{}\n\nИтог: PASS\n", body("Тесты")),
    );
    put(
        dir,
        "reports/fitness.md",
        &format!("# Fitness\n\n{}\n\nИтог: PASS\n", body("Правила")),
    );
}

/// Абсолютный регресс 0.3.3: бандл из заглушек проходил как «выпуск разрешён».
/// A1 (0.3.14): содержательный бандл зелёный, но рукописные отчёты прогонов
/// по умолчанию названы предупреждением `evidence_report_unbound` — проза не
/// доказывает прогон (error — только по флагу `require_records`).
#[test]
fn verify_passes_on_complete_bundle() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    let (_b, v) = pack(dir, Route::Critical).expect("pack");
    assert!(v.passed, "missing: {:?}", v.missing);
    let v = verify(dir).expect("verify");
    assert!(
        v.passed,
        "содержательный бандл обязан быть зелёным; находки: {:?}",
        v.semantics
    );
    assert!(
        v.semantics
            .iter()
            .all(|f| f.rule == "evidence_report_unbound" && f.severity == "warn"),
        "кроме предупреждений о рукописных отчётах находок нет: {:?}",
        v.semantics
    );
    // Подлинность подписи A3 — заявленное, но механикой не проверяется.
    assert!(
        v.not_verified
            .iter()
            .any(|n| n.contains("подпись A3: заявлена")),
        "{:?}",
        v.not_verified
    );
}

/// Заглушка вместо артефакта: файл есть, содержания нет.
#[test]
fn verify_flags_stub_artifact() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(dir, "DECISION.md", "TODO");
    let (_b, _v) = pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed, "заглушка на Critical обязана блокировать выпуск");
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "evidence_stub" && f.key == "decision_a3" && f.severity == "error"),
        "{:?}",
        v.semantics
    );
    assert!(!v.blocking_semantics().is_empty());
}

/// Ревью с вердиктом NOT-READY больше не даёт «выпуск разрешён».
#[test]
fn verify_blocks_not_ready_review() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(
        dir,
        "docs/REVIEW.md",
        &format!("# R\n\nVERDICT: NOT-READY\n\n{}", body("замечания")),
    );
    let (_b, _v) = pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    assert!(
        v.semantics.iter().any(|f| f.rule == "review_not_ready"),
        "{:?}",
        v.semantics
    );
    // Отсутствие строки вердикта — отдельная находка.
    put(dir, "docs/REVIEW.md", &body("ревью без вердикта"));
    pack(dir, Route::Critical).expect("repack");
    let v = verify(dir).expect("verify");
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "review_verdict_missing"),
        "{:?}",
        v.semantics
    );
}

/// Неподписанная запись A3 (пустой `decided_by`) — находка `a3_not_signed`.
#[test]
fn verify_flags_unsigned_a3() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(
        dir,
        "DECISION.md",
        "# Решение A3\n\n- **choice**: вариант А — централизованный клиринг\n- **rationale**: снижает операционный риск расчётов и снимает зависимость от ручных сверок\n- **rejected**: вариант Б — распределённый клиринг, отклонён из-за сложности сопровождения\n- **expiry**: 2099-12-31\n- **decided_by**: \n",
    );
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "a3_not_signed" && f.message.contains("decided_by")),
        "{:?}",
        v.semantics
    );
    // Прочерк — тот же случай, что пустое поле.
    put(
        dir,
        "DECISION.md",
        "# Решение A3\n\n- **choice**: вариант А — централизованный клиринг\n- **rationale**: снижает операционный риск расчётов и снимает зависимость от ручных сверок\n- **rejected**: вариант Б — распределённый клиринг, отклонён из-за сложности сопровождения\n- **expiry**: 2099-12-31\n- **decided_by**: —\n",
    );
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(v.semantics.iter().any(|f| f.rule == "a3_not_signed"));
    assert!(!v.passed);
}

/// Просроченный срок пересмотра решения — находка `a3_expired`.
#[test]
fn verify_flags_expired_a3() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(
        dir,
        "DECISION.md",
        "# Решение A3\n\n- **choice**: вариант А — централизованный клиринг\n- **rationale**: снижает операционный риск расчётов и снимает зависимость от ручных сверок\n- **rejected**: вариант Б — распределённый клиринг, отклонён из-за сложности сопровождения\n- **expiry**: 2020-01-01\n- **decided_by**: Архитектор\n",
    );
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    assert!(
        v.semantics.iter().any(|f| f.rule == "a3_expired"),
        "{:?}",
        v.semantics
    );
}

/// `semantics = off` возвращает поведение 0.3.3: проверяется только
/// наличие и целостность, о выключенных проверках сказано честно.
#[test]
fn semantics_off_restores_033_behaviour() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(dir, "DECISION.md", "TODO");
    pack(dir, Route::Critical).expect("pack");
    let cfg = crate::config::EvidenceConfig {
        semantics: crate::config::EvidenceSemantics::Off,
        ..crate::config::EvidenceConfig::default()
    };
    let v = verify_with(dir, &cfg).expect("verify");
    assert!(
        v.passed,
        "0.3.3 не смотрела на содержание: {:?}",
        v.semantics
    );
    assert!(v.semantics.is_empty());
    assert!(
        v.not_verified.iter().any(|n| n.contains("выключены")),
        "{:?}",
        v.not_verified
    );
    // На маршруте Standard та же заглушка — warn, выпуск не блокируется.
    let std_dir = tmp.path().join("standard");
    std::fs::create_dir_all(&std_dir).expect("mkdir");
    put(&std_dir, "PROBLEM.md", &body("Проблема"));
    put(&std_dir, "SPEC.md", &body("Спека"));
    put(&std_dir, "RISK.md", &body("Риск"));
    put(&std_dir, "ROLLBACK.md", &body("Откат"));
    put(&std_dir, "VALIDATION.md", "TODO TODO TODO");
    put(&std_dir, "reports/fitness.md", &body("Fitness"));
    put(&std_dir, "docs/adr/ADR-001.md", &body("ADR"));
    pack(&std_dir, Route::Standard).expect("pack");
    let v = verify(&std_dir).expect("verify");
    assert!(v.passed, "Standard — warn, не блокирует: {:?}", v.semantics);
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "evidence_stub" && f.severity == "warn"),
        "{:?}",
        v.semantics
    );
}

/// Отчёт с провалом внутри — `evidence_reports_fail`.
#[test]
fn verify_flags_failing_report() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(
        dir,
        "VALIDATION.md",
        &format!("{}\n\nИтог: FAIL\n", body("Прогон")),
    );
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "evidence_reports_fail"),
        "{:?}",
        v.semantics
    );
}

/// Baseline репетиции разошёлся с планом отката — evidence обесценено.
#[test]
fn verify_flags_stale_rehearsal_baseline() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(
        dir,
        ".arch-handoff/ROLLBACK.yaml",
        "baseline_commit: deadbeef\nsteps: []\n",
    );
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "rehearsal_stale_baseline"),
        "{:?}",
        v.semantics
    );
}

/// git-команда в песочнице теста; identity задаётся явно — в окружении CI
/// её может не быть.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=test@example.com", "-c", "user.name=test"])
        .args(args)
        .output()
        .expect("git запускается");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Д8: отчёт, объявленный пройденным, но без единого шага, — не
/// репетиция. На Critical это блокирующая находка: пустой список шагов
/// откат не подтверждает, а «passed: true» утверждает обратное.
#[test]
fn verify_flags_empty_rehearsal() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    put(
        dir,
        ".arch-handoff/REHEARSAL.json",
        r#"{"kind":"rollback_rehearsal","gate":"A4","passed":true,
                "baseline_commit":"abc123","rehearsed_at":"2026-09-19T10:00:00Z",
                "duration_secs":1.5,"steps":[],"verify":null,
                "log":["репетиция отката прошла"]}"#,
    );
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed, "заготовка не имеет права давать зелёный");
    let found = v
        .semantics
        .iter()
        .find(|f| f.rule == "rehearsal_empty")
        .unwrap_or_else(|| panic!("находка rehearsal_empty: {:?}", v.semantics));
    assert_eq!(found.severity, "error", "Critical — блокирует выпуск");
    assert!(!found.fix_hint.is_empty(), "у находки есть подсказка");
}

/// Д8: якорь отката, не резолвящийся в коммит ЭТОГО репозитория,
/// называется явно — но не блокирует: пакет мог быть собран в другой
/// истории (так живут перенесённые кейсы `кейсы/*`), и обвинять их в
/// подлоге нечем.
#[test]
fn verify_flags_unresolved_baseline() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    // Репозиторий есть, коммита `abc123` в нём нет.
    git(dir, &["init", "-q"]);
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    let found = v
        .semantics
        .iter()
        .find(|f| f.rule == "rehearsal_baseline_unresolved")
        .unwrap_or_else(|| panic!("находка о нерезолвящемся якоре: {:?}", v.semantics));
    assert_eq!(found.severity, "warn", "не блокирует: другой репозиторий");
    assert!(found.message.contains("abc123"), "{}", found.message);
    assert!(v.passed, "warn выпуск не блокирует: {:?}", v.semantics);
}

/// Обратная сторона Д8: честная репетиция (шаги записаны, якорь
/// резолвится) проходит без единого замечания о репетиции — усиление не
/// наказывает того, кто откат действительно отрепетировал.
#[test]
fn real_rehearsal_still_passes() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    git(dir, &["init", "-q"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "каркас кейса"]);
    let head = git(dir, &["rev-parse", "HEAD"]);
    put(
        dir,
        ".arch-handoff/ROLLBACK.yaml",
        &format!(
            "baseline_commit: {head}\nsteps:\n  - name: якорь-доступен\n    \
             run: \"true\"\nverify: \"true\"\n"
        ),
    );
    let report = crate::rehearsal::rehearse(dir, &dir.join(".arch-handoff")).expect("репетиция");
    assert!(report.passed, "{:?}", report.log);
    assert!(!report.steps.is_empty(), "шаги обязаны попасть в отчёт");
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(
        v.semantics.iter().all(|f| !f.rule.starts_with("rehearsal")),
        "честная репетиция не даёт находок о себе: {:?}",
        v.semantics
    );
    assert!(v.passed, "{:?}", v.semantics);
}

#[tokio::test]
async fn evidence_tools_soft_errors_on_missing_input() {
    let tmp = tempfile::tempdir().expect("tmp");
    let ctx = tool_ctx(tmp.path());
    // Без манифеста — мягкая ошибка verify.
    let out = EvidenceVerifyTool
        .call(json!({"change_dir": "."}), &ctx)
        .await
        .expect("вызов");
    assert!(out.is_error, "{}", out.content);
    // Неизвестный маршрут pack — мягкая ошибка, файл не создан.
    let out = EvidencePackTool
        .call(json!({"change_dir": ".", "route": "ludicrous"}), &ctx)
        .await
        .expect("вызов");
    assert!(out.is_error, "{}", out.content);
    assert!(!tmp.path().join("EVIDENCE.yaml").exists());
}
// --- прямые проверки хэшей, служебных файлов и строк отчёта --------------

/// FNV-1a по эталонным векторам: смещение и простое для каждого байта.
#[test]
fn fnv1a64_matches_reference_vectors() {
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_ne!(fnv1a64(b"abc"), fnv1a64(b"abd"));
}

/// Дефолт алгоритма для бандлов старого формата — именно legacy: иначе
/// бандл без поля `hash_alg` не получил бы предупреждения о переупаковке.
#[test]
fn default_hash_alg_is_legacy() {
    assert_eq!(default_hash_alg(), HASH_ALG_LEGACY);
}

/// Временные и служебные файлы: бэкапы, `.DS_Store` и редакторские
/// черновики — по расширению, без учёта регистра.
#[test]
fn is_transient_recognizes_each_form() {
    for name in ["notes.md~", ".DS_Store", "draft.TMP", "x.swp", "y.SWO"] {
        assert!(is_transient(name), "{name} — служебный");
    }
    for name in ["SPEC.md", "NOTES.txt", "data.json"] {
        assert!(!is_transient(name), "{name} — не служебный");
    }
}

/// Обход каталога: только файлы, рекурсивно, без `.git` и служебных.
#[test]
fn dir_files_returns_only_real_files_recursively() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    put(root, "a.md", "a");
    put(root, "b.md~", "b");
    put(root, "nested/c.md", "c");
    put(root, ".git/config", "git");
    let names: Vec<String> = dir_files(root)
        .iter()
        .map(|p| {
            p.strip_prefix(root)
                .expect("внутри корня")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(names, vec!["a.md", "nested/c.md"], "{names:?}");
}

/// Хэш каталога считает суммарный размер всех файлов (а не только
/// последнего) и даёт sha256-дайджест.
#[test]
fn hash_artifact_sums_sizes_and_digests() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    put(root, "one.md", "12345");
    put(root, "two.md", "123");
    let (digest, size) = hash_artifact(root, HASH_ALG_SHA256).expect("хэш каталога");
    assert_eq!(size, 8);
    assert_eq!(digest.len(), 64, "{digest}");
    // Тот же каталог, переданный иначе («.», с завершающим слэшем) — тот же хэш.
    let (digest_dot, _) = hash_artifact(&root.join("."), HASH_ALG_SHA256).expect("хэш через точку");
    assert_eq!(digest, digest_dot, "путь-аргумент на вердикт не влияет");
}

/// Legacy-свёртка каталога: сверка с формулой по каждому файлу и
/// накопление размера.
#[test]
fn hash_artifact_legacy_matches_formula() {
    use std::fmt::Write as _;
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    put(root, "a.md", "aa");
    put(root, "b.md", "bbb");
    let (digest, size) = hash_artifact_legacy(root).expect("legacy");
    assert_eq!(size, 5);
    let mut acc = String::new();
    for name in ["a.md", "b.md"] {
        let path = root.join(name);
        let bytes = std::fs::read(&path).expect("read");
        let _ = write!(acc, "{}:{:016x};", path.display(), fnv1a64(&bytes));
    }
    assert_eq!(digest, format!("{:016x}", fnv1a64(acc.as_bytes())));
}

/// Хэш одного файла legacy-алгоритмом — свёртка содержимого и его размер.
#[test]
fn hash_artifact_legacy_file_matches_content_hash() {
    let tmp = tempfile::tempdir().expect("tmp");
    let path = tmp.path().join("one.md");
    std::fs::write(&path, "content").expect("write");
    let (digest, size) = hash_artifact_legacy(&path).expect("legacy");
    assert_eq!(size, 7);
    assert_eq!(digest, format!("{:016x}", fnv1a64(b"content")));
}

/// `stub_file_of` называет проблемный файл: меньше порога — пустышка,
/// ровно порог с осмысленным текстом — нет; маркер-заглушка ловится и в
/// большом файле.
#[test]
fn stub_file_of_follows_threshold_strictly() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    let min = 10_u64;
    put(root, "exact.md", &"x".repeat(min as usize));
    assert!(
        stub_file_of(root, min).is_none(),
        "ровно порог — не пустышка"
    );
    std::fs::write(root.join("exact.md"), "x".repeat(min as usize - 1)).expect("write");
    assert!(
        stub_file_of(root, min).is_some(),
        "меньше порога — пустышка"
    );
    std::fs::write(
        root.join("exact.md"),
        format!(
            "{} TODO {}",
            "y".repeat(min as usize),
            "z".repeat(min as usize)
        ),
    )
    .expect("write");
    assert!(
        stub_file_of(root, min).is_some(),
        "маркер-заглушка видна и в большом файле"
    );
    // Файл — не каталог: адресного поиска нет.
    let file = root.join("exact.md");
    assert!(stub_file_of(&file, min).is_none());
}

/// Пустые значения и маркеры-заглушки поля A3.
#[test]
fn field_is_empty_recognizes_empty_and_placeholders() {
    for value in ["", "   ", "—", "–", "-", "?", "TBD", "TODO", "нет", "n/a"] {
        assert!(field_is_empty(value), "«{value}» — пустое");
    }
    assert!(!field_is_empty("Вариант Б: свой шлюз"));
}

/// Строка провала — любая из трёх форм, включая английскую.
#[test]
fn has_fail_line_recognizes_each_form() {
    assert!(has_fail_line("Итог: FAIL"));
    assert!(has_fail_line("fail"));
    assert!(has_fail_line("FAIL — есть дефекты"));
    assert!(!has_fail_line("PASS (3 из 5)"));
}

/// A1: строка «Итог: PASS» в прозе больше не удостоверяет прогон — у
/// артефактов прогонов её не требуют и ей не верят: прогон доказывает
/// машинная запись (`evidence_report_unbound`/`evidence_record_*`).
#[test]
fn handwritten_result_line_is_not_evidence_anymore() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    let report = dir.join("VALIDATION.md");
    std::fs::write(
        &report,
        format!("# Отчёт\n\n{}\n\nИтог: PASS (8 из 8)\n", body("Прогон")),
    )
    .expect("write");
    let (findings, _notes) = semantic_check(dir, "validation", &report, &cfg, "error");
    assert!(
        findings.is_empty(),
        "проза-проверка не краснит написанный отчёт — её роль теперь у записи: {findings:?}"
    );
    // А проверка записи (без markdown-строки итога) называет отсутствие
    // машинного подтверждения — см. verify_flags_unbound_handwritten_report.
}

/// Прогресс бандла считается по манифесту: сколько обязательных ключей
/// маршрута уже описано.
#[test]
fn bundle_progress_counts_manifest_keys() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", "проблема");
    let (_bundle, _v) = pack(dir, Route::Fast).expect("pack");
    let required = required_artifacts(Route::Fast);
    // Ожидание считается по тому же манифесту: прогресс — сколько
    // обязательных ключей маршрута в нём описано.
    let text = std::fs::read_to_string(dir.join("EVIDENCE.yaml")).expect("манифест");
    let bundle: EvidenceBundle = serde_yaml_ng::from_str(&text).expect("yaml");
    let expected = required
        .iter()
        .filter(|(key, _)| bundle.items.iter().any(|i| &i.key == key))
        .count();
    assert!(
        expected > 0 && expected < required.len(),
        "фикстура неполная: {expected} из {}",
        required.len()
    );
    assert_eq!(
        bundle_progress(dir, Route::Fast),
        Some(expected),
        "прогресс по манифесту: {} обязательных ключей",
        required.len()
    );
    // Каталог без манифеста — прогресса нет.
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).expect("mkdir");
    assert!(bundle_progress(&empty, Route::Fast).is_none());
}
/// Порог размера строгий: ровно порог — артефакт написан, на байт меньше —
/// пустышка.
#[test]
fn semantic_size_threshold_is_strict() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    let min = cfg.min_bytes as usize;
    let artifact = dir.join("SPEC.md");
    std::fs::write(&artifact, "s".repeat(min)).expect("write");
    let (findings, _notes) = semantic_check(dir, "spec", &artifact, &cfg, "error");
    assert!(
        !findings.iter().any(|f| f.rule == "evidence_stub"),
        "ровно порог — не пустышка: {findings:?}"
    );
    std::fs::write(&artifact, "s".repeat(min - 1)).expect("write");
    let (findings, _notes) = semantic_check(dir, "spec", &artifact, &cfg, "error");
    assert!(
        findings.iter().any(|f| f.rule == "evidence_stub"),
        "меньше порога — пустышка: {findings:?}"
    );
}

/// Заглушка A3 не разбирается по полям: у неё один честный диагноз
/// (не написан), а не пять «поле не заполнено».
#[test]
fn semantic_stub_a3_is_not_checked_field_by_field() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    let a3 = dir.join("DECISION.md");
    std::fs::write(&a3, "TODO").expect("write");
    let (findings, _notes) = semantic_check(dir, "decision_a3", &a3, &cfg, "error");
    assert!(
        findings.iter().any(|f| f.rule == "evidence_stub"),
        "заглушка названа: {findings:?}"
    );
    assert!(
        !findings.iter().any(|f| f.rule == "a3_not_signed"),
        "поля заглушки не разбираются: {findings:?}"
    );
}

/// Подпись A3 называется ровно один раз — по полю `decided_by`, а не по
/// каждому заполненному полю.
#[test]
fn semantic_a3_signature_note_names_only_decided_by() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    put_complete_critical(dir);
    let a3 = dir.join("DECISION.md");
    let (findings, notes) = semantic_check(dir, "decision_a3", &a3, &cfg, "error");
    assert!(
        !findings.iter().any(|f| f.rule == "a3_not_signed"),
        "полный A3 подписан: {findings:?}"
    );
    let signatures: Vec<&String> = notes.iter().filter(|n| n.contains("подпись A3")).collect();
    assert_eq!(signatures.len(), 1, "подпись названа один раз: {notes:?}");
}

/// Срок A3 «сегодня» — ещё не просрочен: сравнение строгое.
#[test]
fn semantic_a3_expiry_today_is_not_expired() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    put_complete_critical(dir);
    let today = chrono::Local::now().date_naive();
    let text = std::fs::read_to_string(dir.join("DECISION.md")).expect("read");
    let text = text
        .lines()
        .map(|l| {
            if l.contains("expiry") {
                format!("- **expiry**: {today}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect::<String>();
    std::fs::write(dir.join("DECISION.md"), text).expect("write");
    let a3 = dir.join("DECISION.md");
    let (findings, _notes) = semantic_check(dir, "decision_a3", &a3, &cfg, "error");
    assert!(
        !findings.iter().any(|f| f.rule == "a3_expired"),
        "срок истекает сегодня — не просрочен: {findings:?}"
    );
}

/// Отчёт-заглушка без машинной записи: диагноз уже назван («не написан»),
/// а с A1 рядом стоит `evidence_report_unbound` (на уровне verify) —
/// требовать ещё и строку итога у прозы — шум, прогон закрывает запись.
#[test]
fn semantic_stub_report_does_not_require_result_line() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    let report = dir.join("VALIDATION.md");
    std::fs::write(&report, "TODO").expect("write");
    let (findings, _notes) = semantic_check(dir, "validation", &report, &cfg, "warn");
    assert!(
        findings.iter().any(|f| f.rule == "evidence_stub"),
        "заглушка названа: {findings:?}"
    );
    assert!(
        !findings
            .iter()
            .any(|f| f.message.contains("нет строки итога")),
        "строку итога у заглушки не требуем: {findings:?}"
    );
}

/// Заглушка репетиции отката не разбирается как отчёт: у неё нет ни
/// списка шагов, ни якоря, и требовать их — шум.
#[test]
fn semantic_stub_rehearsal_is_not_parsed_as_report() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    let cfg = crate::config::EvidenceConfig::default();
    // Рядом лежит отчёт-заготовка: без охранной ветки её разбор дал бы
    // находку `rehearsal_empty` поверх честного «не написан».
    put(
        dir,
        "REHEARSAL.json",
        r#"{"kind":"rollback_rehearsal","gate":"A4","passed":true,
                "baseline_commit":"abc123","rehearsed_at":"2026-09-19T10:00:00Z",
                "duration_secs":1.5,"steps":[],"verify":null,
                "log":["репетиция отката прошла"]}"#,
    );
    let rehearsal = dir.join("ROLLBACK-REHEARSAL.md");
    std::fs::write(&rehearsal, "TODO").expect("write");
    let (findings, _notes) = semantic_check(dir, "rollback_rehearsal", &rehearsal, &cfg, "error");
    assert!(
        !findings.iter().any(|f| f.rule == "rehearsal_empty"),
        "заглушку не разбираем как отчёт: {findings:?}"
    );
    assert!(
        findings.iter().any(|f| f.rule == "evidence_stub"),
        "заглушка названа: {findings:?}"
    );
    let extra: Vec<&SemanticFinding> = findings
        .iter()
        .filter(|f| f.rule != "evidence_stub")
        .collect();
    assert!(extra.is_empty(), "лишних требований нет: {extra:?}");
}

/// Обязательные артефакты маршрута, которых нет в манифесте, попадают в
/// `missing` поимённо.
#[test]
fn verify_lists_missing_required_artifacts() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", &body("Проблема"));
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(!v.passed);
    for key in ["decision_a3", "spine", "adversarial_review"] {
        assert!(
            v.missing.contains(&key.to_string()),
            "{key} назван: {:?}",
            v.missing
        );
    }
}

/// A1 (0.3.14, репродукция «рукописный PASS»): отчёт о прогоне, написанный
/// прозой без машинной записи (`arch-be evidence record`), не доказывает
/// прогон — обязана быть находка `evidence_report_unbound`.
#[test]
fn verify_flags_unbound_handwritten_report() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    for key in ["validation", "fitness_report", "walking_skeleton"] {
        assert!(
            v.semantics
                .iter()
                .any(|f| f.rule == "evidence_report_unbound" && f.key == key),
            "рукописный отчёт «{key}» без машинной записи обязан быть назван: {:?}",
            v.semantics
        );
    }
    // По умолчанию — warn (не блокирует), по флагу require_records — error.
    assert!(v.passed, "warn не блокирует выпуск: {:?}", v.semantics);
    let strict = crate::config::EvidenceConfig {
        require_records: true,
        ..crate::config::EvidenceConfig::default()
    };
    let v = verify_with(dir, &strict).expect("verify");
    assert!(
        !v.passed,
        "с require_records=true рукописный PASS блокируется: {:?}",
        v.semantics
    );
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "evidence_report_unbound" && f.severity == "error"),
        "{:?}",
        v.semantics
    );
}

/// Запись прогона: команда, exit-код, HEAD, хэш входов, итог — в файле
/// `.arch-handoff/evidence/<kind>.json`.
#[test]
fn record_run_writes_machine_record() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "src/main.py", "print('ok')\n");
    let rec = record_run(dir, RecordKind::Tests, "true", 0).expect("record");
    assert!(rec.passed);
    assert_eq!(rec.exit_code, Some(0));
    assert_eq!(rec.schema, RECORD_SCHEMA);
    assert_eq!(rec.kind, "tests");
    assert_eq!(rec.head, "absent", "не git-репозиторий");
    assert_eq!(rec.inputs_hash.len(), 64, "sha256 hex");
    assert_eq!(rec.attestation.len(), 64);
    let path = record_path(dir, RecordKind::Tests);
    assert!(path.is_file(), "запись записана: {}", path.display());
    let loaded = load_run_record(dir, RecordKind::Tests)
        .expect("load")
        .expect("запись есть");
    assert_eq!(loaded.inputs_hash, rec.inputs_hash);
    // Детерминизм: повторный прогон на неизменном дереве — тот же хэш входов.
    let rec2 = record_run(dir, RecordKind::Tests, "true", 0).expect("record");
    assert_eq!(rec.inputs_hash, rec2.inputs_hash);
}

/// Провал прогона — тоже запись (passed=false, честный exit-код).
#[test]
fn record_run_failed_command_records_fail() {
    let tmp = tempfile::tempdir().expect("tmp");
    let rec = record_run(tmp.path(), RecordKind::Fitness, "false", 0).expect("record");
    assert!(!rec.passed);
    assert_eq!(rec.exit_code, Some(1));
}

/// Битая запись — ошибка разбора, а не молчание (подмена не замалчивается).
#[test]
fn load_run_record_rejects_garbage() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, ".arch-handoff/evidence/tests.json", "{ не json");
    assert!(load_run_record(dir, RecordKind::Tests).is_err());
}

/// Полный цикл A1: запись без markdown закрывает артефакт; правка кода
/// после записи делает её устаревшей (`evidence_record_stale`).
#[test]
fn verify_accepts_fresh_record_and_flags_stale_after_code_edit() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    // Мир A1: отчёты прогонов — машинные записи, markdown не пишется.
    for rel in ["VALIDATION.md", "reports/fitness.md", "WALKING-SKELETON.md"] {
        std::fs::remove_file(dir.join(rel)).expect("remove");
    }
    for kind in RecordKind::all() {
        record_run(dir, kind, "true", 0).expect("record");
    }
    let (bundle, v) = pack(dir, Route::Critical).expect("pack");
    assert!(v.passed, "missing: {:?}", v.missing);
    // Артефактом стала сама запись, а не проза.
    assert!(
        bundle
            .items
            .iter()
            .any(|i| i.key == "validation" && i.path.ends_with("evidence/tests.json")),
        "{:?}",
        bundle.items.iter().map(|i| &i.path).collect::<Vec<_>>()
    );
    let v = verify(dir).expect("verify");
    assert!(
        v.passed,
        "свежие записи закрывают артефакты без markdown: {:?}",
        v.semantics
    );
    assert!(
        v.semantics
            .iter()
            .all(|f| !f.rule.starts_with("evidence_record") && f.rule != "evidence_report_unbound"),
        "{:?}",
        v.semantics
    );
    // Правка кода после записи → запись устарела (приёмка A1).
    put(dir, "src/new_module.py", "def f(): ...\n");
    let v = verify(dir).expect("verify");
    assert!(
        !v.passed,
        "устаревшая запись обязана блокировать: {:?}",
        v.semantics
    );
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "evidence_record_stale" && f.key == "validation"),
        "{:?}",
        v.semantics
    );
}

/// Запись с провалом прогона — находка, даже когда рядом проза говорит PASS.
#[test]
fn verify_flags_failed_record_over_prose_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    record_run(dir, RecordKind::Tests, "false", 0).expect("record");
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    assert!(
        v.semantics
            .iter()
            .any(|f| f.rule == "evidence_record_failed" && f.key == "validation"),
        "{:?}",
        v.semantics
    );
    assert!(!v.passed);
}

/// Ни одна находка по артефактам прогонов не советует дописать текст,
/// закрывающий её без прогона (приёмка A1): все подсказки ведут к
/// `arch-be evidence record`.
#[test]
fn report_findings_never_teach_prose_bypass() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put_complete_critical(dir);
    // Разносим типы находок: проза-PASS, заглушка, FAIL в тексте.
    put(dir, "reports/fitness.md", "TODO");
    put(
        dir,
        "WALKING-SKELETON.md",
        &format!("{}\n\nИтог: FAIL\n", body("Скелет")),
    );
    record_run(dir, RecordKind::Fitness, "true", 0).expect("record");
    // Устаревшая запись: код изменился после прогона.
    put(dir, "src/changed.py", "x = 1\n");
    pack(dir, Route::Critical).expect("pack");
    let v = verify(dir).expect("verify");
    let report_findings: Vec<&SemanticFinding> = v
        .semantics
        .iter()
        .filter(|f| RecordKind::for_artifact(&f.key).is_some())
        .collect();
    assert!(
        report_findings.len() >= 4,
        "ожидались находки всех видов: {report_findings:?}"
    );
    for f in report_findings {
        assert!(
            f.fix_hint.contains("evidence record"),
            "подсказка обязана вести к машинной записи, а не к тексту: {f:?}"
        );
        for banned in ["добавьте", "допишите", "строку итога"] {
            assert!(
                !f.fix_hint.contains(banned),
                "подсказка не учит обходу («{banned}»): {f:?}"
            );
        }
    }
}

// --- A2: выводимые машиной артефакты -----------------------------------------

/// Запись значимости для тестов: без git и диффа — маршрут + пустые триггеры.
fn test_significance(route: Route) -> SignificanceRecord {
    SignificanceRecord {
        route: format!("{route:?}"),
        score: 0,
        triggers: Vec::new(),
        head: "absent".to_string(),
        recorded_at: "2026-10-07T00:00:00Z".to_string(),
    }
}

/// A2: `risk_level` выводится из записи значимости в EVIDENCE.yaml —
/// рукописный RISK.md не нужен ни при упаковке, ни при проверке.
#[test]
fn pack_with_significance_covers_risk_level_without_file() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    put(dir, "PROBLEM.md", &body("Проблема"));
    put(dir, "SPEC.md", &body("Спецификация"));
    put(dir, "ROLLBACK.md", &body("Откат"));
    // RISK.md намеренно НЕ написан.
    let sig = test_significance(Route::Fast);
    let (bundle, verdict) = pack_with(dir, Route::Fast, Some(sig)).expect("pack_with");
    assert!(
        verdict.passed,
        "risk_level выведен из записи значимости: {:?}",
        verdict.missing
    );
    assert!(bundle.significance.is_some());
    assert!(
        bundle.items.iter().all(|i| i.key != "risk_level"),
        "для выводимого артефакта нет файла-артефакта: {:?}",
        bundle.items.iter().map(|i| &i.key).collect::<Vec<_>>()
    );
    let v = verify(dir).expect("verify");
    assert!(
        v.passed,
        "проверка тоже считает risk_level по записи: {:?}",
        v.missing
    );
    // Без записи значимости поведение прежнее: нужен файл.
    let bare = tmp.path().join("bare");
    std::fs::create_dir_all(&bare).expect("mkdir");
    put(&bare, "PROBLEM.md", &body("Проблема"));
    let (_b, verdict) = pack(&bare, Route::Fast).expect("pack");
    assert!(verdict.missing.iter().any(|m| m == "risk_level"));
}

/// A2: раздельный счёт церемонии — «пишет автор X/8 · выведет машина Y/5»
/// на маршруте Critical; этими числами снимается замер до/после.
#[test]
fn bundle_progress_split_separates_author_from_machine() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    // Полный профиль Critical: 8 артефактов автора + 5 машинных.
    let author = [
        "problem",
        "spec_or_delta",
        "acceptance",
        "rollback",
        "adr_or_pattern",
        "spine",
        "decision_a3",
        "adversarial_review",
    ];
    let machine = [
        "risk_level",
        "validation",
        "fitness_report",
        "walking_skeleton",
        "rollback_rehearsal",
    ];
    assert_eq!(author.len(), 8);
    assert_eq!(machine.len(), 5);
    for key in author {
        assert!(!is_machine_derived(key), "{key} — артефакт автора");
    }
    for key in machine {
        assert!(is_machine_derived(key), "{key} выводится машиной");
    }
    // Пустая упаковка (только запись значимости): машина 1/5, автор 0/8.
    let sig = test_significance(Route::Critical);
    pack_with(dir, Route::Critical, Some(sig)).expect("pack");
    let p = bundle_progress_split(dir, Route::Critical).expect("прогресс");
    assert_eq!(
        (
            p.author_done,
            p.author_total,
            p.machine_done,
            p.machine_total
        ),
        (0, 8, 1, 5),
        "{p:?}"
    );
    // Совместимость: свёрнутый прогресс — сумма двух долей.
    assert_eq!(bundle_progress(dir, Route::Critical), Some(1));
}

/// A2: запись значимости несёт источники триггеров (S-1) — детектор диффа.
#[test]
fn significance_record_captures_triggers_with_sources() {
    let tmp = tempfile::tempdir().expect("tmp");
    let dir = tmp.path();
    // Репозиторий с новым компонентом модели в рабочем дереве.
    git(dir, &["init", "-q"]);
    put(dir, "README.md", "x\n");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "base"]);
    put(
        dir,
        "model/CMP-001-novy.md",
        "---\nid: CMP-001\ntype: cmp\n---\n",
    );
    let rec = significance_record(
        dir,
        Route::Standard,
        (1, 4),
        &crate::control::DiffGlobs::default(),
    );
    assert_eq!(rec.route, "Standard");
    assert_eq!(rec.head.len(), 40, "sha коммита: {}", rec.head);
    assert!(
        rec.triggers
            .iter()
            .any(|t| t.name == "new_component" && t.source == "diff"),
        "триггер из диффа с источником: {:?}",
        rec.triggers
    );
    assert_eq!(rec.score, rec.triggers.len());
}

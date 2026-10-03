use super::*;

/// Конфиг с assets внутри временного каталога (изоляция от ~/.arch-harness).
fn cfg_in(dir: &Path) -> Config {
    let mut cfg = Config::default();
    cfg.paths.assets_dir = dir.join("assets");
    cfg
}

fn write_file(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, text).expect("write");
}

const SPINE: &str = "# Spine\n\n\
        ## AD-1: Единый стек\n\n\
        **Binds:** все сервисы — Rust 1.85.\n\n\
        **Prevents:** зоопарк языков в контуре.\n\n\
        **Rule:** в CI закреплён toolchain 1.85.\n\n\
        ## Прочее\n\n\
        Абзац один.\n\n\
        Абзац два.\n\n\
        Абзац три — не должен попасть в контекст.\n";

/// git-репозиторий с одним baseline-коммитом (явная идентичность —
/// на CI/в контейнерах user.name/user.email может не быть).
fn git_repo_with_baseline(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).expect("mkdir repo");
    std::fs::write(dir.join("README.md"), "# baseline\n").expect("readme");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
    };
    git(&["init", "-q"]);
    git(&["add", "README.md"]);
    git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@test",
        "commit",
        "-q",
        "-m",
        "baseline",
    ]);
}

#[test]
fn generates_full_packet_and_preserves_user_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    // Маркер Rust-стека: дефолтные CONSTRAINTS — cargo-правила.
    write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
    let cfg = cfg_in(tmp.path());
    write_file(
        &cfg.paths.rubrics_dir().join("handoff_quality.yaml"),
        "# якорная рубрика\n",
    );
    let spine = tmp.path().join("specs/spine.md");
    write_file(&spine, SPINE);
    let adr = tmp.path().join("specs/adr/ADR-001.md");
    write_file(&adr, "# ADR-001\n\nСтатус: Accepted.\n");
    let notes = tmp.path().join("specs/notes.md");
    write_file(&notes, "# Заметки\n\nпервый\n\nвторой\n\nтретий\n");

    let packet = generate_handoff(
        &repo,
        "сделать фичу X",
        &[spine.clone(), adr.clone(), notes.clone()],
        &cfg,
        None,
        Route::Standard,
    )
    .expect("handoff");
    let dir = repo.join(".arch-handoff");
    assert_eq!(packet.dir, dir);

    let task_md = std::fs::read_to_string(dir.join("TASK.md")).expect("TASK.md");
    assert!(task_md.contains("сделать фичу X"));
    // Финализация: контракт требует git-коммита результата (иначе
    // оркестратор работу не увидит — регрессия «агенты без коммита»).
    assert!(task_md.contains("## Финализация (обязательно)"));
    assert!(task_md.contains("git add -A -- . ':!.arch-handoff'"));
    // План отката с якорем baseline (рубрика handoff_quality::rollback_plan).
    assert!(task_md.contains("## План отката"));
    let baseline = packet.baseline.as_deref().expect("baseline-якорь");
    assert!(
        task_md.contains(&format!("git reset --hard {baseline}")),
        "план отката с якорем:\n{task_md}"
    );
    assert!(task_md.contains("Владелец решения об откате"));
    assert!(
        packet.git_initialized,
        "не-git каталог — предгейт делает init"
    );
    assert!(task_md.contains("## Контракт результата"));
    assert!(task_md.contains("\"complete|partial|blocked\""));

    // MANIFEST несёт маршрут и рекомендованный таймаут (подхват harness_run).
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("MANIFEST.json")).expect("MANIFEST.json"),
    )
    .expect("manifest json");
    assert_eq!(manifest["route"], "Standard");
    assert_eq!(manifest["recommended_timeout_secs"], 3600);
    assert_eq!(recommended_timeout_secs(&repo), Some(3600));

    let arch = std::fs::read_to_string(dir.join("ARCHITECTURE.md")).expect("ARCHITECTURE.md");
    // ADR-блок включён целиком (все три поля на месте).
    for field in ["**Binds:**", "**Prevents:**", "**Rule:**"] {
        assert!(arch.contains(field), "нет поля {field}");
    }
    // Прочие секции — заголовок + первые абзацы; спека мелкая, поэтому
    // сработала адаптивная глубина (окно рубрики 800–1500 токенов): все
    // три абзаца включены.
    assert!(arch.contains("Абзац два."));
    assert!(arch.contains("Абзац три"), "глубокий рендер:\n{arch}");
    assert!(arch.contains("Источники:"));

    // CONSTRAINTS.yaml создан с дефолтными правилами.
    let constraints = dir.join("CONSTRAINTS.yaml");
    let c = std::fs::read_to_string(&constraints).expect("CONSTRAINTS.yaml");
    for marker in [
        "must_not_contain",
        "unwrap",
        "dbg!",
        "file_exists",
        "command_succeeds",
        "cargo check",
        "timeout_secs: 120",
    ] {
        assert!(c.contains(marker), "CONSTRAINTS.yaml: нет '{marker}'");
    }

    // RUBRIC.yaml — копия якорной рубрики.
    let rubric = std::fs::read_to_string(dir.join("RUBRIC.yaml")).expect("RUBRIC.yaml");
    assert_eq!(rubric, "# якорная рубрика\n");

    // MANIFEST.json — мета пакета.
    let manifest_text = std::fs::read_to_string(dir.join("MANIFEST.json")).expect("MANIFEST.json");
    let manifest: Value = serde_json::from_str(&manifest_text).expect("manifest json");
    assert_eq!(manifest["task"], "сделать фичу X");
    assert!(manifest["created_at"].is_string());
    assert_eq!(manifest["sources"].as_array().expect("sources").len(), 3);
    let chars = manifest["epic_context_chars"].as_u64().expect("chars") as usize;
    assert_eq!(chars, arch.chars().count());
    let tokens = manifest["epic_context_tokens"].as_u64().expect("tokens") as usize;
    assert_eq!(tokens, chars / 4);
    assert_eq!(packet.epic_context_tokens, tokens);

    // adr/ — копия ADR-файла.
    assert!(dir.join("adr/ADR-001.md").is_file());
    assert!(packet.files.contains(&dir.join("adr/ADR-001.md")));

    // Повторный прогон: пользовательские CONSTRAINTS/RUBRIC не затираются,
    // TASK.md и MANIFEST.json перезаписываются.
    std::fs::write(&constraints, "# пользовательские правила\n").expect("custom constraints");
    std::fs::write(dir.join("RUBRIC.yaml"), "# пользовательская рубрика\n").expect("custom rubric");
    let packet2 = generate_handoff(
        &repo,
        "другая задача",
        &[spine, adr, notes],
        &cfg,
        None,
        Route::Standard,
    )
    .expect("second handoff");
    assert_eq!(
        std::fs::read_to_string(&constraints).expect("constraints after"),
        "# пользовательские правила\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("RUBRIC.yaml")).expect("rubric after"),
        "# пользовательская рубрика\n"
    );
    assert!(
        std::fs::read_to_string(dir.join("TASK.md"))
            .expect("TASK.md after")
            .contains("другая задача")
    );
    assert!(packet2.files.contains(&constraints));
}

#[test]
fn handoff_includes_spec_template_and_preserves_filled_spec() {
    // SPEC.md — шаблон верифицируемых контрактов интерфейсов (модель
    // «5.2»: контракты вместо прозы ARCHITECTURE.md компонента); пишется
    // один раз и не затирается повторной генерацией, как CONSTRAINTS.yaml.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());

    let packet = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let spec_path = packet.dir.join("SPEC.md");
    assert!(packet.files.contains(&spec_path), "{:?}", packet.files);
    let spec = std::fs::read_to_string(&spec_path).expect("SPEC.md");
    for section in [
        "## Входы (контракты соседей)",
        "## Выходы (публикуемые контракты)",
        "## Структуры данных",
        "## Границы ошибок",
        "## Критерии верификации (тесты)",
    ] {
        assert!(spec.contains(section), "нет секции «{section}»:\n{spec}");
    }
    // EARS-подсказка на месте.
    assert!(
        spec.contains("When <событие>, the <система> shall"),
        "{spec}"
    );
    // TASK.md несёт пункт чеклиста про SPEC.md.
    let task_md = std::fs::read_to_string(packet.dir.join("TASK.md")).expect("TASK.md");
    assert!(task_md.contains("SPEC.md"), "{task_md}");

    // Заполненный SPEC.md повторная генерация не затирает.
    std::fs::write(&spec_path, "# SPEC\n\nЗаполнено архитектором.\n").expect("fill spec");
    let packet2 =
        generate_handoff(&repo, "задача 2", &[], &cfg, None, Route::Fast).expect("handoff 2");
    assert_eq!(
        std::fs::read_to_string(&spec_path).expect("SPEC.md after"),
        "# SPEC\n\nЗаполнено архитектором.\n"
    );
    assert!(packet2.files.contains(&spec_path));
}

#[test]
fn handoff_fills_spec_from_passed_spec_files() {
    // Кейс 2026-09-01: исполнители (theseus, codewhale) спотыкались о
    // пустой SPEC.md-шаблон, когда архитектор передал контракты через
    // --spec: теперь SPEC.md собирается из полных текстов переданных спек.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("docs")).expect("mkdir");
    let cfg = cfg_in(tmp.path());
    let spec_src = repo.join("docs/SPEC-meta.md");
    std::fs::write(
        &spec_src,
        "# Спека агента\n\n## Выходы\n\n- карточка ТчВ, идемпотентность по id\n",
    )
    .expect("write spec");
    let packet = generate_handoff(
        &repo,
        "задача",
        std::slice::from_ref(&spec_src),
        &cfg,
        None,
        Route::Fast,
    )
    .expect("handoff");
    let spec = std::fs::read_to_string(packet.dir.join("SPEC.md")).expect("SPEC.md");
    assert!(spec.contains("Источник:"), "{spec}");
    assert!(
        spec.contains("карточка ТчВ, идемпотентность по id"),
        "{spec}"
    );
    assert!(
        !spec.contains("<что компонент потребляет"),
        "шаблон не нужен: {spec}"
    );
    // Правка архитектора поверх собранного файла не затирается.
    let spec_path = packet.dir.join("SPEC.md");
    std::fs::write(&spec_path, "# SPEC\n\nУточнено архитектором.\n").expect("edit");
    generate_handoff(
        &repo,
        "задача 2",
        std::slice::from_ref(&spec_src),
        &cfg,
        None,
        Route::Fast,
    )
    .expect("handoff 2");
    assert_eq!(
        std::fs::read_to_string(&spec_path).expect("SPEC.md after"),
        "# SPEC\n\nУточнено архитектором.\n"
    );
}

/// E5.3: модель-автор кода берётся из контракта передачи; пустое значение
/// и отсутствие пакета — не автор, а не догадка.
#[test]
fn contract_author_model_is_read_from_manifest() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path();
    assert!(
        author_model_from_contract(repo).is_none(),
        "без пакета автора нет"
    );
    std::fs::create_dir_all(repo.join(HANDOFF_DIR)).expect("mkdir");
    std::fs::write(
        repo.join(HANDOFF_DIR).join("MANIFEST.json"),
        "{\"created_at\":\"2026-09-25T00:00:00+00:00\",\"task\":\"t\",\"model\":\"code-agent-x\"}",
    )
    .expect("manifest");
    assert_eq!(
        author_model_from_contract(repo).as_deref(),
        Some("code-agent-x")
    );
    std::fs::write(
        repo.join(HANDOFF_DIR).join("MANIFEST.json"),
        "{\"created_at\":\"2026-09-25T00:00:00+00:00\",\"task\":\"t\",\"model\":\"  \"}",
    )
    .expect("manifest 2");
    assert!(
        author_model_from_contract(repo).is_none(),
        "пробелы — не автор"
    );
}

/// Модель с QAS в `<repo>/model/` для тестов критериев приёмки (ADR-007).
fn repo_with_qas_model(repo: &Path) {
    write_file(
        &repo.join("model/NFR-001-lat.md"),
        "---\nid: NFR-001\ntype: nfr\ntitle: Latency\nstatus: accepted\nverification: hist\n---\n\np99 < 2s.\n",
    );
    write_file(
        &repo.join("model/QAS-001-peak.md"),
        "---\nid: QAS-001\ntype: qas\ntitle: Пиковая нагрузка\nstatus: accepted\n\
             implements: [NFR-001]\nsource: клиент канала\nstimulus: запрос авторизации в пике 5000 TPS\n\
             artifact: CMP-003 Authorization\nresponse: ответ об авторизации возвращён\n\
             measure: p99 < 2000 мс (NFR-001)\n---\n\nПроза.\n",
    );
}

#[test]
fn handoff_unfolds_qas_into_acceptance_criteria() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    repo_with_qas_model(&repo);
    let cfg = cfg_in(tmp.path());

    generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let task_md = std::fs::read_to_string(repo.join(".arch-handoff/TASK.md")).expect("TASK.md");
    // Секция появилась автоматически, без ручного копирования (DoD P1-1).
    assert!(
        task_md.contains("## Критерии приёмки (QAS из модели)"),
        "{task_md}"
    );
    assert!(task_md.contains("QAS-001"), "{task_md}");
    assert!(
        task_md.contains("запрос авторизации в пике 5000 TPS"),
        "{task_md}"
    );
    assert!(task_md.contains("p99 < 2000 мс (NFR-001)"), "{task_md}");
    // Секция стоит после задачи и до плана отката.
    let task_pos = task_md.find("задача").expect("задача");
    let qas_pos = task_md.find("## Критерии приёмки").expect("секция");
    let rollback_pos = task_md.find("## План отката").expect("откат");
    assert!(task_pos < qas_pos && qas_pos < rollback_pos, "{task_md}");
}

#[test]
fn handoff_without_model_or_qas_has_no_acceptance_section() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let task_md = std::fs::read_to_string(repo.join(".arch-handoff/TASK.md")).expect("TASK.md");
    assert!(!task_md.contains("Критерии приёмки (QAS"), "{task_md}");

    // Модель есть, но QAS в ней нет — секции тоже нет.
    let repo2 = tmp.path().join("repo2");
    std::fs::create_dir_all(&repo2).expect("mkdir repo2");
    write_file(
        &repo2.join("model/CMP-001-x.md"),
        "---\nid: CMP-001\ntype: cmp\ntitle: X\nstatus: designed\n---\n",
    );
    generate_handoff(&repo2, "задача", &[], &cfg, None, Route::Fast).expect("handoff 2");
    let task_md2 = std::fs::read_to_string(repo2.join(".arch-handoff/TASK.md")).expect("TASK.md 2");
    assert!(!task_md2.contains("Критерии приёмки (QAS"), "{task_md2}");
}

#[test]
fn handoff_with_broken_model_fails_loudly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    write_file(&repo.join("model/broken.md"), "нет frontmatter\n");
    let cfg = cfg_in(tmp.path());
    let err = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast)
        .expect_err("битая модель — ошибка, не молчаливый пропуск");
    assert!(err.to_string().contains("QAS"), "{err}");
}

#[test]
fn handoff_git_pregate_is_idempotent() {
    // Предгейт: не-git каталог получает git init + пустой baseline-якорь;
    // повторная генерация якорь не двигает (HEAD — тот же коммит).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());

    let p1 = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff 1");
    assert!(p1.git_initialized);
    let b1 = p1.baseline.clone().expect("baseline 1");
    assert_eq!(recommended_timeout_secs(&repo), Some(1800), "fast → 1800");

    let p2 = generate_handoff(&repo, "задача 2", &[], &cfg, None, Route::Fast).expect("handoff 2");
    assert!(!p2.git_initialized, "повторный init не нужен");
    assert_eq!(p2.baseline.as_deref(), Some(b1.as_str()), "якорь стабилен");
    // Baseline — пустой коммит, содержимое каталога не подмётено.
    let count = git_out(&repo, &["log", "--oneline"]).expect("git log");
    assert_eq!(count.lines().count(), 1, "{count}");
}

#[test]
fn handoff_explicit_rollback_and_critical_route() {
    // Явный план отката попадает в TASK.md дословно; маршрут Critical
    // даёт рекомендованный таймаут 7200 (регрессия: Critical-прогон
    // обрывался на дефолтных 30 минутах адаптера). Critical требует
    // epic-context в окне рубрики — даём объёмную спеку.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let spec = big_spec(tmp.path());

    let packet = generate_handoff(
        &repo,
        "миграция ядра",
        &[spec],
        &cfg,
        Some("Шаг 1: вернуть флаг фичи. Шаг 2: restore из snapshot БД."),
        Route::Critical,
    )
    .expect("handoff");
    assert_eq!(packet.recommended_timeout_secs, 7200);
    assert_eq!(recommended_timeout_secs(&repo), Some(7200));
    let task_md = std::fs::read_to_string(packet.dir.join("TASK.md")).expect("TASK.md");
    assert!(task_md.contains("## План отката"));
    assert!(
        task_md.contains("Шаг 1: вернуть флаг фичи. Шаг 2: restore из snapshot БД."),
        "явный откат дословно:\n{task_md}"
    );
    // Автотекст не подмешивается к явному плану.
    assert!(!task_md.contains("Сигналы отката"));
    // Несуществующий пакет — None (адаптер берёт свой дефолт).
    assert_eq!(recommended_timeout_secs(tmp.path()), None);
}

#[test]
fn critical_route_refuses_thin_epic_context() {
    // Разрыв P2: для Critical контроль нижней границы окна рубрики —
    // отказ на сборке пакета, а не молчаливое предупреждение.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let err = generate_handoff(&repo, "миграция ядра", &[], &cfg, None, Route::Critical)
        .expect_err("Critical без спек обязан отказывать");
    let msg = err.to_string();
    assert!(msg.contains("ниже окна рубрики"), "{msg}");
    assert!(msg.contains("spec"), "{msg}");
    // Fast/Standard на том же объёме — собираются (Fast молча, Standard с warning).
    generate_handoff(&repo, "фикс", &[], &cfg, None, Route::Fast).expect("Fast ок");
    generate_handoff(&repo, "фикс", &[], &cfg, None, Route::Standard).expect("Standard ок");
}

/// Объёмная спека для Critical (epic-context в окне рубрики).
fn big_spec(tmp: &Path) -> PathBuf {
    let mut big_text = String::from("# Спека миграции\n\n");
    for i in 0..60 {
        let _ = write!(
            big_text,
            "## Блок {i}\n\nИнвариант: AD-{i} — дословное правило интеграции, \
                 проверяемое тестом; детали, стыки и запреты для полноты контекста.\n\n"
        );
    }
    let spec = tmp.join("spec-big.md");
    write_file(&spec, &big_text);
    spec
}

#[test]
fn handoff_generates_machine_readable_rollback_plan() {
    // ROLLBACK.yaml — машиночитаемый план отката (репетиция на гейте A4):
    // baseline-якорь + шаги по умолчанию, парсится rehearsal::load_plan;
    // MANIFEST.json несёт baseline_commit и rollback_plan; правки
    // архитектора повторная генерация не затирает (как CONSTRAINTS.yaml).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());

    let packet = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let plan_path = packet.dir.join(crate::rehearsal::ROLLBACK_FILE);
    assert!(packet.files.contains(&plan_path), "{:?}", packet.files);
    let baseline = packet.baseline.clone().expect("baseline");
    let plan = crate::rehearsal::load_plan(&packet.dir).expect("план парсится");
    assert_eq!(plan.baseline_commit, baseline);
    assert_eq!(plan.steps.len(), 2, "якорь + откат: {:?}", plan.steps);
    assert!(plan.steps[1].run.contains("git reset --hard"));
    assert!(plan.verify.is_some(), "verify чистоты дерева");

    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(packet.dir.join("MANIFEST.json")).expect("MANIFEST.json"),
    )
    .expect("manifest json");
    assert_eq!(manifest["baseline_commit"], baseline.as_str());
    assert!(
        manifest["rollback_plan"]
            .as_str()
            .expect("rollback_plan")
            .contains("Сигналы отката"),
        "дефолтный план в манифесте"
    );

    // Правка архитектора не затирается повторной генерацией.
    std::fs::write(
        &plan_path,
        "baseline_commit: \"\"\nsteps: []\n# пользовательский план\n",
    )
    .expect("custom plan");
    generate_handoff(&repo, "задача 2", &[], &cfg, None, Route::Fast).expect("handoff 2");
    assert!(
        std::fs::read_to_string(&plan_path)
            .expect("plan after")
            .contains("пользовательский план")
    );
}

#[test]
fn critical_route_requires_baseline_and_rollback_plan() {
    // Rollback-first для Critical: пустой явный план отката — ошибка
    // сборки пакета (репетиции на A4 нечего прогонять).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let spec = big_spec(tmp.path());
    let err = generate_handoff(
        &repo,
        "миграция",
        std::slice::from_ref(&spec),
        &cfg,
        Some("   "),
        Route::Critical,
    )
    .expect_err("Critical с пустым rollback обязан отказывать");
    assert!(err.to_string().contains("rollback"), "{err}");
    // Дефолтный план (откат на baseline) — валиден: baseline от предгейта.
    let packet =
        generate_handoff(&repo, "миграция", &[spec], &cfg, None, Route::Critical).expect("ok");
    assert!(packet.baseline.is_some());
    // Для Fast пустой явный план — не ошибка маршрута (пакет собирается).
    let repo2 = tmp.path().join("repo2");
    std::fs::create_dir_all(&repo2).expect("mkdir repo2");
    generate_handoff(&repo2, "фикс", &[], &cfg, Some(""), Route::Fast).expect("Fast ок");
}

#[test]
fn handoff_warns_on_dirty_tracked_tree() {
    // Хвост предгейта: грязные ОТСЛЕЖИВАЕМЫЕ файлы — откат на baseline
    // их потеряет; предупреждаем при генерации пакета.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    git_repo_with_baseline(&repo);
    let cfg = cfg_in(tmp.path());
    let p = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    assert!(!p.git_dirty_tracked, "чистое дерево — без предупреждения");
    // Модифицируем отслеживаемый файл без коммита.
    std::fs::write(repo.join("README.md"), "# изменено\n").expect("edit");
    let p = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    assert!(p.git_dirty_tracked, "грязное дерево — флаг выставлен");
    // Untracked-файлы грязью не считаются (reset --hard их не трогает).
    let p3_repo = tmp.path().join("repo3");
    git_repo_with_baseline(&p3_repo);
    std::fs::write(p3_repo.join("new-file.py"), "x = 1\n").expect("untracked");
    let p3 = generate_handoff(&p3_repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff 3");
    assert!(!p3.git_dirty_tracked, "untracked — не грязь");
}

#[test]
fn long_spec_is_truncated_with_notice() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let big = tmp.path().join("big.md");
    let mut text = String::from("# Большая спека\n\n");
    for i in 0..500 {
        let _ = write!(
            text,
            "## Секция {i}\n\nДостаточно длинный абзац, чтобы набрать объём контекста.\n\n"
        );
    }
    write_file(&big, &text);

    let packet =
        generate_handoff(&repo, "задача", &[big], &cfg, None, Route::Standard).expect("handoff");
    let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
    assert!(
        arch.chars().count() <= EPIC_CONTEXT_MAX_CHARS,
        "len = {}",
        arch.chars().count()
    );
    assert!(arch.contains("Контекст усечён"));
}

#[test]
fn default_constraints_follow_repo_stack() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");

    // Пустой репозиторий — общий минимум.
    let generic = default_constraints(&repo);
    assert!(generic.contains("readme-exists"), "{generic}");
    assert!(generic.contains("Стек: generic"), "{generic}");
    assert!(!generic.contains("cargo check"), "{generic}");

    write_file(&repo.join("requirements.txt"), "pytest\n");
    let py = default_constraints(&repo);
    assert!(py.contains("pytest -q"), "{py}");
    assert!(py.contains("print\\("), "{py}");
    assert!(!py.contains("cargo check"), "{py}");

    std::fs::remove_file(repo.join("requirements.txt")).expect("rm");
    write_file(&repo.join("go.mod"), "module demo\n");
    let go = default_constraints(&repo);
    assert!(go.contains("go build ./..."), "{go}");

    std::fs::remove_file(repo.join("go.mod")).expect("rm");
    write_file(&repo.join("package.json"), "{}\n");
    assert!(default_constraints(&repo).contains("npm test"));

    std::fs::remove_file(repo.join("package.json")).expect("rm");
    write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
    assert!(default_constraints(&repo).contains("cargo check"));
}

#[test]
fn epic_context_deepens_below_rubric_window() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    // Секция с пятью абзацами: на мелкой глубине (2) контекст ниже окна
    // рубрики — рендер углубляется, хвост секции доезжает.
    let spec = tmp.path().join("spec.md");
    write_file(
        &spec,
        "# Спека\n\n## Детали\n\nпервый\n\nвторой\n\nтретий\n\nчетвёртый\n\nпятый\n",
    );
    let packet =
        generate_handoff(&repo, "задача", &[spec], &cfg, None, Route::Standard).expect("handoff");
    let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
    assert!(
        arch.contains("пятый"),
        "глубокий рендер дотянул хвост:\n{arch}"
    );
}

#[tokio::test]
async fn handoff_create_warns_when_epic_below_window() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let tool = HandoffCreateTool::new(cfg.clone());
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));
    // Без спек epic-context ≈ один заголовок — ниже окна рубрики.
    let out = tool
        .call(json!({"repo": "repo", "task": "x"}), &ctx)
        .await
        .expect("call");
    assert!(out.content.contains("ниже окна рубрики"), "{}", out.content);
    // T-02: в выводе названо, ЧТО стало с пакетным реестром — гейт читает
    // именно его. Корневого реестра здесь нет, значит заготовка под стек.
    assert!(
        out.content
            .contains("Реестр правил пакета: заготовка под стек"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn handoff_create_tool_reports_summary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let tool = HandoffCreateTool::new(cfg.clone());
    assert_eq!(tool.spec().name, "handoff_create");
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));

    // Нет обязательного аргумента.
    let out = tool.call(json!({"task": "x"}), &ctx).await.expect("call");
    assert!(out.is_error);
    assert!(out.content.contains("'repo'"));

    // Полный вызов (repo относительно cwd).
    let out = tool
        .call(json!({"repo": "repo", "task": "сделать Y"}), &ctx)
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("Handoff-пакет создан"));
    assert!(repo.join(".arch-handoff/TASK.md").is_file());
}

#[test]
fn default_constraints_yaml_is_valid_for_every_stack() {
    // Дефект A1: шаблоны теряли 2-пробельный отступ первой строки и
    // выдавали YAML с ScannerError. Для каждого из 5 стеков итоговый
    // документ (с корнем `rules:`) обязан парситься и как YAML, и по
    // боевой схеме fitness-правил.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let stacks: [(&str, &str, &str); 5] = [
        ("generic", "", ""),
        ("Rust", "Cargo.toml", "[package]\nname = \"demo\"\n"),
        ("Python", "requirements.txt", "pytest\n"),
        ("Go", "go.mod", "module demo\n"),
        ("Node", "package.json", "{}\n"),
    ];
    let mut marker: Option<&str> = None;
    for (stack, file, content) in stacks {
        if let Some(prev) = marker.take() {
            std::fs::remove_file(repo.join(prev)).expect("remove marker");
        }
        if !file.is_empty() {
            write_file(&repo.join(file), content);
            marker = Some(file);
        }
        let text = default_constraints(&repo);
        assert!(text.contains(&format!("Стек: {stack}")), "{text}");
        let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)
            .unwrap_or_else(|e| panic!("стек {stack}: YAML не парсится: {e}\n{text}"));
        let rules = parsed["rules"]
            .as_sequence()
            .unwrap_or_else(|| panic!("стек {stack}: нет списка rules:\n{text}"));
        assert!(!rules.is_empty(), "стек {stack}: пустые rules:\n{text}");
        for rule in rules {
            for key in ["name", "type", "severity"] {
                assert!(
                    rule[key].is_string(),
                    "стек {stack}: у правила нет '{key}':\n{text}"
                );
            }
        }
        // Боевая схема (control::check): файл читается загрузчиком правил.
        let path = tmp.path().join("CONSTRAINTS.yaml");
        write_file(&path, &text);
        let loaded = crate::control::load_fitness_rules(&path)
            .unwrap_or_else(|e| panic!("стек {stack}: схема не принимает: {e}\n{text}"));
        assert_eq!(loaded.len(), rules.len(), "стек {stack}");
    }
}

#[test]
fn constraints_self_validation_rejects_broken_yaml() {
    // Самовалидация генератора: битый YAML отклоняется до записи файла —
    // лучше упасть, чем выдать исполнителю нечитаемый CONSTRAINTS.yaml.
    let broken = "rules:\n- name: x\n    type: must_not_contain\n";
    let err = validate_constraints_text(broken).expect_err("битый YAML — ошибка");
    assert!(err.to_string().contains("дефект шаблонов"), "{err}");
    let valid =
        "rules:\n  - name: x\n    type: file_exists\n    path: README.md\n    severity: warn\n";
    validate_constraints_text(valid).expect("валидный YAML проходит");
}

#[test]
fn handoff_writes_parseable_constraints_yaml() {
    // Сквозная проверка A1: записанный в пакет CONSTRAINTS.yaml парсится
    // (раньше исполнителю уезжал файл с ScannerError).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
    let cfg = cfg_in(tmp.path());
    let packet = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let text =
        std::fs::read_to_string(packet.dir.join("CONSTRAINTS.yaml")).expect("CONSTRAINTS.yaml");
    let parsed: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&text).expect("CONSTRAINTS.yaml пакета парсится");
    assert!(parsed["rules"].as_sequence().is_some_and(|r| !r.is_empty()));
}

#[test]
fn epic_context_ladder_preserves_adr_blocks_verbatim() {
    // Дефект A2: тупое усечение по символам обрезало инвариант AD-010 на
    // полуслове. Фикстура: сумма > EPIC_CONTEXT_MAX_CHARS, несколько
    // AD-блоков в хвосте — все они обязаны остаться дословно и целиком,
    // сноска перечисляет сокращённые и выкинутые секции.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let mut text = String::from("# Спека эпика\n\n");
    for i in 0..120 {
        let _ = write!(
            text,
            "## Прозаическая секция номер {i} с длинным хвостом имени\n\n\
                 Наполнитель PROSE-{i}: длинный абзац прозы про контракты, стыки, \
                 ограничения и детали реализации для объёма контекста.\n\n"
        );
    }
    for i in 10..13 {
        let _ = write!(
            text,
            "## AD-{i}: Инвариант интеграции\n\n\
                 **Binds:** MARKER-BINDS-{i} — дословное правило стыковки компонентов.\n\n\
                 **Prevents:** MARKER-PREVENTS-{i} — запрещённый класс отказов.\n\n\
                 **Rule:** MARKER-RULE-{i} — финальная строка правила, обязана доехать целиком.\n\n"
        );
    }
    let spec = tmp.path().join("spec-ladder.md");
    write_file(&spec, &text);

    let packet =
        generate_handoff(&repo, "задача", &[spec], &cfg, None, Route::Standard).expect("handoff");
    let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
    // Все AD-блоки присутствуют дословно, целиком — до последней строки.
    for i in 10..13 {
        for marker in [
            format!("**Binds:** MARKER-BINDS-{i}"),
            format!("**Prevents:** MARKER-PREVENTS-{i}"),
            format!(
                "**Rule:** MARKER-RULE-{i} — финальная строка правила, обязана доехать целиком."
            ),
        ] {
            assert!(arch.contains(&marker), "AD-{i} обрезан ({marker}):\n{arch}");
        }
    }
    // Честная сноска: маркер усечения + перечень сокращённого и выкинутого.
    assert!(arch.contains("Контекст усечён"), "{arch}");
    assert!(arch.contains("сокращены до заголовков"), "{arch}");
    assert!(
        arch.contains("выкинуты прозаические секции с хвоста"),
        "{arch}"
    );
    assert!(arch.contains("дословно"), "{arch}");
    // Секция 119 выкинута с хвоста (есть в сноске, но нет как заголовка).
    assert!(arch.contains("Прозаическая секция номер 119"), "{arch}");
    assert!(
        !arch.contains("## Прозаическая секция номер 119 с длинным хвостом имени"),
        "{arch}"
    );
    // Ранняя секция осталась (хотя бы заголовком), AD не пострадали.
    assert!(arch.contains("## Прозаическая секция номер 0"), "{arch}");
    assert!(
        arch.chars().count() <= EPIC_CONTEXT_MAX_CHARS,
        "len = {}",
        arch.chars().count()
    );
}

/// Секции сгенерированного ARCHITECTURE.md по заголовкам `## ` (без
/// сноски об усечении): (текст заголовка без `#`, тело до следующего
/// заголовка). Хелпер охранных проверок «сноска не врёт» (D1).
fn arch_ad_sections(arch: &str) -> Vec<(String, String)> {
    // Сноска примыкает к последней секции — отрезаем её, иначе текст
    // сноски попадёт в «тело» последней секции и замаскирует пустоту.
    let region = arch
        .split_once("\n\n> **Контекст усечён**")
        .map_or(arch, |(before, _)| before);
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    for line in region.lines() {
        if line.starts_with("## ") {
            if let Some(s) = cur.take() {
                sections.push(s);
            }
            cur = Some((
                line.trim_start_matches('#').trim().to_string(),
                String::new(),
            ));
        } else if let Some((_, body)) = cur.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(s) = cur.take() {
        sections.push(s);
    }
    sections
}

/// Охранная сверка сноски с фактом (D1): каждая AD/ADR-секция выхода
/// имеет непустое тело; если сноска декларирует дословность, ни одна
/// AD/ADR-секция не урезана. Возвращает число AD/ADR-заголовков.
fn assert_notice_matches_fact(arch: &str) -> usize {
    let sections = arch_ad_sections(arch);
    let ad_sections: Vec<&(String, String)> = sections
        .iter()
        .filter(|(title, _)| is_ad_title(title))
        .collect();
    for (title, body) in &ad_sections {
        assert!(
            !body.trim().is_empty(),
            "секция «{title}» без тела:\n{arch}"
        );
    }
    if arch.contains("приведены дословно и не сокращались") {
        assert!(
            !arch.contains("дословность AD-блоков НЕ гарантируется"),
            "сноска противоречит сама себе:\n{arch}"
        );
    }
    ad_sections.len()
}

#[test]
fn adr_field_detector_accepts_markdown_forms() {
    // D1: детектор ADR-блока толерантен к формам полей из реальных
    // спайнов (линтер `control spine` их принимает — epic-context обязан
    // узнавать те же формы, иначе инварианты режутся как проза).
    let re = epic_re(ADR_FIELD_PATTERN).expect("regex");
    for form in [
        "Binds: все сервисы — Rust.",
        "- Binds: Оркестратор ↔ Платформа",
        "**Binds**: контур интеграции",
        "- **Binds**: Оркестратор операций ЦР, Реестр кошельков",
        "* **Rule**: остаток живёт на платформе",
        "  - **Prevents**: теневой баланс",
        "**Binds:** все сервисы — Rust 1.85.",
    ] {
        assert!(is_adr_block(&re, form), "форма не узнана: {form}");
    }
    for prose in [
        "Связывает компоненты контура.",
        "Rule-based подход без двоеточия",
        "поля инварианта перечислены ниже",
        "",
    ] {
        assert!(!is_adr_block(&re, prose), "ложное срабатывание: {prose:?}");
    }
    // Заголовки AD/ADR — подстраховка классификации.
    for title in [
        "AD-008. Криптографическая граница",
        "ADR-012 Восстановление",
        "AD-1: Стек",
    ] {
        assert!(is_ad_title(title), "заголовок не узнан: {title}");
    }
    for title in [
        "Проза про AD-008 и его следствия",
        "Изменения по ADR-1",
        "Deferred — отложенные решения",
        "",
    ] {
        assert!(!is_ad_title(title), "ложный заголовок: {title:?}");
    }
}

#[test]
fn epic_context_ladder_preserves_bold_adr_blocks_verbatim() {
    // Дефект D1 живого отчёта: спайн кейса digital-ruble пишет поля
    // инвариантов в жирной markdown-форме `- **Binds**:` — до фикса
    // AD-блоки резались лесенкой как проза, а сноска утверждала
    // дословность. Фикстура: сумма > EPIC_CONTEXT_MAX_CHARS, AD-блоки в
    // хвосте в жирной форме — все обязаны доехать дословно и целиком.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let mut text = String::from("# Спека эпика\n\n");
    for i in 0..120 {
        let _ = write!(
            text,
            "## Прозаическая секция номер {i} с длинным хвостом имени\n\n\
                 Наполнитель PROSE-{i}: длинный абзац прозы про контракты, стыки, \
                 ограничения и детали реализации для объёма контекста.\n\n"
        );
    }
    for i in 8..11 {
        let _ = write!(
            text,
            "## AD-{i:03}. Инвариант контура\n\n\
                 - **Binds**: MARKER-BINDS-{i} — дословное правило стыковки компонентов\n\
                 - **Prevents**: MARKER-PREVENTS-{i} — запрещённый класс отказов\n\
                 - **Rule**: MARKER-RULE-{i} — финальная строка правила, обязана доехать целиком\n\
                 - **Status**: [ADOPTED]\n\n"
        );
    }
    let spec = tmp.path().join("spec-bold-ladder.md");
    write_file(&spec, &text);

    let packet =
        generate_handoff(&repo, "задача", &[spec], &cfg, None, Route::Standard).expect("handoff");
    let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
    // Все AD-блоки присутствуют дословно, целиком — до последней строки.
    for i in 8..11 {
        for marker in [
            format!("- **Binds**: MARKER-BINDS-{i}"),
            format!("- **Prevents**: MARKER-PREVENTS-{i}"),
            format!(
                "- **Rule**: MARKER-RULE-{i} — финальная строка правила, обязана доехать целиком"
            ),
            "- **Status**: [ADOPTED]".to_string(),
        ] {
            assert!(
                arch.contains(&marker),
                "AD-{i:03} обрезан ({marker}):\n{arch}"
            );
        }
    }
    // Сноска честная: маркер усечения есть, утверждение дословности
    // совпадает с фактом (все AD-секции с телами).
    assert!(arch.contains("Контекст усечён"), "{arch}");
    assert!(arch.contains("дословно"), "{arch}");
    let ad_count = assert_notice_matches_fact(&arch);
    assert_eq!(ad_count, 3, "все три AD-заголовка в выходе:\n{arch}");
    // AD-секции не названы «прозаическими» в перечне сокращённых.
    assert!(
        !arch.contains("сокращены до заголовков: AD-"),
        "AD попали в перечень сокращённой прозы:\n{arch}"
    );
}

#[test]
fn epic_context_keeps_all_ten_ad_bodies_from_report_case() {
    // Регрессия по форме кейса digital-ruble из отчёта: спайн — преамбула
    // и 10 инвариантов `## AD-0XX` в форме `- **Binds**:`; в пакете
    // обязано быть 10 заголовков AD — и у всех 10 непустые тела (в отчёте
    // у AD-008/009/010 тела были вырезаны до заголовков). Лимит символов
    // здесь осознанно не проверяется: документ из одних инвариантов
    // уходит за лимит дословным (зафиксированный компромисс лесенки).
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let mut text = String::from(
        "# ARCHITECTURE-SPINE. Сервис примера\n\n\
             Преамбула спайна: регуляторный контур, статусы решений, граница честности. \
             Вторая строка преамбулы для объёма и проверки лесенки на прозе.\n\n",
    );
    for i in 1..=10 {
        let _ = write!(
            text,
            "## AD-{i:03}. Инвариант контура номер {i}\n\n\
                 - **Binds**: Компонент-A-{i}, Компонент-B-{i} ↔ Внешняя платформа\n\
                 - **Prevents**: класс отказов {i}: теневой баланс, двойное списание, потерю следа\n\
                 - **Rule**: правило инварианта {i} исполняется дословно: записи append-only, \
                 корректировка — компенсирующей записью со ссылкой на исходную; журнал — \
                 единственный источник аудита и аргументов в спорах. Любое решение об исходе \
                 операции принимается по ответу или выписке платформы, но не по локальной \
                 записи; проекция, устаревшая сверх лага сверки, помечается несвежей, \
                 а операции по ней не исполняются. MARKER-RULE-TAIL-{i}\n\
                 - **Status**: [ADOPTED] 2026-09-19\n\n"
        );
    }
    assert!(
        text.chars().count() > EPIC_CONTEXT_MAX_CHARS,
        "фикстура обязана превышать лимит: {}",
        text.chars().count()
    );
    let spec = tmp.path().join("spine.md");
    write_file(&spec, &text);

    let packet =
        generate_handoff(&repo, "задача", &[spec], &cfg, None, Route::Standard).expect("handoff");
    let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
    // 10 заголовков → 10 непустых тел, каждый — с финальным маркером.
    for i in 1..=10 {
        assert!(
            arch.contains(&format!("## AD-{i:03}. Инвариант контура номер {i}")),
            "заголовок AD-{i:03} потерян:\n{arch}"
        );
        assert!(
            arch.contains(&format!("MARKER-RULE-TAIL-{i}")),
            "тело AD-{i:03} урезано:\n{arch}"
        );
    }
    let ad_count = assert_notice_matches_fact(&arch);
    assert_eq!(ad_count, 10, "10 заголовков → 10 тел:\n{arch}");
    // Сноска (усечение преамбулы) не врёт про инварианты.
    assert!(arch.contains("Контекст усечён"), "{arch}");
    assert!(arch.contains("дословно"), "{arch}");
    assert!(
        !arch.contains("НЕ гарантируется"),
        "все AD с телами — дословность подтверждена:\n{arch}"
    );
}

#[test]
fn truncation_notice_never_claims_verbatim_for_cut_ad_sections() {
    // Охранный тест честности сноски (D1): если AD/ADR-секция по факту
    // без тела (урезана лесенкой или выкинута), сноска НЕ пишет «дословно
    // и не сокращались» — утверждение не расходится с фактом.
    let section = |title: &str, body: &str, shortened: bool| EpicSection {
        title: title.to_string(),
        heading: format!("## {title}"),
        body: body.to_string(),
        is_adr: !body.is_empty(),
        shortened_to_heading: shortened,
    };
    let render = EpicRender {
        header: String::new(),
        sections: vec![
            section("AD-008. Криптографическая граница", "", true),
            section("AD-009. Антифрод", "- **Binds**: x", false),
        ],
    };
    let notice = truncation_notice(&render, &["ADR-012. Выкинутый инвариант".to_string()]);
    assert!(
        !notice.contains("дословно и не сокращались"),
        "сноска врёт про дословность:\n{notice}"
    );
    assert!(notice.contains("НЕ гарантируется"), "{notice}");
    assert!(notice.contains("AD-008"), "{notice}");
    assert!(notice.contains("ADR-012"), "{notice}");

    // Все AD-секции с телами — дословность декларируется законно.
    let render_ok = EpicRender {
        header: String::new(),
        sections: vec![section("AD-009. Антифрод", "- **Binds**: x", false)],
    };
    let notice_ok = truncation_notice(&render_ok, &[]);
    assert!(
        notice_ok.contains("дословно и не сокращались"),
        "{notice_ok}"
    );
    assert!(!notice_ok.contains("НЕ гарантируется"), "{notice_ok}");
}

#[test]
fn epic_context_fits_keeps_everything_without_notice() {
    // Случай «влезает без усечения» не меняется: ни сноски, ни потерь.
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    let cfg = cfg_in(tmp.path());
    let spec = tmp.path().join("spec.md");
    write_file(
        &spec,
        "# Спека\n\n## AD-1: Стек\n\n**Binds:** точный текст инварианта.\n\n## Детали\n\nабзац\n",
    );
    let packet =
        generate_handoff(&repo, "задача", &[spec], &cfg, None, Route::Standard).expect("handoff");
    let arch = std::fs::read_to_string(packet.dir.join("ARCHITECTURE.md")).expect("arch");
    assert!(!arch.contains("Контекст усечён"), "{arch}");
    assert!(arch.contains("точный текст инварианта"), "{arch}");
    assert!(arch.contains("абзац"), "{arch}");
}

/// Модель из `n` REQ-сущностей в `<repo>/model/` (для проверки
/// предупреждения о декомпозиции REQ → задачи).
fn repo_with_req_model(repo: &Path, n: usize) {
    for i in 1..=n {
        write_file(
            &repo.join(format!("model/REQ-{i:03}.md")),
            &format!(
                "---\nid: REQ-{i:03}\ntype: req\ntitle: Требование {i}\nstatus: accepted\n---\n"
            ),
        );
    }
}

#[test]
fn handoff_warns_on_thin_req_task_decomposition() {
    // B3: REQ существенно больше задач в TASK.md — детерминированное
    // предупреждение в пакете (исполнитель получал TASK.md, чей список
    // задач недопокрывал REQ-множество).
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = cfg_in(tmp.path());

    // 4 REQ против задачи без пунктов: 4 > 2×0 и 4 >= 3 — предупреждение.
    let repo = tmp.path().join("repo-thin");
    std::fs::create_dir_all(&repo).expect("mkdir");
    repo_with_req_model(&repo, 4);
    let packet =
        generate_handoff(&repo, "сделать фичу", &[], &cfg, None, Route::Fast).expect("handoff");
    assert_eq!(packet.warnings.len(), 1, "{:?}", packet.warnings);
    assert!(
        packet.warnings[0].contains("4 REQ-сущностей"),
        "{:?}",
        packet.warnings
    );
    assert!(
        packet.warnings[0].contains("декомпозиция"),
        "{:?}",
        packet.warnings
    );

    // Достаточная декомпозиция: 4 REQ против 2 пунктов — 4 > 2×2 ложно.
    let repo2 = tmp.path().join("repo-ok");
    std::fs::create_dir_all(&repo2).expect("mkdir");
    repo_with_req_model(&repo2, 4);
    let packet2 = generate_handoff(
        &repo2,
        "сделать фичу:\n\n- задача раз\n\n- задача два",
        &[],
        &cfg,
        None,
        Route::Fast,
    )
    .expect("handoff 2");
    assert!(packet2.warnings.is_empty(), "{:?}", packet2.warnings);

    // Мелкий эпик: 2 REQ — ниже минимума проверки, предупреждения нет.
    let repo3 = tmp.path().join("repo-small");
    std::fs::create_dir_all(&repo3).expect("mkdir");
    repo_with_req_model(&repo3, 2);
    let packet3 =
        generate_handoff(&repo3, "сделать фичу", &[], &cfg, None, Route::Fast).expect("handoff 3");
    assert!(packet3.warnings.is_empty(), "{:?}", packet3.warnings);

    // Нет model/ — проверка не включается.
    let repo4 = tmp.path().join("repo-nomodel");
    std::fs::create_dir_all(&repo4).expect("mkdir");
    let packet4 =
        generate_handoff(&repo4, "сделать фичу", &[], &cfg, None, Route::Fast).expect("handoff 4");
    assert!(packet4.warnings.is_empty(), "{:?}", packet4.warnings);
}

#[tokio::test]
async fn handoff_create_tool_prints_warnings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    repo_with_req_model(&repo, 5);
    let cfg = cfg_in(tmp.path());
    let tool = HandoffCreateTool::new(cfg.clone());
    let ctx = ToolContext::new(tmp.path().to_path_buf(), Arc::new(cfg));
    let out = tool
        .call(json!({"repo": "repo", "task": "сделать фичу"}), &ctx)
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content.contains("ВНИМАНИЕ: в модели 5 REQ-сущностей"),
        "{}",
        out.content
    );
}

#[test]
fn spec_md_carries_machine_banner() {
    // B3: сгенерированный SPEC.md первой строкой несёт баннер машинной
    // компиляции — и в ветке шаблона, и в ветке сборки из спек (исполнитель
    // принимал компиляцию за авторскую спеку архитектора).
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = cfg_in(tmp.path());

    // Ветка шаблона (без спек).
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir");
    let packet = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let spec = std::fs::read_to_string(packet.dir.join("SPEC.md")).expect("SPEC.md");
    assert!(spec.starts_with(SPEC_MACHINE_BANNER), "{spec}");

    // Ветка сборки из переданных спек.
    let repo2 = tmp.path().join("repo2");
    std::fs::create_dir_all(&repo2).expect("mkdir");
    let spec_src = tmp.path().join("spec-src.md");
    write_file(&spec_src, "# Контракты\n\n- идемпотентность по id\n");
    let packet2 = generate_handoff(
        &repo2,
        "задача",
        std::slice::from_ref(&spec_src),
        &cfg,
        None,
        Route::Fast,
    )
    .expect("handoff 2");
    let spec2 = std::fs::read_to_string(packet2.dir.join("SPEC.md")).expect("SPEC.md 2");
    assert!(spec2.starts_with(SPEC_MACHINE_BANNER), "{spec2}");
}

/// Н13 (T-02): схема инструмента объявляет ровно то, что разбирает
/// реализация. Раньше `handoff_create` рекламировал свойство `path`, а
/// требовал `repo` — клиент, читающий `tools/list`, получал отказ по
/// несуществующему аргументу.
#[test]
fn handoff_tool_schema_advertises_what_it_parses() {
    let cfg = Config::default();
    let tools = tools(&cfg);
    let spec = tools[0].spec();
    let props = spec.parameters["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for arg in [
        "path",
        "task",
        "spec",
        "rollback",
        "route",
        "refresh_constraints",
    ] {
        assert!(props.contains(&arg.to_string()), "нет '{arg}' в {props:?}");
    }
    // Обязательные аргументы обязаны быть объявлены среди свойств.
    for req in spec.parameters["required"].as_array().expect("required") {
        let req = req.as_str().expect("строка");
        assert!(
            props.contains(&req.to_string()),
            "обязательный '{req}', но его нет в свойствах: {props:?}"
        );
    }
    assert_eq!(spec.parameters["required"], serde_json::json!(["task"]));
}

/// T-02: при существующем КОРНЕВОМ реестре пакетный — его КОПИЯ, а не
/// заготовка под стек. Пакетная копия приоритетна для резолвера гейта,
/// поэтому заготовка из одного правила молча переключала гейт на себя:
/// «Правил: 1, PASS» вместо прогона правил проекта.
#[test]
fn packet_registry_is_a_copy_of_the_root_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    let root = "rules:\n  - id: C-001\n    name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - id: C-002\n    name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n";
    write_file(&repo.join("CONSTRAINTS.yaml"), root);
    write_file(&repo.join("README.md"), "# Проект\n");
    let cfg = cfg_in(tmp.path());

    let packet = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let packet_registry = repo.join(".arch-handoff/CONSTRAINTS.yaml");
    assert_eq!(
        std::fs::read_to_string(&packet_registry).expect("реестр пакета"),
        root,
        "пакетный реестр обязан быть копией корневого"
    );
    assert_eq!(
        packet.constraints_action.as_deref(),
        Some("копия корневого CONSTRAINTS.yaml")
    );
    // Гейт читает именно пакетную копию — значит правил столько же.
    let resolved = crate::control::resolve_constraints_path(&repo, None).expect("резолв");
    assert_eq!(resolved, packet_registry);
    assert_eq!(
        crate::control::load_constraints_resolved(&resolved)
            .expect("реестр читается")
            .rules
            .len(),
        2
    );

    // Повторная генерация не затирает правки архитектора; явный флаг —
    // затирает (T-02: «существующую копию не перезаписывает без флага»).
    write_file(&packet_registry, "# правка архитектора\n");
    let again =
        generate_handoff(&repo, "другая", &[], &cfg, None, Route::Fast).expect("повторный handoff");
    assert_eq!(
        std::fs::read_to_string(&packet_registry).expect("реестр после"),
        "# правка архитектора\n"
    );
    assert!(again.constraints_action.is_none());
    let refreshed = generate_handoff_opts(
        &repo,
        "третья",
        &[],
        &cfg,
        None,
        Route::Fast,
        HandoffOptions {
            refresh_constraints: true,
        },
    )
    .expect("refresh");
    assert_eq!(
        std::fs::read_to_string(&packet_registry).expect("реестр после refresh"),
        root
    );
    assert_eq!(
        refreshed.constraints_action.as_deref(),
        Some("копия корневого CONSTRAINTS.yaml")
    );
}

/// Без корневого реестра пакетный — по-прежнему заготовка под стек: копировать
/// нечего, и об этом сказано в результате, а не умолчано.
#[test]
fn packet_registry_stays_a_stack_stub_without_root_one() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
    let cfg = cfg_in(tmp.path());
    let packet = generate_handoff(&repo, "задача", &[], &cfg, None, Route::Fast).expect("handoff");
    let action = packet.constraints_action.as_deref().expect("действие");
    assert!(action.contains("заготовка"), "{action}");
    assert!(
        std::fs::read_to_string(repo.join(".arch-handoff/CONSTRAINTS.yaml"))
            .expect("реестр")
            .contains("cargo")
            || action.contains("корневого реестра"),
        "{action}"
    );
}

/// `handoff_create` пишет `control_plane` с ПОЛНЫМ обязательным минимумом:
/// существующие файлы — хэшем (совпадающим с содержимым), отсутствующие —
/// `null`; опциональные — по факту наличия; журнал вызовов не пинится.
#[test]
fn manifest_pins_control_plane_minimum() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("mkdir repo");
    write_file(&repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n");
    // Опциональный файл присутствует — обязан попасть в пины.
    write_file(&repo.join(".gitlab-ci.yml"), "stages: [test]\n");
    // Журнал вызовов — не конфиг: в пины не входит.
    write_file(&repo.join(".arch-handoff/mcp-calls.jsonl"), "{}\n");
    let cfg = cfg_in(tmp.path());

    generate_handoff(&repo, "задача", &[], &cfg, None, Route::Standard).expect("handoff");
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(repo.join(".arch-handoff/MANIFEST.json")).expect("manifest"),
    )
    .expect("manifest json");
    let pins = manifest["control_plane"]
        .as_object()
        .expect("поле control_plane");
    // Обязательный минимум присутствует целиком.
    for rel in crate::control_plane::REQUIRED {
        assert!(pins.contains_key(rel), "нет обязательного пина {rel}");
    }
    // Существующие файлы пакета запинены хэшем их содержимого.
    for rel in [
        ".arch-handoff/TASK.md",
        ".arch-handoff/CONSTRAINTS.yaml",
        ".arch-handoff/SPEC.md",
        ".arch-handoff/ROLLBACK.yaml",
    ] {
        let expected = crate::hash::sha256_file(&repo.join(rel)).expect("файл пакета");
        assert_eq!(
            pins[rel]["sha256"].as_str(),
            Some(expected.as_str()),
            "пин {rel} не совпал с содержимым"
        );
    }
    // Отсутствующие на момент выдачи — пин отсутствия (`null`).
    for rel in [".claude/settings.json", "arch-harness.toml"] {
        assert!(pins[rel].is_null(), "{rel} должен быть запинен как null");
    }
    // Опциональный по факту наличия — с хэшем; журнал не пинится.
    let expected_ci =
        crate::hash::sha256_file(&repo.join(".gitlab-ci.yml")).expect(".gitlab-ci.yml");
    assert_eq!(
        pins[".gitlab-ci.yml"]["sha256"].as_str(),
        Some(expected_ci.as_str())
    );
    assert!(!pins.contains_key(".arch-handoff/mcp-calls.jsonl"));
}

//! Внешний корпус мутаций (E3, исследование — за флагом `--corpus`):
//! патчи из журналов прогонов харнессов как мутаторы.
//!
//! Замысел: каталог [`MUTATORS`](super::MUTATORS) пишется рукой по классам
//! дефектов, а у команды уже лежат ЖУРНАЛЫ реальных прогонов агентов —
//! из них можно собрать корпус «что агенты реально ломают» в виде патчей
//! и измерить, сколько из них ловит контур конкретного кейса.
//!
//! Минимальный формат корпуса: файлы `*.patch` / `*.diff` (unified diff,
//! git-style с префиксами `a/`/`b/` или без) в каталоге корпуса на любой
//! глубине. Патч применяется к копии кейса как мутатор (`git apply`),
//! дальше прогон идёт по обычному конвейеру redteam (эталон → правка →
//! гейт Critical → карта обнаружения).
//!
//! Честные границы прототипа:
//! - патч, который не применился (`git apply --check` отказал на обоих
//!   уровнях `-p1`/`-p0`), — это честный пропуск с причиной, а не «пойман»
//!   и не «не пойман»: неприменимый патч ничего не измеряет;
//! - ожидание у корпусной позиции всегда «ловится»: предпосылка корпуса —
//!   патчи суть реальные нарушения агентов. Патч-исправление в корпусе
//!   даст строку «пойман, а не должен» — корпус обязан быть вычищен до
//!   замера (это свойство данных, а не инструмента);
//! - позиции корпуса — отдельные строки вне знаменателя доли обнаружения
//!   (как D15/D18–D24): они меряют корпус против кейса, а не набор
//!   раздела 7;
//! - прогон информационный: exit-код не зависит от доли поимки корпуса
//!   (порог — решение архитектора после калибровки, не зашит), несовместим
//!   с `--save` (измерение корпуса не является измерением защищённости
//!   кейса для метрики доверия).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Detection, Expectation, Fixture, Layer, RedteamOptions, RedteamReport};
use crate::control::Route;
use crate::error::{HarnessError, Result};
use crate::gate::GateStatus;

/// Один патч корпуса: файл unified diff и разобранные из него метаданные.
#[derive(Debug, Clone)]
pub struct CorpusPatch {
    /// Идентификатор позиции (`C01`, `C02`, … — по отсортированному порядку
    /// относительных путей: прогон детерминирован).
    pub id: String,
    /// Относительный путь патча внутри каталога корпуса (для отчёта).
    pub rel: String,
    /// Абсолютный путь к файлу патча.
    pub path: PathBuf,
    /// Файлы кейса, затрагиваемые патчем (из заголовков `---`/`+++`,
    /// `/dev/null` исключён; детерминированный порядок).
    pub files: Vec<String>,
    /// Слой дефекта по расширениям затрагиваемых файлов (хотя бы один
    /// кодовый файл — слой «код»).
    pub layer: Layer,
}

/// Итог загрузки корпуса: разобранные патчи и честно отброшенные файлы.
#[derive(Debug, Clone, Default)]
pub struct CorpusLoad {
    /// Патчи в детерминированном порядке (по относительному пути).
    pub patches: Vec<CorpusPatch>,
    /// Отброшенные при загрузке файлы: (относительный путь, причина).
    /// Попадают в карту прогона строками «пропущен», а не теряются.
    pub rejected: Vec<(String, String)>,
}

/// Сводка корпусного прогона для шапки отчёта (E3): отдельно от доли
/// обнаружения — корпус меряется, а не принимается.
#[derive(Debug, Clone)]
pub struct CorpusRunInfo {
    /// Каталог корпуса.
    pub dir: PathBuf,
    /// Файлов `*.patch`/`*.diff` найдено (до разбора).
    pub found: usize,
    /// Применилось к копии кейса (по ним есть вердикт гейта).
    pub applied: usize,
    /// Из применившихся поймано гейтом.
    pub caught: usize,
    /// Не применилось (`git apply` отказал) — неприменимы к кейсу.
    pub not_applicable: usize,
    /// Отброшено при загрузке (не unified diff).
    pub rejected: usize,
}

/// Расширения кодовых файлов для классификации слоя патча: языки извлечения
/// импортов (волна K) плюс SQL-миграции.
const CODE_EXTENSIONS: [&str; 12] = [
    "rs", "py", "java", "kt", "ts", "tsx", "js", "jsx", "mjs", "go", "sql", "c",
];

/// Слой патча по затрагиваемым файлам: кодовое расширение хотя бы у одного
/// файла — слой «код» (доли E2 по слоям читаются раздельно).
fn layer_of_files(files: &[String]) -> Layer {
    let code = files.iter().any(|f| {
        Path::new(f)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| CODE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
    });
    if code { Layer::Code } else { Layer::DocsModel }
}

/// Затрагиваемые файлы из заголовков unified diff: цели `+++` и источники
/// `---` (у чистого удаления файла цели нет — файл виден только в `---`),
/// без `/dev/null`, со срезом префикса `a/`/`b/` (git-style) и кавычек
/// квотированной формы git (кириллические пути git печатает как
/// `"a/model/CMP-001-\320….md"`). Порядок — появление в патче, дубли
/// убраны (детерминизм).
fn touched_files(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let raw = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "));
        let Some(raw) = raw else { continue };
        // Хвост после пути (табуляция с датой у GNU diff) отсекаем.
        let path = raw.split('\t').next().unwrap_or(raw).trim();
        if path == "/dev/null" || path.is_empty() {
            continue;
        }
        // Квотированная форма git: внешние кавычки снимаем ДО среза префикса
        // (`"a/model/…"` иначе не начинается с `a/`). Восьмеричные escapes
        // внутри не разворачиваем — для классификации слоя и строки отчёта
        // достаточно расширения.
        let path = path
            .strip_prefix('"')
            .and_then(|p| p.strip_suffix('"'))
            .unwrap_or(path);
        let path = path
            .strip_prefix("a/")
            .or_else(|| path.strip_prefix("b/"))
            .unwrap_or(path);
        if !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

/// Похож ли файл на unified diff: есть заголовок `+++` (минимальный признак;
/// полную проверку честно делает `git apply --check` при прогоне).
fn looks_like_unified_diff(text: &str) -> bool {
    text.lines().any(|l| l.starts_with("+++ "))
}

/// Загружает корпус патчей из каталога: рекурсивный обход, `*.patch` и
/// `*.diff`, детерминированный порядок (сортировка по относительному пути).
///
/// Файлы с нужным расширением, но без признаков unified diff, честно
/// отбрасываются с причиной (попадут в карту прогона строками «пропущен»).
///
/// # Errors
/// Каталог корпуса недоступен или не читается.
pub fn load_corpus(dir: &Path) -> Result<CorpusLoad> {
    if !dir.is_dir() {
        return Err(HarnessError::Control(format!(
            "каталог корпуса недоступен: {}",
            dir.display()
        )));
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let walker = walkdir::WalkDir::new(dir).follow_links(false);
    for entry in walker {
        let entry = entry
            .map_err(|e| HarnessError::Control(format!("корпус {}: обход: {e}", dir.display())))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let is_patch = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "patch" | "diff"));
        if is_patch {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    let mut load = CorpusLoad::default();
    for path in &files {
        let rel = path
            .strip_prefix(dir)
            .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
        let text = std::fs::read_to_string(path).map_err(|e| HarnessError::io(path, e))?;
        if !looks_like_unified_diff(&text) {
            load.rejected
                .push((rel, "нет заголовков unified diff".to_string()));
            continue;
        }
        let touched = touched_files(&text);
        // Идентификатор — по числу ПРИНЯТЫХ патчей (отброшенные номера не
        // занимают: карта компактна); порядок детерминирован сортировкой.
        let id = format!("C{:02}", load.patches.len() + 1);
        load.patches.push(CorpusPatch {
            id,
            rel,
            path: path.clone(),
            layer: layer_of_files(&touched),
            files: touched,
        });
    }
    Ok(load)
}

/// Один прогон `git apply` над мутантом: `check_only` — проба без записи.
/// Патч — внешний файл (корпус живёт отдельно от копии кейса).
fn git_apply(
    root: &Path,
    patch: &Path,
    strip: &str,
    check_only: bool,
) -> std::result::Result<(), String> {
    let mut args: Vec<&str> = vec!["apply", "--whitespace=nowarn", strip];
    if check_only {
        args.push("--check");
    }
    let patch_str = patch.display().to_string();
    args.push(patch_str.as_str());
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git apply не запустился: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("git apply завершился с ошибкой без сообщения");
    Err(reason.chars().take(200).collect())
}

/// Применяет патч корпуса к мутанту: `git apply` с префиксом `-p1`
/// (git-style `a/`/`b/`), при отказе — `-p0` (голые пути GNU diff).
///
/// # Errors
/// `Err(причина)` — патч к копии кейса неприменим (контекст не сошёлся,
/// целевого файла нет, битый diff). Это честный пропуск позиции, а не
/// результат «пойман/не пойман».
fn apply_patch(root: &Path, patch: &Path) -> std::result::Result<(), String> {
    let mut reasons = Vec::new();
    for strip in ["-p1", "-p0"] {
        match git_apply(root, patch, strip, true) {
            Ok(()) => return git_apply(root, patch, strip, false),
            Err(reason) => reasons.push(format!("{strip}: {reason}")),
        }
    }
    Err(reasons.join("; "))
}

/// Корпусный прогон (E3): каждый патч каталога — мутатор копии кейса.
///
/// Конвейер позиции совпадает со стандартным прогоном [`super::run`]:
/// свежая копия → git-репозиторий мутанта → применение патча →
/// переупаковка бандла → коммит → гейт на маршруте Critical (база
/// `HEAD~1`). Эталонный прогон без правок обязателен и требует зелёного
/// кейса: иначе карта мерила бы сломанный пакет.
///
/// # Errors
/// Кейс или каталог корпуса недоступны, кейс не зелёный на эталоне,
/// git или гейт отказали.
pub fn run_corpus(
    case: &Path,
    corpus_dir: &Path,
    options: &RedteamOptions,
) -> Result<RedteamReport> {
    let load = load_corpus(corpus_dir)?;
    if load.patches.is_empty() && load.rejected.is_empty() {
        return Err(HarnessError::Control(format!(
            "в корпусе {} нет файлов *.patch/*.diff — соберите патчи из журналов прогонов \
             (unified diff) и повторите",
            corpus_dir.display()
        )));
    }
    let fixture = Fixture::new(case)?;
    let reference_root = fixture.mutant("reference")?;
    super::prepare(&reference_root).map_err(HarnessError::Control)?;
    // Второй коммит обязателен (база прогона — HEAD~1), как в стандартном
    // прогоне.
    let _ = crate::evidence::pack(&reference_root, Route::Critical);
    if let Err(e) = super::git(&reference_root, &["add", "-A"])
        .and_then(|()| super::git(&reference_root, &["commit", "-q", "-m", "reference"]))
    {
        return Err(HarnessError::Control(format!("эталон: коммит: {e}")));
    }
    let reference = super::gate_report(&reference_root, options.decision_quality)?;
    if reference.outcome != crate::gate::GateOutcome::Pass {
        return Err(HarnessError::Control(format!(
            "кейс {} не зелёный на маршруте Critical — корпусный прогон мерил бы \
             сломанный пакет ({})",
            case.display(),
            super::not_green_reasons(&reference)
        )));
    }

    let mut detections: Vec<Detection> = Vec::new();
    // Отброшенные при загрузке — честные строки «пропущен» с причиной.
    for (rel, reason) in &load.rejected {
        detections.push(Detection {
            id: "—".to_string(),
            title: format!("корпус: {rel}"),
            expected: Expectation::Caught,
            layer: Layer::DocsModel,
            caught_by: None,
            skipped: Some(reason.clone()),
            expected_by: "чтение патча".to_string(),
            in_ratio: false,
        });
    }
    let mut applied = 0usize;
    let mut caught = 0usize;
    for patch in &load.patches {
        let root = fixture.mutant(&patch.id)?;
        if let Err(e) = super::prepare(&root) {
            return Err(HarnessError::Control(format!(
                "{}: подготовка мутанта: {e}",
                patch.id
            )));
        }
        if let Err(reason) = apply_patch(&root, &patch.path) {
            detections.push(Detection {
                id: patch.id.clone(),
                title: format!("корпус: {}", patch.rel),
                expected: Expectation::Caught,
                layer: patch.layer,
                caught_by: None,
                skipped: Some(format!("патч не применился: {reason}")),
                expected_by: "контур контроля (fitness/гейт)".to_string(),
                in_ratio: false,
            });
            continue;
        }
        applied += 1;
        // Бандл переупаковывается ПОСЛЕ правки — как в стандартном прогоне:
        // иначе дефект ловился бы evidence_verify, а не проверяемым контуром.
        let _ = crate::evidence::pack(&root, Route::Critical);
        if let Err(e) = super::git(&root, &["add", "-A"]).and_then(|()| {
            super::git(
                &root,
                &["commit", "-q", "-m", &format!("corpus {}", patch.id)],
            )
        }) {
            return Err(HarnessError::Control(format!("{}: коммит: {e}", patch.id)));
        }
        let report = super::gate_report(&root, options.decision_quality)?;
        let failed: Vec<String> = report
            .components
            .iter()
            .filter(|c| c.status == GateStatus::Fail)
            .map(|c| c.name.to_string())
            .collect();
        if !failed.is_empty() {
            caught += 1;
        }
        detections.push(Detection {
            id: patch.id.clone(),
            title: format!("корпус: {}", patch.rel),
            expected: Expectation::Caught,
            layer: patch.layer,
            caught_by: if failed.is_empty() {
                None
            } else {
                Some(failed.join(", "))
            },
            skipped: None,
            expected_by: "контур контроля (fitness/гейт)".to_string(),
            in_ratio: false,
        });
    }
    let not_applicable = load.patches.len() - applied;
    Ok(RedteamReport {
        case: case.to_path_buf(),
        detections,
        min_detection: options.min_detection,
        min_code_detection: options.min_code_detection,
        control_ok: true,
        control_note: None,
        semantic_kept: Vec::new(),
        corpus: Some(CorpusRunInfo {
            dir: corpus_dir.to_path_buf(),
            found: load.patches.len() + load.rejected.len(),
            applied,
            caught,
            not_applicable,
            rejected: load.rejected.len(),
        }),
    })
}

/// Шапка корпусного прогона для текстового отчёта (E3): сколько патчей
/// найдено, сколько применилось и сколько из применившихся поймано гейтом.
/// Доля обнаружения набора раздела 7 корпусом не двигается — строки корпуса
/// вне знаменателя.
#[must_use]
pub fn render_summary(info: &CorpusRunInfo) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    // Запись в String не может завершиться ошибкой — игнор безопасен.
    let _ = writeln!(
        out,
        "Корпус (E3, эксперимент): {} · найдено патчей: {} · применилось: {} · \
         не применилось: {} · отброшено при загрузке: {}",
        info.dir.display(),
        info.found,
        info.applied,
        info.not_applicable,
        info.rejected
    );
    if info.applied > 0 {
        let _ = writeln!(
            out,
            "Из применившихся поймано гейтом: {}/{} ({:.0}%) — измерение корпуса, \
             не приёмка: порог не задан ([РЕШЕНИЕ ЧЕЛОВЕКА])",
            info.caught,
            info.applied,
            info.caught as f64 / info.applied as f64 * 100.0
        );
    } else {
        let _ = writeln!(
            out,
            "Применимых патчей нет — измерять нечего (граница применимости, \
             см. docs/experiments/redteam-corpus.md)"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пишет файл фикстуры (родители создаются).
    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, text).expect("write");
    }

    /// git в каталоге с тестовой идентичностью (изоляция AD-7).
    fn git_in(dir: &Path, args: &[&str]) {
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

    /// Патч, добавляющий кодовый файл (git-style, префиксы a//b).
    const ADD_RS_PATCH: &str = "diff --git a/skeleton/dirty.rs b/skeleton/dirty.rs\n\
         new file mode 100644\n\
         --- /dev/null\n\
         +++ b/skeleton/dirty.rs\n\
         @@ -0,0 +1,2 @@\n\
         +pub fn total(parts: &[f64]) -> f64 { parts.iter().sum() }\n\
         +pub const KEY: &str = \"x\";\n";

    /// Загрузка: рекурсивный обход, только *.patch/*.diff, порядок по
    /// относительному пути, не-diff файлы честно отброшены.
    #[test]
    fn load_corpus_collects_sorted_and_rejects_non_diffs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path();
        write(dir, "b-run/two.patch", ADD_RS_PATCH);
        write(dir, "a-run/one.diff", ADD_RS_PATCH);
        write(dir, "a-run/notes.patch", "просто журнал, не патч\n");
        write(dir, "a-run/readme.txt", "не патч и не по расширению\n");
        let load = load_corpus(dir).expect("загрузка");
        let rels: Vec<&str> = load.patches.iter().map(|p| p.rel.as_str()).collect();
        assert_eq!(rels, vec!["a-run/one.diff", "b-run/two.patch"], "{rels:?}");
        assert_eq!(load.patches[0].id, "C01");
        assert_eq!(load.patches[1].id, "C02");
        assert_eq!(load.rejected.len(), 1);
        assert_eq!(load.rejected[0].0, "a-run/notes.patch");
        assert!(load.rejected[0].1.contains("unified diff"));
    }

    /// Затрагиваемые файлы: цели `+++`, источники `---` у удалений,
    /// `/dev/null` исключён, префиксы `a/`/`b/` срезаны, дублей нет.
    #[test]
    fn touched_files_parse_git_and_plain_headers() {
        let text = "--- a/src/old.py\n\
                    --- /dev/null\n\
                    +++ b/src/new.py\n\
                    +++ b/src/new.py\n\
                    --- plain/go.mod\n";
        let files = touched_files(text);
        assert_eq!(files, vec!["src/old.py", "src/new.py", "plain/go.mod"]);
    }

    /// Квотированная форма git (кириллические пути печатаются с
    /// восьмеричными escapes): внешние кавычки снимаются, расширение файла
    /// остаётся видимым для классификации слоя.
    #[test]
    fn touched_files_unquote_git_quoted_paths() {
        let text = "--- \"a/model/CMP-001-\\320\\272.md\"\n+++ \"b/model/CMP-001-\\320\\272.md\"\n";
        let files = touched_files(text);
        assert_eq!(files.len(), 1);
        assert!(
            files[0]
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("md")),
            "{files:?}"
        );
        assert_eq!(layer_of_files(&files), Layer::DocsModel);
    }

    /// Слой патча — по расширениям затрагиваемых файлов: кодовое расширение
    /// хотя бы у одного файла переводит позицию в кодовый слой (E2).
    #[test]
    fn layer_follows_touched_files() {
        assert_eq!(
            layer_of_files(&["skeleton/api.py".to_string(), "README.md".to_string()]),
            Layer::Code
        );
        assert_eq!(
            layer_of_files(&["model/CMP-001.md".to_string()]),
            Layer::DocsModel
        );
    }

    /// Применение: валидный git-style патч кладёт файл в мутант (p1), голый
    /// diff без префиксов — тоже (откат на p0).
    #[test]
    fn apply_patch_handles_git_style_and_plain_paths() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("case");
        std::fs::create_dir_all(&root).expect("mkdir");
        git_in(&root, &["init", "-q"]);
        write(&root, "base.txt", "строка\n");
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "base"]);

        let patch = tmp.path().join("add.patch");
        std::fs::write(&patch, ADD_RS_PATCH).expect("patch");
        apply_patch(&root, &patch).expect("git-style патч применился");
        assert!(root.join("skeleton/dirty.rs").is_file());

        // Голый diff (без a//b) — откат на p0.
        let plain = tmp.path().join("plain.diff");
        std::fs::write(
            &plain,
            "--- base.txt\n+++ base.txt\n@@ -1 +1,2 @@\n строка\n+ещё строка\n",
        )
        .expect("plain");
        apply_patch(&root, &plain).expect("голый патч применился (p0)");
        let text = std::fs::read_to_string(root.join("base.txt")).expect("base.txt");
        assert!(text.contains("ещё строка"), "{text}");
    }

    /// Неприменимый патч (контекст не сошёлся: целевого файла нет) — честная
    /// ошибка с причиной, а не молчаливый успех.
    #[test]
    fn apply_patch_reports_inapplicable() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().join("case");
        std::fs::create_dir_all(&root).expect("mkdir");
        git_in(&root, &["init", "-q"]);
        write(&root, "base.txt", "строка\n");
        git_in(&root, &["add", "-A"]);
        git_in(&root, &["commit", "-q", "-m", "base"]);

        let patch = tmp.path().join("bad.patch");
        std::fs::write(
            &patch,
            "--- a/src/missing.py\n+++ b/src/missing.py\n@@ -1 +1 @@\n-old\n+new\n",
        )
        .expect("patch");
        let reason = apply_patch(&root, &patch).expect_err("патч неприменим");
        assert!(reason.contains("-p1"), "{reason}");
        assert!(reason.contains("-p0"), "{reason}");
    }

    /// Пустой корпус — явная ошибка с подсказкой; несуществующий каталог —
    /// тоже (fail-closed, а не «поймано 0 из 0»).
    #[test]
    fn empty_or_missing_corpus_is_an_error() {
        let tmp = tempfile::tempdir().expect("tmp");
        let err = load_corpus(&tmp.path().join("нет-такого")).expect_err("нет каталога");
        assert!(err.to_string().contains("недоступен"), "{err}");
    }

    /// Шапка корпусного прогона: числа найденного/применённого/пойманного и
    /// честное «порог не задан» вместо вердикта приёмки.
    #[test]
    fn summary_names_counts_and_no_threshold() {
        let info = CorpusRunInfo {
            dir: PathBuf::from("corpus"),
            found: 5,
            applied: 4,
            caught: 3,
            not_applicable: 1,
            rejected: 0,
        };
        let text = render_summary(&info);
        assert!(text.contains("найдено патчей: 5"), "{text}");
        assert!(text.contains("применилось: 4"), "{text}");
        assert!(text.contains("3/4 (75%)"), "{text}");
        assert!(text.contains("порог не задан"), "{text}");
        // Без применимых — честное «измерять нечего».
        let empty = CorpusRunInfo {
            applied: 0,
            ..CorpusRunInfo {
                dir: PathBuf::from("corpus"),
                found: 2,
                applied: 0,
                caught: 0,
                not_applicable: 2,
                rejected: 0,
            }
        };
        assert!(render_summary(&empty).contains("измерять нечего"));
    }
}

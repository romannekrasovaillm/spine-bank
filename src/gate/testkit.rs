//! Общие фикстуры тестов гейта (B1): git-обёртка с тестовой идентичностью
//! и репозитории-заготовки. Разделяются тестовыми модулями подмодулей гейта.

use std::path::Path;
use std::process::Command;

use super::types::{GateReport, GateStatus};

/// git в каталоге с тестовой идентичностью коммиттера (образец —
/// `src/delta.rs::make_guard_repo`).
pub(super) fn git(dir: &Path, args: &[&str]) {
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

/// Репо-фикстура гейта: git + `.arch-handoff/CONSTRAINTS.yaml` с двумя
/// error-правилами и файл, который они требуют. Один коммит.
pub(super) fn make_gate_repo(dir: &Path) {
    std::fs::create_dir_all(dir.join(".arch-handoff")).expect("mkdir handoff");
    std::fs::write(
            dir.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n  - name: no_pan\n    type: must_not_contain\n    glob: \"**/*.py\"\n    pattern: 'PAN'\n    severity: error\n",
        )
        .expect("constraints");
    std::fs::write(dir.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    git(dir, &["init", "-q"]);
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

/// Статус составляющей по имени.
pub(super) fn status_of(report: &GateReport, name: &str) -> GateStatus {
    report
        .components
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("нет составляющей {name}"))
        .status
}
/// Пишет минимальную модель: `(id, хвост frontmatter)`.
pub(super) fn write_model(dir: &Path, entities: &[(&str, &str)]) {
    let model = dir.join("model");
    std::fs::create_dir_all(&model).expect("mkdir model");
    for (id, body) in entities {
        // Тип выводится из префикса ID — иначе `id-type-mismatch`.
        let kind = id.split('-').next().unwrap_or("cmp").to_ascii_lowercase();
        std::fs::write(
                model.join(format!("{id}.md")),
                format!(
                    "---\nid: {id}\ntype: {kind}\ntitle: \"{id}\"\nstatus: \"designed\"\n{body}\n---\n\n# {id}\n"
                ),
            )
            .expect("write entity");
    }
}
/// git-репозиторий без единого коммита: `.arch-handoff/CONSTRAINTS.yaml`
/// и spine на месте, базы для диффа/сравнения нет.
pub(super) fn make_uncommitted_repo(repo: &Path) {
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("mkdir");
    std::fs::write(
            repo.join(".arch-handoff/CONSTRAINTS.yaml"),
            "rules:\n  - name: spine_present\n    type: file_exists\n    path: \"ARCHITECTURE-SPINE.md\"\n    severity: error\n",
        )
        .expect("constraints");
    std::fs::write(repo.join("ARCHITECTURE-SPINE.md"), "# Spine\n").expect("spine");
    git(repo, &["init", "-q"]);
}

/// Записывает файл фикстуры с созданием родительских каталогов.
pub(super) fn write_fixture(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, text).expect("write fixture");
}

/// Вывод git-команды в фикстуре (тестkit даёт `git`, но не отдаёт stdout;
/// базовый коммит нужен как SHA). Идентичность коммиттера — как в `git`.
pub(super) fn git_stdout(dir: &Path, args: &[&str]) -> String {
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
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

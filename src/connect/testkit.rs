//! Общие фикстуры тестов `connect` (B1): чтение файла, снимок дерева,
//! счёт хуков по маркеру, библиотека пользователя, git-обёртка с тестовой
//! идентичностью. Разделяются тестовыми модулями подмодулей.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::connect::hooks::HOOK_MARKER;

/// Читает файл в строку (тестовый хелпер).
pub(super) fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read")
}

/// Снимок дерева каталога: отсортированные (путь, содержимое).
pub(super) fn snapshot(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).sort_by_file_name() {
        let entry = entry.expect("walkdir");
        if entry.file_type().is_file() {
            out.push((
                entry.path().strip_prefix(root).expect("rel").to_path_buf(),
                std::fs::read_to_string(entry.path()).expect("read"),
            ));
        }
    }
    out
}

/// Число хуков с нашим маркером в событии settings.json.
pub(super) fn count_marked_hooks(settings: &Value, event: &str) -> usize {
    settings["hooks"][event].as_array().map_or(0, |entries| {
        entries
            .iter()
            .filter(|g| {
                g["hooks"].as_array().is_some_and(|cmds| {
                    cmds.iter().any(|c| {
                        c["command"]
                            .as_str()
                            .is_some_and(|cmd| cmd.contains(HOOK_MARKER))
                    })
                })
            })
            .count()
    })
}

/// Библиотека пользователя на диске (plugins.dirs) с фикстурой: плагин
/// без plugin.json + один скилл.
pub(super) fn user_library(root: &Path, plugin: &str, skill: &str, body: &str) -> PathBuf {
    let lib = root.join("lib");
    let skill_dir = lib.join(plugin).join("skills").join(skill);
    std::fs::create_dir_all(&skill_dir).expect("mkdir lib");
    std::fs::write(skill_dir.join("SKILL.md"), body).expect("write skill");
    lib
}

/// git в каталоге с тестовой идентичностью коммиттера (для git-hooks).
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

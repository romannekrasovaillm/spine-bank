//! Общие фикстуры тестов MCP-сервера (разбиение B1): сервер на дефолтном
//! конфиге, in-memory прогон NDJSON-пачки через [`run_loop`], каталоги
//! знаний/плагинов, мини-репозитории (agentsmd, кейс досье) и YAML-рубрика.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::Config;

use super::protocol::run_loop;
use super::types::{McpServe, ServeMode};

/// Сервер на дефолтном конфиге (без ключей и реального дома).
pub(super) fn server() -> McpServe {
    McpServe::new(Arc::new(Config::default()))
}

/// Прогоняет пачку входных строк через in-memory цикл и возвращает
/// разобранные ответы (по одному на строку вывода). Сервер — дефолтный.
pub(super) async fn run_lines(input: &[&str]) -> Vec<Value> {
    run_lines_on(server(), input).await
}

/// Вариант [`run_lines`] на сервере с заданным конфигом (например,
/// с временными knowledge/plugins-каталогами).
pub(super) async fn run_lines_on(server: McpServe, input: &[&str]) -> Vec<Value> {
    // spawn требует 'static: пачка клонируется в owned-строки заранее.
    let owned: Vec<String> = input.iter().map(|s| (*s).to_string()).collect();
    let (read_end, mut write_end) = tokio::io::duplex(64 * 1024);
    let (out_read, out_write) = tokio::io::duplex(64 * 1024);
    let writer_task = tokio::spawn(async move {
        for line in owned {
            write_end.write_all(line.as_bytes()).await.expect("запись");
            write_end.write_all(b"\n").await.expect("запись nl");
        }
        // Закрытие write_end → EOF на read_end → цикл завершается.
    });
    run_loop(&server, read_end, out_write).await.expect("цикл");
    writer_task.await.expect("писатель");
    let mut responses = Vec::new();
    let mut lines = BufReader::new(out_read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        responses.push(serde_json::from_str(&line).expect("валидный JSON ответа"));
    }
    responses
}

/// Сервер на конфиге с временными каталогами знаний и плагинов
/// (реального дома тест не касается).
pub(super) fn server_with_dirs(kb_dir: &Path, plugins_dir: &Path) -> McpServe {
    let mut cfg = Config::default();
    cfg.knowledge.dirs = vec![kb_dir.to_path_buf()];
    cfg.knowledge.extensions = vec!["md".into(), "txt".into()];
    cfg.plugins.dirs = vec![plugins_dir.to_path_buf()];
    McpServe::new(Arc::new(cfg))
}

/// Файлы тестовой базы знаний: документ про Kafka и нейтральный readme.
pub(super) fn kb_fixture(dir: &Path) {
    std::fs::write(
        dir.join("adr-042-kafka.md"),
        "# ADR-042\n\nБрокер сообщений.\nKafka выбран как шина событий.\nИтог: kafka в проде.\n",
    )
    .expect("adr");
    std::fs::write(dir.join("readme.md"), "# Общее\n\nНичего про брокеров.\n").expect("readme");
}

/// Плагин `mine` со скиллом `arch-core` (каталог плагина — носитель
/// `skills/`, манифест синтезируется из имени каталога).
pub(super) fn plugin_fixture(dir: &Path) {
    let skill_md = dir.join("mine/skills/arch-core/SKILL.md");
    std::fs::create_dir_all(skill_md.parent().expect("parent")).expect("dirs");
    std::fs::write(
        &skill_md,
        "---\nname: arch-core\ndescription: Архитектурное ядро банка: каркас решений по ADR.\n---\n\
         # Arch Core\n\nМетодика принятия решений: фиксируйте ADR в каталоге model/adr.\n",
    )
    .expect("SKILL.md");
}

/// Сервер в режиме `--rw` на дефолтном конфиге.
pub(super) fn rw_server() -> McpServe {
    McpServe::with_mode(Arc::new(Config::default()), ServeMode::ReadWrite)
}

/// Мини-репозиторий для `agentsmd_generate` (по образцу фикстуры
/// `agentsmd::tests`): манифест, спайн, каталог ADR, `.arch-handoff/`.
pub(super) fn agentsmd_repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("docs/adr")).expect("adr");
    std::fs::create_dir_all(repo.join(".arch-handoff")).expect("handoff");
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname=\"r\"\n").expect("cargo");
    std::fs::write(
        repo.join("docs/ARCHITECTURE-SPINE.md"),
        "# Spine\n\n## AD-1. Формат id\nBinds: все\nPrevents: рассинхрон\nRule: правило\n",
    )
    .expect("spine");
    repo
}

/// YAML-фикстура рубрики из двух критериев (context вес 1, alternatives вес 3).
pub(super) fn rubric_fixture(root: &Path) -> PathBuf {
    let path = root.join("adr-quality.yaml");
    std::fs::write(
        &path,
        "name: adr-quality\n\
         description: Качество ADR\n\
         scale_max: 5\n\
         origin: anchor\n\
         criteria:\n  \
         - id: context\n    \
         name: Контекст\n    \
         description: Описан контекст и проблема\n    \
         weight: 1.0\n  \
         - id: alternatives\n    \
         name: Альтернативы\n    \
         description: Рассмотрены альтернативы\n    \
         weight: 3.0\n",
    )
    .expect("рубрика");
    path
}

/// Репозиторий-кейс для досье: спайн с инвариантом и ADR.
pub(super) fn pack_repo(root: &Path) -> PathBuf {
    let repo = root.join("case");
    std::fs::create_dir_all(repo.join("docs/adr")).expect("adr dir");
    std::fs::write(
        repo.join("ARCHITECTURE-SPINE.md"),
        "# Spine\n\n## AD-2: Детерминированный слой контроля\n\n- **Rule**: механика контроля без LLM.\n",
    )
    .expect("spine");
    std::fs::write(
        repo.join("docs/adr/ADR-001-x.md"),
        "# ADR-001\n\nРешение: контроль без LLM в гейте.\n",
    )
    .expect("adr");
    repo
}

//! `arch-be connect ci` (B1): готовая джоба архитектурного гейта под
//! площадку (GitLab CI / GitHub Actions / Jenkins) — маркерные блоки,
//! подстановка версии и адреса релизов.

use std::path::{Path, PathBuf};

use super::files::commit_file;
use super::types::ConnectReport;
use crate::error::{HarnessError, Result};

/// Маркер начала нашего блока в файле CI/хуке (комментарий; префикс `#`/`//`
/// зависит от файла — ищется голая подстрока маркера).
pub(super) const BLOCK_BEGIN: &str = "spine-connect:begin";
/// Маркер конца нашего блока.
const BLOCK_END: &str = "spine-connect:end";

/// CI-провайдер для `arch-be connect ci --provider …`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiProvider {
    /// GitLab CI: мердж-блок в `.gitlab-ci.yml`, артефакт `reports.codequality`.
    GitLab,
    /// GitHub Actions: новый `.github/workflows/spine-gate.yml`, SARIF-артефакт.
    GitHub,
    /// Jenkins: мердж-блок в `Jenkinsfile`, публикация `junit(...)`.
    Jenkins,
}

impl CiProvider {
    /// Разбор значения CLI: `gitlab` | `github` | `jenkins`.
    ///
    /// # Errors
    /// Неизвестный провайдер — сообщение со списком допустимых.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "gitlab" | "git-lab" => Ok(Self::GitLab),
            "github" | "git-hub" => Ok(Self::GitHub),
            "jenkins" => Ok(Self::Jenkins),
            other => Err(format!(
                "неизвестный CI-провайдер '{other}' (допустимы: gitlab, github, jenkins)"
            )),
        }
    }

    /// Каноничное имя для вывода.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GitLab => "gitlab",
            Self::GitHub => "github",
            Self::Jenkins => "jenkins",
        }
    }

    /// Путь целевого файла джобы в проекте.
    fn job_file(self, dir: &Path) -> PathBuf {
        match self {
            Self::GitLab => dir.join(".gitlab-ci.yml"),
            Self::GitHub => dir.join(".github/workflows/spine-gate.yml"),
            Self::Jenkins => dir.join("Jenkinsfile"),
        }
    }

    /// Текст джобы (между маркерами [`BLOCK_BEGIN`]/[`BLOCK_END`]).
    fn job_block(self) -> String {
        match self {
            Self::GitLab => gitlab_ci_block(),
            Self::GitHub => github_workflow_block(),
            Self::Jenkins => jenkinsfile_block(),
        }
    }
}

/// Подстановка версии крейта в шаблоны CI (токен `@ARCH_BE_VERSION@` —
/// `format!` не подходит: в шаблонах есть `${{ … }}` GitHub Actions).
fn with_version(template: &str) -> String {
    template.replace("@ARCH_BE_VERSION@", env!("CARGO_PKG_VERSION"))
}

/// Джоба GitLab CI: `arch-be gate` с нативным форматом `gitlab-codequality`
/// в артефакт `reports.codequality` (нарушения появляются в интерфейсе merge
/// request без ручной настройки) + текстовая сводка в лог джобы.
fn gitlab_ci_block() -> String {
    with_version(
        "# spine-connect:begin — архитектурный гейт Spine (arch-be)\n\
         # Блок перегенерируется: `arch-be connect ci --provider gitlab`; свои правки — вне маркеров.\n\
         # Нарушения видны в интерфейсе merge request (Code Quality) из артефакта\n\
         # reports.codequality — ручной настройки не нужно.\n\
         spine-gate:\n\
         \x20 stage: test\n\
         \x20 image: debian:bookworm-slim\n\
         \x20 variables:\n\
         \x20   ARCH_BE_VERSION: \"@ARCH_BE_VERSION@\"\n\
         \x20   # Откуда брать бинарь arch-be (linux-x86_64):\n\
         \x20   #   A) релизы: ${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\n\
         \x20   #   B) закрытый контур: офлайн-бандл spine-offline-*-linux-x86_64.tar.gz\n\
         \x20   #      во внутреннем хранилище артефактов (scripts/make_offline_bundle.sh).\n\
         \x20   RELEASES_URL: \"https://github.com/<org>/<repo>/releases/download\"\n\
         \x20   GIT_DEPTH: \"0\"   # полная история: гейту нужна база диффа origin/<целевая ветка>\n\
         \x20 before_script:\n\
         \x20   - apt-get update -qq && apt-get install -y -qq curl ca-certificates git > /dev/null\n\
         \x20   - curl -fsSL -o /usr/local/bin/arch-be \"${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\" && chmod +x /usr/local/bin/arch-be\n\
         \x20   - arch-be --version\n\
         \x20 script:\n\
         \x20   # Машинный отчёт — в файл артефакта; при провале гейта джоба красная (exit 1).\n\
         \x20   - arch-be gate --route auto --base \"origin/${CI_MERGE_REQUEST_TARGET_BRANCH_NAME:-main}\" --format gitlab-codequality > codequality-spine.json || GATE_EXIT=$?\n\
         \x20   # Текстовая сводка в лог джобы (при дорогих правилах command_succeeds строку можно убрать).\n\
         \x20   - arch-be gate --route auto --base \"origin/${CI_MERGE_REQUEST_TARGET_BRANCH_NAME:-main}\" || true\n\
         \x20   - exit ${GATE_EXIT:-0}\n\
         \x20 artifacts:\n\
         \x20   when: always\n\
         \x20   reports:\n\
         \x20     codequality: codequality-spine.json   # → виджет Code Quality в merge request\n\
         # spine-connect:end",
    )
}

/// Workflow GitHub Actions: `arch-be gate` с SARIF (артефакт прогона; при
/// включённом Advanced Security — загрузка в code scanning, вариант в
/// комментарии) + markdown-сводка в Job Summary. Внешних действий сверх
/// официальных `actions/checkout` и `actions/upload-artifact` нет.
fn github_workflow_block() -> String {
    with_version(
        "# spine-connect:begin — архитектурный гейт Spine (arch-be)\n\
         # Файл целиком генерируется `arch-be connect ci --provider github`; перегенерация — той же командой.\n\
         name: spine-gate\n\
         on:\n\
         \x20 pull_request:\n\
         \x20 push:\n\
         \x20   branches: [main]\n\
         permissions: {}\n\
         jobs:\n\
         \x20 gate:\n\
         \x20   runs-on: ubuntu-latest\n\
         \x20   steps:\n\
         \x20     - uses: actions/checkout@v4\n\
         \x20       with:\n\
         \x20         fetch-depth: 0   # полная история для --base (дифф к целевой ветке)\n\
         \x20     - name: Установка arch-be\n\
         \x20       env:\n\
         \x20         ARCH_BE_VERSION: \"@ARCH_BE_VERSION@\"\n\
         \x20         # A) релизы (linux-x86_64); B) закрытый контур — URL офлайн-бандла\n\
         \x20         # из внутреннего хранилища артефактов.\n\
         \x20         RELEASES_URL: \"https://github.com/<org>/<repo>/releases/download\"\n\
         \x20       run: |\n\
         \x20         sudo curl -fsSL -o /usr/local/bin/arch-be \"${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\" && sudo chmod +x /usr/local/bin/arch-be\n\
         \x20         arch-be --version\n\
         \x20     - name: Архитектурный гейт\n\
         \x20       run: |\n\
         \x20         BASE=\"origin/${{ github.base_ref || 'main' }}\"\n\
         \x20         arch-be gate --route auto --base \"${BASE}\" --format sarif > spine-gate.sarif || GATE_EXIT=$?\n\
         \x20         arch-be gate --route auto --base \"${BASE}\" --format markdown >> \"$GITHUB_STEP_SUMMARY\" || true\n\
         \x20         exit ${GATE_EXIT:-0}\n\
         \x20     - name: Отчёт SARIF артефактом\n\
         \x20       if: always()\n\
         \x20       uses: actions/upload-artifact@v4\n\
         \x20       with:\n\
         \x20         name: spine-gate-sarif\n\
         \x20         path: spine-gate.sarif\n\
         \x20     # Вариант для code scanning (Security → Code scanning), если включён\n\
         \x20     # GitHub Advanced Security:\n\
         \x20     # - name: Загрузка SARIF\n\
         \x20     #   if: always()\n\
         \x20     #   uses: github/codeql-action/upload-sarif@v3\n\
         \x20     #   with: { sarif_file: spine-gate.sarif }\n\
         # spine-connect:end",
    )
}

/// Джоба Jenkins (declarative pipeline): `arch-be gate --format junit` в файл,
/// публикация `junit(...)` (находки — как упавшие тесты) и `error(...)` по
/// коду возврата гейта.
fn jenkinsfile_block() -> String {
    with_version(
        "// spine-connect:begin — архитектурный гейт Spine (arch-be)\n\
         // Блок перегенерируется: `arch-be connect ci --provider jenkins`; свои правки — вне маркеров.\n\
         pipeline {\n\
         \x20   agent any\n\
         \x20   stages {\n\
         \x20       stage('Spine gate') {\n\
         \x20           steps {\n\
         \x20               // Бинарь arch-be (linux-x86_64): A) curl из релизов (ниже);\n\
         \x20               // B) закрытый контур — офлайн-бандл из внутреннего хранилища\n\
         \x20               // (укажите его адрес в RELEASES_URL).\n\
         \x20               sh '''\n\
         \x20                 if ! command -v arch-be >/dev/null 2>&1; then\n\
         \x20                   curl -fsSL -o /tmp/arch-be \"${RELEASES_URL:-https://github.com/<org>/<repo>/releases/download}/v@ARCH_BE_VERSION@/arch-be-linux-x86_64\" && install -m 755 /tmp/arch-be /usr/local/bin/arch-be\n\
         \x20                 fi\n\
         \x20                 arch-be --version\n\
         \x20               '''\n\
         \x20               script {\n\
         \x20                   // Код возврата сохраняем: junit() публикуем даже при красном гейте.\n\
         \x20                   env.SPINE_GATE_EXIT = sh(script: 'arch-be gate --route auto --format junit > spine-gate.xml', returnStatus: true).toString()\n\
         \x20                   sh 'arch-be gate --route auto || true'   // текстовая сводка в лог\n\
         \x20               }\n\
         \x20           }\n\
         \x20           post {\n\
         \x20               always {\n\
         \x20                   junit testResults: 'spine-gate.xml', allowEmptyResults: true\n\
         \x20                   archiveArtifacts artifacts: 'spine-gate.xml', allowEmptyArchive: true\n\
         \x20                   script {\n\
         \x20                       if (env.SPINE_GATE_EXIT != '0') {\n\
         \x20                           error('Spine gate FAIL — находки в spine-gate.xml и в логе джобы')\n\
         \x20                       }\n\
         \x20                   }\n\
         \x20               }\n\
         \x20           }\n\
         \x20       }\n\
         \x20   }\n\
         }\n\
         // spine-connect:end",
    )
}

/// Заменяет зону между маркерами [`BLOCK_BEGIN`]/[`BLOCK_END`] в существующем
/// файле; маркеров нет — дописка блока в конец (чужое содержимое сохраняется).
/// Детерминировано: повторный прогон побайтово совпадает.
pub(super) fn splice_marked_block(existing: &str, block: &str) -> String {
    let trimmed = block.trim_end();
    match (existing.find(BLOCK_BEGIN), existing.find(BLOCK_END)) {
        (Some(b), Some(e)) if b < e => {
            // Границы — по строкам: начало строки с маркером begin и конец
            // строки с маркером end.
            let start = existing[..b].rfind('\n').map_or(0, |i| i + 1);
            let end = existing[e..]
                .find('\n')
                .map_or(existing.len(), |i| e + i + 1);
            let pre = existing[..start].trim_end();
            let tail = existing[end..].trim();
            // Те же разделители, что в ветке дописки (пустая строка между
            // зонами) — иначе повторный прогон менял бы файл после дописки.
            match (pre.is_empty(), tail.is_empty()) {
                (true, true) => format!("{trimmed}\n"),
                (true, false) => format!("{trimmed}\n\n{tail}\n"),
                (false, true) => format!("{pre}\n\n{trimmed}\n"),
                (false, false) => format!("{pre}\n\n{trimmed}\n\n{tail}\n"),
            }
        }
        _ => format!("{}\n\n{trimmed}\n", existing.trim_end()),
    }
}

/// Создаёт файл джобы либо встраивает блок в существующий (мердж маркерный —
/// чужие джобы/этапы сохраняются). GitHub — особый: `spine-gate.yml` —
/// целиком наш файл, существующий без маркера не затираем (отказ).
fn upsert_ci_job(
    provider: CiProvider,
    dir: &Path,
    dry_run: bool,
    releases_url: Option<&str>,
    report: &mut ConnectReport,
) -> Result<()> {
    let path = provider.job_file(dir);
    let block = substitute_releases_url(&provider.job_block(), releases_url);
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HarnessError::io(path, e)),
    };
    let new = match old.as_deref() {
        None => format!("{block}\n"),
        Some(existing) => {
            if provider == CiProvider::GitHub && !existing.contains(BLOCK_BEGIN) {
                return Err(HarnessError::Config(format!(
                    "{}: файл существует и не помечен маркером «{BLOCK_BEGIN}» — не затираю; \
                     переименуйте его или удалите вручную",
                    path.display()
                )));
            }
            if existing.contains(BLOCK_BEGIN) {
                report.notes.push(format!(
                    "{}: блок между маркерами «{BLOCK_BEGIN}/{BLOCK_END}» обновлён, \
                     содержимое вне маркеров сохранено",
                    path.display()
                ));
            } else {
                report.notes.push(format!(
                    "{}: джоба дописана блоком с маркерами «{BLOCK_BEGIN}/{BLOCK_END}» в конец \
                     файла; свои секции проверьте на конфликт имён (job `spine-gate`)",
                    path.display()
                ));
            }
            splice_marked_block(existing, &block)
        }
    };
    commit_file(&path, old.as_deref(), &new, dry_run, report)
}

/// `arch-be connect ci --provider …`: пишет готовую джобу архитектурного
/// гейта под площадку (см. [`CiProvider`]).
///
/// # Errors
/// Целевой файл GitHub существует без нашего маркера; ошибки чтения/записи.
pub fn connect_ci(
    provider: CiProvider,
    dir: &Path,
    dry_run: bool,
    releases_url: Option<&str>,
) -> Result<ConnectReport> {
    let mut report = ConnectReport {
        dry_run,
        ..ConnectReport::default()
    };
    upsert_ci_job(provider, dir, dry_run, releases_url, &mut report)?;
    match provider {
        CiProvider::GitLab => {
            report.notes.push(
                "нарушения появятся в интерфейсе merge request (Code Quality) из артефакта \
                 reports.codequality — ручной настройки площадки не нужно"
                    .into(),
            );
            report.next_steps.extend([
                "закоммитьте .gitlab-ci.yml и откройте merge request — джоба spine-gate \
                 появится в пайплайне MR"
                    .to_string(),
                if releases_url.is_none() {
                    "задайте адрес релизов: `arch-be connect ci --provider gitlab \
                     --releases-url https://github.com/<org>/<repo>/releases/download` — \
                     сейчас в шаблоне заглушка <org>/<repo>"
                        .to_string()
                } else {
                    "адрес релизов подставлен из --releases-url".to_string()
                },
            ]);
        }
        CiProvider::GitHub => {
            report.notes.push(
                "SARIF складывается артефактом прогона (actions/upload-artifact); загрузка в \
                 code scanning (вкладка Security) — закомментированным шагом в файле (нужен \
                 GitHub Advanced Security)"
                    .into(),
            );
            report.next_steps.extend([
                "закоммитьте .github/workflows/spine-gate.yml — workflow spine-gate появится \
                 на pull_request и push в main"
                    .to_string(),
                "замените <org>/<repo> в RELEASES_URL на адрес релизов/хранилища, где лежит \
                 arch-be-linux-x86_64"
                    .to_string(),
            ]);
        }
        CiProvider::Jenkins => {
            report.next_steps.extend([
                "закоммитьте Jenkinsfile (или перенесите блок в свой) — этап 'Spine gate' \
                 публикует находки через junit(...)"
                    .to_string(),
                "бинарь arch-be: готовый в PATH агента Jenkins или curl из релизов/бандла \
                 (варианты — в комментарии этапа)"
                    .to_string(),
            ]);
        }
    }
    Ok(report)
}

/// Есть ли в конфигурации CI проекта незаменённая заглушка `<org>/<repo>`.
///
/// Читает `doctor`: молчаливая заглушка в джобе выглядит как рабочая
/// настройка, а пайплайн упадёт на первом же прогоне — предупреждать надо
/// заранее, а не по факту красного CI.
#[must_use]
pub fn ci_placeholder_present(dir: &Path) -> bool {
    [
        ".gitlab-ci.yml",
        ".github/workflows/spine-gate.yml",
        "Jenkinsfile",
    ]
    .iter()
    .filter_map(|rel| std::fs::read_to_string(dir.join(rel)).ok())
    .any(|text| text.contains("<org>/<repo>"))
}

/// Подставляет адрес релизов в шаблон джобы вместо заглушки `<org>/<repo>`.
///
/// Без `--releases-url` шаблон остаётся с заглушкой (джоба печатается как
/// черновик), но в «Следующих шагах» появляется строка, которую надо
/// отредактировать, а `doctor` предупреждает — молчаливая заглушка в CI
/// выглядит как рабочая конфигурация (Н-CI волны C 0.3.4).
fn substitute_releases_url(block: &str, releases_url: Option<&str>) -> String {
    let Some(url) = releases_url else {
        return block.to_string();
    };
    let url = url.trim().trim_end_matches('/');
    block.replace("https://github.com/<org>/<repo>/releases/download", url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::git_hooks::pre_push_hook_block;
    use crate::connect::hooks::{post_tool_use_hook_command, stop_hook_command};
    use crate::connect::testkit::read;

    /// T-03: шаблоны передают базу ГОЛОЙ ревизией. Готовый диапазон
    /// `rev...HEAD` гейт дополнял вторым `...HEAD`, git отказывал, и гейт
    /// молча уходил в fail-safe Critical — строгость зависела от формы записи
    /// базы, а не от изменения.
    #[test]
    fn templates_pass_a_bare_base_revision() {
        let mut blocks = vec![
            ("stop-хук", stop_hook_command()),
            ("post-tool-use", post_tool_use_hook_command()),
            ("pre-push", pre_push_hook_block()),
        ];
        for provider in [CiProvider::GitLab, CiProvider::GitHub, CiProvider::Jenkins] {
            blocks.push((provider.name(), provider.job_block()));
        }
        for (name, block) in blocks {
            assert!(
                !block.contains("...HEAD"),
                "{name}: база обязана быть голой ревизией: {block}"
            );
            // Jenkins-шаблон базу не передаёт вовсе (гейт считает дифф
            // рабочего дерева против HEAD) — это отдельная тема, не двойной
            // `...HEAD`; проверяем те шаблоны, где база есть.
            if name != "jenkins" {
                assert!(
                    block.contains("--base "),
                    "{name}: гейт вызывается с базой: {block}"
                );
            }
        }
    }

    /// Разбор CI-провайдеров: алиасы, регистр, ошибка со списком допустимых.
    #[test]
    fn ci_provider_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(CiProvider::parse("gitlab"), Ok(CiProvider::GitLab));
        assert_eq!(CiProvider::parse("GitHub"), Ok(CiProvider::GitHub));
        assert_eq!(CiProvider::parse("jenkins"), Ok(CiProvider::Jenkins));
        let err = CiProvider::parse("gitlab-ci").expect_err("неизвестный провайдер");
        assert!(err.contains("gitlab"), "{err}");
        assert!(err.contains("jenkins"), "{err}");
    }

    /// Джобы всех трёх провайдеров: файл с маркерами и нативной командой
    /// гейта; сухой прогон ничего не пишет; повтор — без дублей.
    #[test]
    fn ci_scaffolds_job_per_provider_idempotently() {
        for (provider, rel, native) in [
            (
                CiProvider::GitLab,
                ".gitlab-ci.yml",
                "--format gitlab-codequality",
            ),
            (
                CiProvider::GitHub,
                ".github/workflows/spine-gate.yml",
                "--format sarif",
            ),
            (CiProvider::Jenkins, "Jenkinsfile", "--format junit"),
        ] {
            let tmp = tempfile::tempdir().expect("tmp");
            let dir = tmp.path().join("proj");
            std::fs::create_dir_all(&dir).expect("mkdir");

            // --dry-run: план есть, файла нет.
            let report = connect_ci(provider, &dir, true, None).expect("dry-run");
            assert!(report.dry_run);
            assert!(
                report.created.iter().any(|p| p.ends_with(rel)),
                "{}: план без {rel}: {:?}",
                provider.name(),
                report.created
            );
            assert!(
                !dir.join(rel).exists(),
                "{}: dry-run записал файл",
                provider.name()
            );

            // Реальный прогон.
            let report = connect_ci(provider, &dir, false, None).expect("connect ci");
            let text = read(&dir.join(rel));
            assert!(text.contains(BLOCK_BEGIN), "{}: {text}", provider.name());
            assert!(text.contains(BLOCK_END), "{}: {text}", provider.name());
            assert!(
                text.contains("arch-be gate --route auto"),
                "{}: {text}",
                provider.name()
            );
            assert!(
                text.contains(native),
                "{}: нативный формат {native}: {text}",
                provider.name()
            );
            assert!(
                text.contains("arch-be-linux-x86_64"),
                "{}: установка бинаря: {text}",
                provider.name()
            );
            assert!(
                report.created.iter().any(|p| p.ends_with(rel)),
                "{:?}",
                report.created
            );

            // Повтор — без изменений и без дублей маркеров.
            let report = connect_ci(provider, &dir, false, None).expect("повтор");
            assert_eq!(
                read(&dir.join(rel)),
                text,
                "{}: повтор изменил файл",
                provider.name()
            );
            assert!(
                report.unchanged.iter().any(|p| p.ends_with(rel)),
                "{}: {:?}",
                provider.name(),
                report.unchanged
            );
            assert_eq!(text.matches(BLOCK_BEGIN).count(), 1, "дубль маркера");
        }
    }

    /// GitLab: существующий `.gitlab-ci.yml` с чужой джобой — блок дописывается,
    /// чужое сохраняется; повтор заменяет блок без дублей.
    #[test]
    fn ci_gitlab_merges_into_existing_pipeline() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(".gitlab-ci.yml"),
            "stages: [test]\n\nunit-tests:\n  stage: test\n  script: cargo test\n",
        )
        .expect("write .gitlab-ci.yml");

        connect_ci(CiProvider::GitLab, &dir, false, None).expect("connect ci");
        let text = read(&dir.join(".gitlab-ci.yml"));
        assert!(text.contains("unit-tests:"), "чужая джоба цела: {text}");
        assert!(text.contains("spine-gate:"), "наша джоба: {text}");
        assert!(
            text.contains("codequality: codequality-spine.json"),
            "{text}"
        );

        // Повтор: блок заменяется, чужая зона не трогается, дублей нет.
        connect_ci(CiProvider::GitLab, &dir, false, None).expect("повтор");
        let again = read(&dir.join(".gitlab-ci.yml"));
        assert_eq!(again, text, "повтор изменил файл");
        assert_eq!(again.matches("spine-gate:").count(), 1, "{again}");
    }

    /// GitHub: существующий workflow без нашего маркера — отказ без затирания;
    /// с маркером — обновление блока.
    #[test]
    fn ci_github_refuses_foreign_workflow_file() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let wf = dir.join(".github/workflows/spine-gate.yml");
        std::fs::create_dir_all(wf.parent().expect("parent")).expect("mkdir");
        std::fs::write(&wf, "name: mine\non: [push]\n").expect("write workflow");

        let err = connect_ci(CiProvider::GitHub, &dir, false, None).expect_err("отказ");
        assert!(err.to_string().contains("не затираю"), "{err}");
        assert_eq!(read(&wf), "name: mine\non: [push]\n", "файл цел");

        // Наш файл (с маркером) обновляется.
        std::fs::write(
            &wf,
            "# spine-connect:begin\nname: spine-gate\n# spine-connect:end\n",
        )
        .expect("write marked");
        connect_ci(CiProvider::GitHub, &dir, false, None).expect("обновление нашего файла");
        let text = read(&wf);
        assert!(text.contains("--format sarif"), "{text}");
        assert_eq!(text.matches(BLOCK_BEGIN).count(), 1, "{text}");
    }

    /// Маркерный сплайс: замена зоны между маркерами, рукописное снаружи цело.
    #[test]
    fn splice_marked_block_preserves_handwritten_zones() {
        let block = "# spine-connect:begin\nA\n# spine-connect:end";
        let existing =
            "голова\n\n# spine-connect:begin\nстарая зона\n# spine-connect:end\n\nхвост\n";
        let out = splice_marked_block(existing, block);
        assert!(out.contains("голова"), "{out}");
        assert!(out.contains("хвост"), "{out}");
        assert!(out.contains("\nA\n"), "{out}");
        assert!(!out.contains("старая зона"), "{out}");
        // Повтор детерминирован.
        assert_eq!(splice_marked_block(&out, block), out);
        // Без маркеров — дописка.
        let appended = splice_marked_block("чужое\n", block);
        assert!(appended.starts_with("чужое\n\n"), "{appended}");
        assert!(appended.contains(BLOCK_BEGIN), "{appended}");
    }
}

#[cfg(test)]
mod tests_releases_url {
    //! Волна C 0.3.4: заглушка `<org>/<repo>` в шаблоне CI не должна выглядеть
    //! рабочей конфигурацией.

    use super::*;

    fn ci_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arch-be-ci-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// `--releases-url` подставляется в джобу; заглушки в файле не остаётся.
    #[test]
    fn releases_url_is_substituted_into_ci_job() {
        let dir = ci_dir("url");
        connect_ci(
            CiProvider::GitLab,
            &dir,
            false,
            Some("https://releases.example.invalid/arch-be/"),
        )
        .expect("connect ci");
        let text = std::fs::read_to_string(dir.join(".gitlab-ci.yml")).expect("read");
        assert!(
            text.contains(r#"RELEASES_URL: "https://releases.example.invalid/arch-be""#),
            "адрес обязан быть подставлен без хвостового слэша: {text}"
        );
        assert!(
            !text.contains("<org>/<repo>"),
            "заглушки в джобе остаться не должно: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Без адреса — заглушка остаётся, но «Следующие шаги» прямо называют
    /// команду с `--releases-url`, а не молчат.
    #[test]
    fn missing_releases_url_is_named_in_next_steps() {
        let dir = ci_dir("no-url");
        let report = connect_ci(CiProvider::GitLab, &dir, false, None).expect("connect ci");
        assert!(
            report
                .next_steps
                .iter()
                .any(|s| s.contains("--releases-url")),
            "шаг обязан называть команду: {:?}",
            report.next_steps
        );
        // Проверка для doctor/CI-файла: заглушка видна.
        assert!(super::ci_placeholder_present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

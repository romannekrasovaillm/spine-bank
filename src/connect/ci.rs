//! `arch-be connect ci` (B1): готовая джоба архитектурного гейта под
//! площадку (GitLab CI / GitHub Actions / Jenkins) — маркерные блоки,
//! подстановка версии и адреса релизов.
//!
//! K4 (волна K): рядом с джобой гейта рендерится джоба `arch-diff` —
//! `arch-be arch-diff --base <целевая> --format md` публикуется ОДНИМ
//! комментарием MR/PR, обновляемым на каждом пуше (маркер
//! `<!-- spine-arch-diff -->` + edit вместо new). Публикация — средствами
//! пайплайна (curl/gh и токены площадки), бинарь сети не касается (C-34).
//! Для Bitbucket Data Center доставка вердикта — Code Insights
//! (`--format bitbucket-insights`, D1, ADR-059) — самообновляющийся отчёт,
//! комментарий не нужен.

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
    /// Bitbucket Data Center (D1, ADR-059): мердж-блок в `Jenkinsfile` —
    /// вердикт публикуется в Code Insights (build-status API и отчёт) под
    /// merge checks «Required builds» и «Code Insights».
    Bitbucket,
}

impl CiProvider {
    /// Разбор значения CLI: `gitlab` | `github` | `jenkins` | `bitbucket`.
    ///
    /// # Errors
    /// Неизвестный провайдер — сообщение со списком допустимых.
    pub fn parse(raw: &str) -> std::result::Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "gitlab" | "git-lab" => Ok(Self::GitLab),
            "github" | "git-hub" => Ok(Self::GitHub),
            "jenkins" => Ok(Self::Jenkins),
            "bitbucket" | "bitbucket-dc" | "bitbucket-data-center" => Ok(Self::Bitbucket),
            other => Err(format!(
                "неизвестный CI-провайдер '{other}' \
                 (допустимы: gitlab, github, jenkins, bitbucket)"
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
            Self::Bitbucket => "bitbucket",
        }
    }

    /// Путь целевого файла джобы в проекте.
    fn job_file(self, dir: &Path) -> PathBuf {
        match self {
            Self::GitLab => dir.join(".gitlab-ci.yml"),
            Self::GitHub => dir.join(".github/workflows/spine-gate.yml"),
            // Bitbucket DC ходит через Jenkins — блок живёт в том же
            // Jenkinsfile, что у Jenkins-провайдера (маркерный сплайс).
            Self::Jenkins | Self::Bitbucket => dir.join("Jenkinsfile"),
        }
    }

    /// Текст джобы (между маркерами [`BLOCK_BEGIN`]/[`BLOCK_END`]).
    fn job_block(self) -> String {
        match self {
            Self::GitLab => gitlab_ci_block(),
            Self::GitHub => github_workflow_block(),
            Self::Jenkins => jenkinsfile_block(),
            Self::Bitbucket => bitbucket_jenkinsfile_block(),
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
/// Рядом — джоба `spine-arch-diff` (K4): `arch-be arch-diff --format md`
/// одним обновляемым комментарием MR (маркер + PUT/POST в notes API).
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
         # --- K4: архитектурный дифф MR одним обновляемым комментарием ---\n\
         spine-arch-diff:\n\
         \x20 stage: test\n\
         \x20 image: debian:bookworm-slim\n\
         \x20 rules:\n\
         \x20   - if: $CI_PIPELINE_SOURCE == \"merge_request_event\"\n\
         \x20 variables:\n\
         \x20   ARCH_BE_VERSION: \"@ARCH_BE_VERSION@\"\n\
         \x20   RELEASES_URL: \"https://github.com/<org>/<repo>/releases/download\"\n\
         \x20   GIT_DEPTH: \"0\"   # диффу нужна база origin/<целевая ветка>\n\
         \x20 before_script:\n\
         \x20   - apt-get update -qq && apt-get install -y -qq curl ca-certificates git jq > /dev/null\n\
         \x20   - curl -fsSL -o /usr/local/bin/arch-be \"${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\" && chmod +x /usr/local/bin/arch-be\n\
         \x20 script:\n\
         \x20   - BASE=\"origin/${CI_MERGE_REQUEST_TARGET_BRANCH_NAME:-main}\"\n\
         \x20   # Дифф информационный (exit 0); краснота MR — джоба spine-gate или --fail-on у arch-diff.\n\
         \x20   - arch-be arch-diff --repo . --base \"$BASE\" --format md > arch-diff.md\n\
         \x20   # Один комментарий MR, обновляемый на каждом пуше: маркер + edit вместо new.\n\
         \x20   # Публикация — curl в GitLab API из пайплайна; бинарь arch-be сети не касается (C-34).\n\
         \x20   # GITLAB_TOKEN — masked CI-переменная (project access token, scope api):\n\
         \x20   # у CI_JOB_TOKEN прав на комментарии MR нет.\n\
         \x20   - |\n\
         \x20     MARKER='<!-- spine-arch-diff -->'\n\
         \x20     { echo \"$MARKER\"; echo; cat arch-diff.md; } > .arch-diff-comment.md\n\
         \x20     PAYLOAD=$(jq -n --arg b \"$(cat .arch-diff-comment.md)\" '{body: $b}')\n\
         \x20     NOTES=\"${CI_API_V4_URL}/projects/${CI_PROJECT_ID}/merge_requests/${CI_MERGE_REQUEST_IID}/notes\"\n\
         \x20     AUTH=\"PRIVATE-TOKEN: ${GITLAB_TOKEN:?задайте masked CI-переменную GITLAB_TOKEN (project access token, scope api)}\"\n\
         \x20     NOTE_ID=$(curl -fsS -H \"$AUTH\" \"${NOTES}?per_page=100&sort=desc\" | jq -r '[.[] | select(.body | contains(\"<!-- spine-arch-diff -->\")) | .id][0] // empty')\n\
         \x20     if [ -n \"$NOTE_ID\" ]; then\n\
         \x20       curl -fsS -X PUT -H \"$AUTH\" -H 'Content-Type: application/json' --data \"$PAYLOAD\" \"${NOTES}/${NOTE_ID}\" > /dev/null\n\
         \x20     else\n\
         \x20       curl -fsS -X POST -H \"$AUTH\" -H 'Content-Type: application/json' --data \"$PAYLOAD\" \"$NOTES\" > /dev/null\n\
         \x20     fi\n\
         \x20     echo \"arch-diff: комментарий MR ${NOTE_ID:+обновлён}${NOTE_ID:-создан}\"\n\
         \x20 artifacts:\n\
         \x20   when: always\n\
         \x20   paths:\n\
         \x20     - arch-diff.md\n\
         # spine-connect:end",
    )
}

/// Workflow GitHub Actions: `arch-be gate` с SARIF (артефакт прогона; при
/// включённом Advanced Security — загрузка в code scanning, вариант в
/// комментарии) + markdown-сводка в Job Summary. Внешних действий сверх
/// официальных `actions/checkout` и `actions/upload-artifact` нет.
/// Рядом — job `arch-diff` (K4): `arch-be arch-diff --format md` одним
/// обновляемым комментарием PR (маркер + PATCH/POST через `gh api`).
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
         \x20   # --- K4: архитектурный дифф PR одним обновляемым комментарием ---\n\
         \x20 arch-diff:\n\
         \x20   if: github.event_name == 'pull_request'\n\
         \x20   runs-on: ubuntu-latest\n\
         \x20   permissions:\n\
         \x20     contents: read\n\
         \x20     pull-requests: write   # правка одного комментария PR (маркер + edit)\n\
         \x20   steps:\n\
         \x20     - uses: actions/checkout@v4\n\
         \x20       with:\n\
         \x20         fetch-depth: 0   # диффу нужна база origin/<целевая ветка>\n\
         \x20     - name: Установка arch-be\n\
         \x20       env:\n\
         \x20         ARCH_BE_VERSION: \"@ARCH_BE_VERSION@\"\n\
         \x20         RELEASES_URL: \"https://github.com/<org>/<repo>/releases/download\"\n\
         \x20       run: |\n\
         \x20         sudo curl -fsSL -o /usr/local/bin/arch-be \"${RELEASES_URL}/v${ARCH_BE_VERSION}/arch-be-linux-x86_64\" && sudo chmod +x /usr/local/bin/arch-be\n\
         \x20         arch-be --version\n\
         \x20     - name: Архитектурный дифф\n\
         \x20       # Информационный (exit 0); красный PR — job gate или --fail-on у arch-diff.\n\
         \x20       run: arch-be arch-diff --repo . --base \"origin/${{ github.base_ref || 'main' }}\" --format md > arch-diff.md\n\
         \x20     - name: Комментарий PR (один, обновляемый)\n\
         \x20       env:\n\
         \x20         GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}\n\
         \x20         PR_NUMBER: ${{ github.event.pull_request.number }}\n\
         \x20       # Маркер + edit вместо new; публикация — gh из раннера, бинарь сети не касается (C-34).\n\
         \x20       run: |\n\
         \x20         MARKER='<!-- spine-arch-diff -->'\n\
         \x20         { echo \"$MARKER\"; echo; cat arch-diff.md; } > .arch-diff-comment.md\n\
         \x20         COMMENTS=\"repos/${{ github.repository }}/issues/${PR_NUMBER}/comments\"\n\
         \x20         CID=$(gh api \"${COMMENTS}?per_page=100\" --jq '[.[] | select(.body | contains(\"<!-- spine-arch-diff -->\")) | .id][0] // empty')\n\
         \x20         if [ -n \"$CID\" ]; then\n\
         \x20           gh api -X PATCH \"repos/${{ github.repository }}/issues/comments/${CID}\" -f body=@.arch-diff-comment.md > /dev/null\n\
         \x20         else\n\
         \x20           gh api -X POST \"$COMMENTS\" -f body=@.arch-diff-comment.md > /dev/null\n\
         \x20         fi\n\
         \x20         echo \"arch-diff: комментарий PR ${CID:+обновлён}${CID:-создан}\"\n\
         \x20     - name: Отчёт диффа артефактом\n\
         \x20       if: always()\n\
         \x20       uses: actions/upload-artifact@v4\n\
         \x20       with:\n\
         \x20         name: spine-arch-diff\n\
         \x20         path: arch-diff.md\n\
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
/// коду возврата гейта. Рядом — этап 'Spine arch-diff' (K4): дифф PR одним
/// обновляемым комментарием MR/PR (маркер + curl в API площадки; endpoint —
/// переменными окружения, формы GitLab/GitHub в комментарии этапа).
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
         \x20       // --- K4: архитектурный дифф PR одним обновляемым комментарием ---\n\
         \x20       stage('Spine arch-diff') {\n\
         \x20           // Только PR-сборки multibranch (CHANGE_ID/CHANGE_TARGET заданы).\n\
         \x20           when { changeRequest() }\n\
         \x20           steps {\n\
         \x20               // Дифф PR в файл (информационный; exit всегда 0). Красный билд — у 'Spine gate'.\n\
         \x20               sh 'arch-be arch-diff --repo . --base \"origin/${CHANGE_TARGET:-main}\" --format md > arch-diff.md'\n\
         \x20               // Один обновляемый комментарий MR/PR: маркер + edit вместо new (K4).\n\
         \x20               // Публикация — curl в REST API площадки MR из пайплайна; бинарь сети не касается (C-34).\n\
         \x20               // [ТРЕБУЕТ ПРОВЕРКИ по документации вашей площадки] — переменные окружения:\n\
         \x20               //   ARCH_DIFF_PLATFORM  gitlab (обновление PUT) | github (PATCH)\n\
         \x20               //   ARCH_DIFF_LIST_URL  коллекция комментариев (GET — список; POST — создание):\n\
         \x20               //     gitlab: https://HOST/api/v4/projects/<id>/merge_requests/<iid>/notes\n\
         \x20               //     github: https://api.github.com/repos/<org>/<repo>/issues/<n>/comments\n\
         \x20               //   ARCH_DIFF_ITEM_URL  URL комментария с плейсхолдером {id} (PUT/PATCH):\n\
         \x20               //     gitlab: .../notes/{id}   github: .../repos/<org>/<repo>/issues/comments/{id}\n\
         \x20               // jq обязателен на агенте (установка — как у бинаря arch-be выше).\n\
         \x20               withCredentials([string(credentialsId: 'arch-diff-mr-token', variable: 'ARCH_DIFF_TOKEN')]) {\n\
         \x20                   sh '''\n\
         \x20                     set -eu\n\
         \x20                     MARKER='<!-- spine-arch-diff -->'\n\
         \x20                     { echo \"$MARKER\"; echo; cat arch-diff.md; } > .arch-diff-comment.md\n\
         \x20                     PAYLOAD=$(jq -n --arg b \"$(cat .arch-diff-comment.md)\" '{body: $b}')\n\
         \x20                     case \"${ARCH_DIFF_PLATFORM:-gitlab}\" in\n\
         \x20                       gitlab) AUTH=\"PRIVATE-TOKEN: ${ARCH_DIFF_TOKEN}\"; METHOD=PUT ;;\n\
         \x20                       github) AUTH=\"Authorization: Bearer ${ARCH_DIFF_TOKEN}\"; METHOD=PATCH ;;\n\
         \x20                       *) echo \"arch-diff: неизвестная ARCH_DIFF_PLATFORM='${ARCH_DIFF_PLATFORM}' (gitlab|github)\"; exit 2 ;;\n\
         \x20                     esac\n\
         \x20                     ID=$(curl -fsS -H \"$AUTH\" \"${ARCH_DIFF_LIST_URL}?per_page=100\" | jq -r '[.[] | select(.body | contains(\"<!-- spine-arch-diff -->\")) | .id][0] // empty')\n\
         \x20                     if [ -n \"$ID\" ]; then\n\
         \x20                       ITEM_URL=$(printf '%s' \"$ARCH_DIFF_ITEM_URL\" | sed \"s/{id}/${ID}/\")\n\
         \x20                       curl -fsS -X \"$METHOD\" -H \"$AUTH\" -H 'Content-Type: application/json' --data \"$PAYLOAD\" \"$ITEM_URL\" > /dev/null\n\
         \x20                     else\n\
         \x20                       curl -fsS -X POST -H \"$AUTH\" -H 'Content-Type: application/json' --data \"$PAYLOAD\" \"$ARCH_DIFF_LIST_URL\" > /dev/null\n\
         \x20                     fi\n\
         \x20                     echo \"arch-diff: комментарий MR/PR ${ID:+обновлён}${ID:-создан}\"\n\
         \x20                   '''\n\
         \x20               }\n\
         \x20           }\n\
         \x20           post {\n\
         \x20               always {\n\
         \x20                   archiveArtifacts artifacts: 'arch-diff.md', allowEmptyArchive: true\n\
         \x20               }\n\
         \x20           }\n\
         \x20       }\n\
         \x20   }\n\
         }\n\
         // spine-connect:end",
    )
}

/// Блок Jenkinsfile для Bitbucket Data Center (D1, ADR-059).
///
/// Сеть живёт ТОЛЬКО в этом CI-скрипте (curl), в Rust-коде сетевых крейтов
/// нет (Core-граница C-34). Вердикт гейта сериализуется `--format
/// bitbucket-insights` и публикуется в Code Insights; build-status API даёт
/// merge check «Required builds».
///
/// Точные пути/поля REST API Code Insights и build-status зависят от версии
/// Bitbucket DC — при внедрении сверить с документацией Atlassian
/// (в шаблоне помечено `[ТРЕБУЕТ ПРОВЕРКИ]`); ниже — общеизвестные формы
/// (report `key`/`title`/`result`/`details`/`data[]`, build-status
/// `POST /rest/build-status/1.0/commits/<sha>` со `state`).
const BITBUCKET_BLOCK: &str = r#"// spine-connect:begin — архитектурный гейт Spine (arch-be) → Bitbucket Code Insights
// Блок перегенерируется: `arch-be connect ci --provider bitbucket`; свои правки — вне маркеров.
// Bitbucket Data Center + Jenkins. Вердикт доезжает до merge checks проекта:
//   * «Required builds» — через build-status API (job публикует state коммита);
//   * «Code Insights»   — через отчёт `--format bitbucket-insights`.
// Включите оба merge check в настройках репозитория Bitbucket.
// [ТРЕБУЕТ ПРОВЕРКИ по документации Atlassian]: точные пути/поля REST API Code
// Insights (reports, annotations) и build-status API зависят от версии Bitbucket DC;
// аутентификация ниже — Bearer-токен. Сверьте поля с докой вашей версии.
pipeline {
    agent any
    environment {
        // Заполните под свой проект (или вынесите в глобальные настройки Jenkins).
        BITBUCKET_URL     = 'https://bitbucket.example.invalid'
        BITBUCKET_PROJECT = 'PROJ'
        BITBUCKET_REPO    = 'repo'
        BITBUCKET_COMMIT  = 'HEAD'
        BITBUCKET_CREDS   = 'bitbucket-insights-token'
    }
    stages {
        stage('Spine gate') {
            steps {
                // Бинарь arch-be (linux-x86_64): A) curl из релизов (ниже);
                // B) закрытый контур — офлайн-бандл из внутреннего хранилища.
                sh '''
                  if ! command -v arch-be >/dev/null 2>&1; then
                    curl -fsSL -o /tmp/arch-be "${RELEASES_URL:-https://github.com/<org>/<repo>/releases/download}/v@ARCH_BE_VERSION@/arch-be-linux-x86_64" && install -m 755 /tmp/arch-be /usr/local/bin/arch-be
                  fi
                  arch-be --version
                '''
                script {
                    // Отчёт Code Insights (JSON) — артефакт публикации; код возврата
                    // сохраняем: публикуем отчёт и статус даже при красном гейте.
                    env.SPINE_GATE_EXIT = sh(script: 'arch-be gate --route auto --format bitbucket-insights > spine-insights.json', returnStatus: true).toString()
                    sh 'arch-be gate --route auto || true'   // текстовая сводка в лог
                }
            }
            post {
                always {
                    // [ТРЕБУЕТ ПРОВЕРКИ по документации Atlassian]: путь отчёта
                    //   PUT /rest/insights/1.0/projects/<proj>/repos/<repo>/reports/<key>
                    // и аннотаций .../reports/<key>/annotations; build-status —
                    //   POST /rest/build-status/1.0/commits/<sha> (state INPROGRESS|SUCCESSFUL|FAILED).
                    withCredentials([string(credentialsId: env.BITBUCKET_CREDS, variable: 'BITBUCKET_TOKEN')]) {
                        sh '''
                          set -eu
                          AUTH="Authorization: Bearer ${BITBUCKET_TOKEN}"
                          REPORT_URL="${BITBUCKET_URL}/rest/insights/1.0/projects/${BITBUCKET_PROJECT}/repos/${BITBUCKET_REPO}/reports/spine-gate"
                          curl -fsS -X PUT -H "$AUTH" -H 'Content-Type: application/json' --data @spine-insights.json "$REPORT_URL"
                          if [ "${SPINE_GATE_EXIT:-1}" = "0" ]; then STATE=SUCCESSFUL; else STATE=FAILED; fi
                          curl -fsS -X POST -H "$AUTH" -H 'Content-Type: application/json' \
                            --data "{\"state\":\"${STATE}\",\"key\":\"spine-gate\",\"url\":\"${REPORT_URL}\"}" \
                            "${BITBUCKET_URL}/rest/build-status/1.0/commits/${BITBUCKET_COMMIT}"
                        '''
                    }
                    archiveArtifacts artifacts: 'spine-insights.json', allowEmptyArchive: true
                    script {
                        if (env.SPINE_GATE_EXIT != '0') {
                            error('Spine gate FAIL — находки в spine-insights.json и в логе джобы')
                        }
                    }
                }
            }
        }
    }
}
// spine-connect:end"#;

/// Собирает блок Bitbucket, подставляя версию крейта.
fn bitbucket_jenkinsfile_block() -> String {
    with_version(BITBUCKET_BLOCK)
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
            report.notes.push(
                "джоба `spine-arch-diff` (K4) публикует архитектурный дифф одним обновляемым \
                 комментарием MR (маркер `<!-- spine-arch-diff -->`, edit вместо new); \
                 публикация — curl в GitLab API, бинарь сети не касается (C-34)"
                    .into(),
            );
            report.next_steps.extend([
                "закоммитьте .gitlab-ci.yml и откройте merge request — джобы spine-gate и \
                 spine-arch-diff появятся в пайплайне MR"
                    .to_string(),
                if releases_url.is_none() {
                    "задайте адрес релизов: `arch-be connect ci --provider gitlab \
                     --releases-url https://github.com/<org>/<repo>/releases/download` — \
                     сейчас в шаблоне заглушка <org>/<repo>"
                        .to_string()
                } else {
                    "адрес релизов подставлен из --releases-url".to_string()
                },
                "для комментария MR от `spine-arch-diff` задайте masked CI-переменную \
                 GITLAB_TOKEN (project access token, scope api) — без неё шаг публикации \
                 комментария красный (у CI_JOB_TOKEN прав на комментарии нет)"
                    .to_string(),
            ]);
        }
        CiProvider::GitHub => {
            report.notes.push(
                "SARIF складывается артефактом прогона (actions/upload-artifact); загрузка в \
                 code scanning (вкладка Security) — закомментированным шагом в файле (нужен \
                 GitHub Advanced Security)"
                    .into(),
            );
            report.notes.push(
                "job `arch-diff` (K4) публикует архитектурный дифф одним обновляемым \
                 комментарием PR (маркер `<!-- spine-arch-diff -->`, PATCH вместо new) — \
                 работает на встроенном GITHUB_TOKEN (permissions: pull-requests: write \
                 объявлены в workflow)"
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
                "этап 'Spine arch-diff' (K4) публикует дифф одним обновляемым комментарием \
                 MR/PR: заведите credentialsId 'arch-diff-mr-token' и переменные \
                 ARCH_DIFF_PLATFORM/ARCH_DIFF_LIST_URL/ARCH_DIFF_ITEM_URL (формы для GitLab и \
                 GitHub — в комментарии этапа); jq обязателен на агенте"
                    .to_string(),
            ]);
        }
        CiProvider::Bitbucket => {
            report.notes.push(
                "вердикт публикуется из CI-скрипта curl'ом в Bitbucket Code Insights \
                 (отчёт `--format bitbucket-insights`) и build-status API; сеть живёт в \
                 Jenkinsfile заказчика — в Rust-коде её нет (Core-граница C-34)"
                    .into(),
            );
            report.next_steps.extend([
                "закоммитьте Jenkinsfile (или перенесите блок в свой) — этап 'Spine gate' \
                 публикует отчёт Insights и статус коммита"
                    .to_string(),
                "включите в Bitbucket merge checks «Required builds» и «Code Insights», \
                 заполните BITBUCKET_URL/PROJECT/REPO и credentialsId (BITBUCKET_CREDS); \
                 точные пути REST API сверьте с докой Atlassian — в шаблоне помечено \
                 [ТРЕБУЕТ ПРОВЕРКИ]"
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
    /// базы, а не от изменения. ROADMAP 1.7 п.2: хуки — shim'ы на бинарь,
    /// база вычисляется ВНУТРИ `arch-be hook <имя>`; `--base` остаётся
    /// только в CI-шаблонах (они вызывают `gate` напрямую).
    #[test]
    fn templates_pass_a_bare_base_revision() {
        // Хуки-shim'ы: базы в shell нет — она в бинаре; двойного `...HEAD`
        // быть не может по построению.
        for (name, block) in [
            ("stop-хук", stop_hook_command()),
            ("post-tool-use", post_tool_use_hook_command()),
            ("pre-push", pre_push_hook_block()),
        ] {
            assert!(block.contains("arch-be hook "), "{name} — shim: {block}");
            assert!(
                !block.contains("...HEAD") && !block.contains("merge-base"),
                "{name}: база живёт в бинаре, не в shell: {block}"
            );
        }
        let mut blocks = Vec::new();
        for provider in [
            CiProvider::GitLab,
            CiProvider::GitHub,
            CiProvider::Jenkins,
            CiProvider::Bitbucket,
        ] {
            blocks.push((provider.name(), provider.job_block()));
        }
        for (name, block) in blocks {
            assert!(
                !block.contains("...HEAD"),
                "{name}: база обязана быть голой ревизией: {block}"
            );
            // Jenkins/Bitbucket-шаблоны базу не передают вовсе (гейт считает
            // дифф рабочего дерева против HEAD) — это отдельная тема, не
            // двойной `...HEAD`; проверяем те шаблоны, где база есть.
            if name != "jenkins" && name != "bitbucket" {
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
        assert_eq!(CiProvider::parse("bitbucket"), Ok(CiProvider::Bitbucket));
        assert_eq!(
            CiProvider::parse("bitbucket-data-center"),
            Ok(CiProvider::Bitbucket)
        );
        let err = CiProvider::parse("gitlab-ci").expect_err("неизвестный провайдер");
        assert!(err.contains("gitlab"), "{err}");
        assert!(err.contains("jenkins"), "{err}");
        assert!(err.contains("bitbucket"), "{err}");
    }

    /// Джобы всех провайдеров: файл с маркерами и нативной командой гейта;
    /// сухой прогон ничего не пишет; повтор — без дублей.
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
            (
                CiProvider::Bitbucket,
                "Jenkinsfile",
                "--format bitbucket-insights",
            ),
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

    /// D1 (ADR-059): Bitbucket-блок — маркеры, curl в Code Insights
    /// (`/rest/insights/1.0`) и build-status, комментарий про merge checks;
    /// повтор обновляет блок внутри маркеров, правки вне маркеров целы.
    #[test]
    fn ci_bitbucket_block_publishes_insights_and_keeps_handwritten_zones() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let report =
            connect_ci(CiProvider::Bitbucket, &dir, false, None).expect("connect ci bitbucket");
        assert!(
            report.created.iter().any(|p| p.ends_with("Jenkinsfile")),
            "{:?}",
            report.created
        );
        let text = read(&dir.join("Jenkinsfile"));
        assert!(text.contains(BLOCK_BEGIN), "{text}");
        assert!(text.contains("--format bitbucket-insights"), "{text}");
        // Сеть — только curl в CI-скрипте; путь отчёта помечен проверкой.
        assert!(text.contains("/rest/insights/1.0"), "{text}");
        assert!(text.contains("build-status/1.0/commits"), "{text}");
        assert!(text.contains("Required builds"), "{text}");
        assert!(text.contains("Code Insights"), "{text}");
        assert!(text.contains("[ТРЕБУЕТ ПРОВЕРКИ"), "{text}");

        // Правка вне маркеров + повторный вызов: чужое цело, наш блок заменён.
        let with_own = text.replace(
            "// spine-connect:end",
            "// spine-connect:end\n// МОЙ этап\n",
        );
        std::fs::write(dir.join("Jenkinsfile"), &with_own).expect("write");
        connect_ci(CiProvider::Bitbucket, &dir, false, None).expect("повтор");
        let again = read(&dir.join("Jenkinsfile"));
        assert!(
            again.contains("// МОЙ этап"),
            "правка вне маркеров цела: {again}"
        );
        assert!(again.contains("--format bitbucket-insights"), "{again}");
        assert_eq!(again.matches(BLOCK_BEGIN).count(), 1, "{again}");
    }

    /// K4: джобы gitlab/github/jenkins рендерят `arch-be arch-diff` с
    /// публикацией ОДНОГО обновляемого комментария MR/PR (маркер + edit вместо
    /// new) силами пайплайна (curl/gh), а не бинаря (C-34); Bitbucket — через
    /// Code Insights (D1, блок выше не тронут).
    #[test]
    fn ci_jobs_render_arch_diff_with_updating_comment() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        std::fs::create_dir_all(&dir).expect("mkdir");

        // GitLab: джоба spine-arch-diff, база MR, комментарий через notes API.
        connect_ci(CiProvider::GitLab, &dir, false, None).expect("connect ci gitlab");
        let gl = read(&dir.join(".gitlab-ci.yml"));
        for needle in [
            "spine-arch-diff:",
            "arch-be arch-diff --repo . --base",
            "--format md > arch-diff.md",
            "<!-- spine-arch-diff -->",
            "/merge_requests/${CI_MERGE_REQUEST_IID}/notes",
            "GITLAB_TOKEN",
            "curl -fsS -X PUT",
            "curl -fsS -X POST",
        ] {
            assert!(gl.contains(needle), "gitlab: нет «{needle}»:\n{gl}");
        }
        assert!(
            !gl.contains("...HEAD"),
            "gitlab: база — голая ревизия: {gl}"
        );

        // GitHub: job arch-diff, PR-only, комментарий через gh api.
        connect_ci(CiProvider::GitHub, &dir, false, None).expect("connect ci github");
        let gh = read(&dir.join(".github/workflows/spine-gate.yml"));
        for needle in [
            "arch-diff:",
            "github.event_name == 'pull_request'",
            "pull-requests: write",
            "arch-be arch-diff --repo . --base",
            "--format md > arch-diff.md",
            "<!-- spine-arch-diff -->",
            "GH_TOKEN",
            "gh api -X PATCH",
            "gh api -X POST",
        ] {
            assert!(gh.contains(needle), "github: нет «{needle}»:\n{gh}");
        }

        // Jenkins: этап 'Spine arch-diff', только PR-сборки, публикация curl'ом
        // в endpoint из переменных окружения (форма GitLab/GitHub — комментарий).
        connect_ci(CiProvider::Jenkins, &dir, false, None).expect("connect ci jenkins");
        let jk = read(&dir.join("Jenkinsfile"));
        for needle in [
            "stage('Spine arch-diff')",
            "changeRequest()",
            "arch-be arch-diff --repo . --base",
            "--format md > arch-diff.md",
            "<!-- spine-arch-diff -->",
            "ARCH_DIFF_LIST_URL",
            "ARCH_DIFF_ITEM_URL",
            "METHOD=PUT",
            "METHOD=PATCH",
            "[ТРЕБУЕТ ПРОВЕРКИ",
            "arch-diff-mr-token",
        ] {
            assert!(jk.contains(needle), "jenkins: нет «{needle}»:\n{jk}");
        }

        // В джобах комментария дифф информационный (без --fail-on): краснота
        // MR/PR — у джобы гейта; --fail-on — отдельная опция (docs/arch-diff.md).
        for (name, text) in [("gitlab", &gl), ("github", &gh), ("jenkins", &jk)] {
            for line in text.lines().filter(|l| l.contains("arch-be arch-diff")) {
                assert!(
                    !line.contains("--fail-on"),
                    "{name}: комментарий-джоба обязана быть информационной: {line}"
                );
            }
        }
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

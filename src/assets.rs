//! Встроенные ассеты харнесса (подхватываются `arch-be init`).
//!
//! Каждый ассет — `include_str!` из `assets/` и примеров репозитория;
//! [`write_defaults`] раскладывает их по `~/.arch-harness/` (assets/prompts,
//! assets/rubrics, assets/benchmarks, assets/ascii, mcp.json, cron.toml,
//! CONSTRAINTS.example.yaml, cron/*.md, plugins/**), НЕ затирая существующие
//! файлы, и возвращает список фактически записанных.

use std::path::{Path, PathBuf};

use crate::error::{HarnessError, Result};

/// ASCII-баннер приложения (стартовый экран TUI).
pub const BANNER: &str = include_str!("../assets/ascii/banner.txt");

/// Главный системный промпт solution-архитектора.
pub const PROMPT_ARCHITECT: &str = include_str!("../assets/prompts/architect.md");
/// Промпт фасилитатора ADR.
pub const PROMPT_ADR: &str = include_str!("../assets/prompts/adr.md");
/// Промпт редактора ARCHITECTURE-SPINE.
pub const PROMPT_SPINE: &str = include_str!("../assets/prompts/spine.md");
/// Промпт состязательного ревьюера архитектуры.
pub const PROMPT_REVIEW_ADVERSARIAL: &str = include_str!("../assets/prompts/review_adversarial.md");
/// Промпт readiness-гейта PASS/CONCERNS/FAIL.
pub const PROMPT_READINESS_GATE: &str = include_str!("../assets/prompts/readiness_gate.md");
/// Промпт компилятора handoff-пакетов.
pub const PROMPT_HANDOFF_COMPILE: &str = include_str!("../assets/prompts/handoff_compile.md");
/// Промпт обратного обследования legacy.
pub const PROMPT_REVERSE_DISCOVERY: &str = include_str!("../assets/prompts/reverse_discovery.md");
/// Промпт проектировщика NFR.
pub const PROMPT_NFR_DESIGN: &str = include_str!("../assets/prompts/nfr_design.md");
/// Промпт дистиллятора скиллов (контекст/статья → SKILL.md).
pub const PROMPT_SKILL_DISTILLER: &str = include_str!("../assets/prompts/skill_distiller.md");

/// Якорная рубрика: комплексная оценка документа solution-архитектуры (15 критериев).
pub const RUBRIC_SOLUTION_ARCHITECTURE: &str =
    include_str!("../assets/rubrics/solution_architecture.yaml");
/// Якорная рубрика: контрольные точки A0–A5.
pub const RUBRIC_ARCHITECTURE_GATES: &str =
    include_str!("../assets/rubrics/architecture_gates.yaml");
/// Якорная рубрика: 6 измерений таксономии Macedo.
pub const RUBRIC_MACEDO_DIMENSIONS: &str = include_str!("../assets/rubrics/macedo_dimensions.yaml");
/// Якорная рубрика: качество ADR.
pub const RUBRIC_ADR_QUALITY: &str = include_str!("../assets/rubrics/adr_quality.yaml");
/// Якорная рубрика: качество handoff-пакета.
pub const RUBRIC_HANDOFF_QUALITY: &str = include_str!("../assets/rubrics/handoff_quality.yaml");

/// Смысловая рубрика (ADR-051, волна B): решение против инварианта спайна
/// (класс `D10`). Досье — `adr_vs_spine`.
pub const RUBRIC_ADR_SPINE_CONSISTENCY: &str =
    include_str!("../assets/rubrics/adr_spine_consistency.yaml");
/// Смысловая рубрика (ADR-051, волна B): ссылка «не на ту» сущность
/// (класс `D6`). Досье — `entity_links`.
pub const RUBRIC_MODEL_LINK_SEMANTICS: &str =
    include_str!("../assets/rubrics/model_link_semantics.yaml");
/// Смысловая рубрика (ADR-051, волна B): обещание показателя без механизма,
/// способного его дать. Досье — `nfr_mechanism`.
pub const RUBRIC_NFR_MECHANISM_FIT: &str = include_str!("../assets/rubrics/nfr_mechanism_fit.yaml");
/// Смысловая рубрика (ADR-051, волна B): код против инварианта
/// (класс `D11`). Досье — `code_vs_spine`.
pub const RUBRIC_CODE_INVARIANT_CONFORMANCE: &str =
    include_str!("../assets/rubrics/code_invariant_conformance.yaml");

/// Пара файлов golden-набора смысловых рубрик (ADR-051, S4).
macro_rules! semantic_golden_files {
    ($($path:literal),* $(,)?) => {
        &[$((
            concat!("assets/benchmarks/golden/semantic/", $path),
            include_str!(concat!("../assets/benchmarks/golden/semantic/", $path)),
        )),*]
    };
}

/// Встроенный golden-набор смысловых рубрик: 4 рубрики × 6 досье × (досье +
/// эталон). Отдельным списком, а не строками `DEFAULT_FILES`: набор растёт
/// вместе с числом рубрик и читается одним блоком.
pub const SEMANTIC_GOLDEN: &[(&str, &str)] = semantic_golden_files![
    "adr_spine_consistency/clean-no-invariants-touched.expected.yaml",
    "adr_spine_consistency/clean-no-invariants-touched.md",
    "adr_spine_consistency/clean-plain.expected.yaml",
    "adr_spine_consistency/clean-plain.md",
    "adr_spine_consistency/distributed-contradiction.expected.yaml",
    "adr_spine_consistency/distributed-contradiction.md",
    "adr_spine_consistency/gross-contradiction.expected.yaml",
    "adr_spine_consistency/gross-contradiction.md",
    "adr_spine_consistency/hard-clean-override.expected.yaml",
    "adr_spine_consistency/hard-clean-override.md",
    "adr_spine_consistency/masked-word-in-place.expected.yaml",
    "adr_spine_consistency/masked-word-in-place.md",
    "code_invariant_conformance/clean-comment-and-check.expected.yaml",
    "code_invariant_conformance/clean-comment-and-check.md",
    "code_invariant_conformance/clean-enforced-by-construction.expected.yaml",
    "code_invariant_conformance/clean-enforced-by-construction.md",
    "code_invariant_conformance/distributed-bypass.expected.yaml",
    "code_invariant_conformance/distributed-bypass.md",
    "code_invariant_conformance/gross-direct-write.expected.yaml",
    "code_invariant_conformance/gross-direct-write.md",
    "code_invariant_conformance/hard-clean-legacy-adapter.expected.yaml",
    "code_invariant_conformance/hard-clean-legacy-adapter.md",
    "code_invariant_conformance/masked-debug-branch.expected.yaml",
    "code_invariant_conformance/masked-debug-branch.md",
    "model_link_semantics/clean-links-fit.expected.yaml",
    "model_link_semantics/clean-links-fit.md",
    "model_link_semantics/clean-rule-fits-invariant.expected.yaml",
    "model_link_semantics/clean-rule-fits-invariant.md",
    "model_link_semantics/contract-other-domain.expected.yaml",
    "model_link_semantics/contract-other-domain.md",
    "model_link_semantics/gross-wrong-executor.expected.yaml",
    "model_link_semantics/gross-wrong-executor.md",
    "model_link_semantics/hard-clean-similar-name.expected.yaml",
    "model_link_semantics/hard-clean-similar-name.md",
    "model_link_semantics/masked-plausible-target.expected.yaml",
    "model_link_semantics/masked-plausible-target.md",
    "nfr_mechanism_fit/clean-degradation-defined.expected.yaml",
    "nfr_mechanism_fit/clean-degradation-defined.md",
    "nfr_mechanism_fit/clean-sync-commit.expected.yaml",
    "nfr_mechanism_fit/clean-sync-commit.md",
    "nfr_mechanism_fit/gross-rpo-zero-async.expected.yaml",
    "nfr_mechanism_fit/gross-rpo-zero-async.md",
    "nfr_mechanism_fit/hard-clean-scoped.expected.yaml",
    "nfr_mechanism_fit/hard-clean-scoped.md",
    "nfr_mechanism_fit/masked-availability-composition.expected.yaml",
    "nfr_mechanism_fit/masked-availability-composition.md",
    "nfr_mechanism_fit/verification-checks-presence.expected.yaml",
    "nfr_mechanism_fit/verification-checks-presence.md",
];

/// Бенчмарк: интеграция платёжного шлюза.
pub const BENCH_PAYMENT_INTEGRATION: &str =
    include_str!("../assets/benchmarks/payment_integration.yaml");
/// Бенчмарк: brownfield-декомпозиция монолита.
pub const BENCH_LEGACY_DECOMPOSITION: &str =
    include_str!("../assets/benchmarks/legacy_decomposition.yaml");
/// Бенчмарк: event-driven интеграция доменов.
pub const BENCH_EVENT_DRIVEN_DESIGN: &str =
    include_str!("../assets/benchmarks/event_driven_design.yaml");
/// Бенчмарк: репликация СУБД Pangolin на вторую площадку.
pub const BENCH_PANGOLIN_REPLICATION: &str =
    include_str!("../assets/benchmarks/pangolin_replication.yaml");
/// Бенчмарк: устойчивый интеграционный слой `AIGena → GigaChat API`.
pub const BENCH_GIGACHAT_SLA_RESILIENCE: &str =
    include_str!("../assets/benchmarks/gigachat_sla_resilience.yaml");
/// Бенчмарк: real-time агент над данными МЕТА (ТчВ/ИВ) ≤ 10 секунд.
pub const BENCH_META_AGENT_REALTIME: &str =
    include_str!("../assets/benchmarks/meta_agent_realtime.yaml");

/// Golden-set судьи рубрик (ADR-004): синтетический ADR-образец + эталонные
/// оценки по рубрике `adr_quality`; прогон — `arch-be bench run --golden`.
pub const GOLDEN_ADR_FULL_MD: &str = include_str!("../assets/benchmarks/golden/adr_full.md");
/// Эталон к [`GOLDEN_ADR_FULL_MD`].
pub const GOLDEN_ADR_FULL_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_full.expected.yaml");
/// Golden-set: середняк по всем критериям.
pub const GOLDEN_ADR_DECENT_MD: &str = include_str!("../assets/benchmarks/golden/adr_decent.md");
/// Эталон к [`GOLDEN_ADR_DECENT_MD`].
pub const GOLDEN_ADR_DECENT_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_decent.expected.yaml");
/// Golden-set: сильный контекст без альтернатив.
pub const GOLDEN_ADR_NO_ALTERNATIVES_MD: &str =
    include_str!("../assets/benchmarks/golden/adr_no_alternatives.md");
/// Эталон к [`GOLDEN_ADR_NO_ALTERNATIVES_MD`].
pub const GOLDEN_ADR_NO_ALTERNATIVES_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_no_alternatives.expected.yaml");
/// Golden-set: только плюсы, без отрицательных последствий.
pub const GOLDEN_ADR_NO_NEGATIVES_MD: &str =
    include_str!("../assets/benchmarks/golden/adr_no_negatives.md");
/// Эталон к [`GOLDEN_ADR_NO_NEGATIVES_MD`].
pub const GOLDEN_ADR_NO_NEGATIVES_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_no_negatives.expected.yaml");
/// Golden-set: необратимое решение без оценки обратимости.
pub const GOLDEN_ADR_IRREVERSIBLE_MD: &str =
    include_str!("../assets/benchmarks/golden/adr_irreversible.md");
/// Эталон к [`GOLDEN_ADR_IRREVERSIBLE_MD`].
pub const GOLDEN_ADR_IRREVERSIBLE_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_irreversible.expected.yaml");
/// Golden-set: запись задним числом после реализации.
pub const GOLDEN_ADR_POSTHOC_MD: &str = include_str!("../assets/benchmarks/golden/adr_posthoc.md");
/// Эталон к [`GOLDEN_ADR_POSTHOC_MD`].
pub const GOLDEN_ADR_POSTHOC_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_posthoc.expected.yaml");
/// Golden-set: заглушка из двух предложений (все критерии на 1).
pub const GOLDEN_ADR_STUB_MD: &str = include_str!("../assets/benchmarks/golden/adr_stub.md");
/// Эталон к [`GOLDEN_ADR_STUB_MD`].
pub const GOLDEN_ADR_STUB_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_stub.expected.yaml");
/// Golden-set: адверсариальный анти-length-bias — самый длинный ADR набора,
/// структурно полный, содержательно пустой (ревью 2026-09-04).
pub const GOLDEN_ADR_VERBOSE_BAD_MD: &str =
    include_str!("../assets/benchmarks/golden/adr_verbose_bad.md");
/// Эталон к [`GOLDEN_ADR_VERBOSE_BAD_MD`].
pub const GOLDEN_ADR_VERBOSE_BAD_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_verbose_bad.expected.yaml");
/// Golden-set: адверсариальный анти-length-bias — компактный ADR, полный
/// по всем якорям рубрики.
pub const GOLDEN_ADR_COMPACT_GOOD_MD: &str =
    include_str!("../assets/benchmarks/golden/adr_compact_good.md");
/// Эталон к [`GOLDEN_ADR_COMPACT_GOOD_MD`].
pub const GOLDEN_ADR_COMPACT_GOOD_EXPECTED: &str =
    include_str!("../assets/benchmarks/golden/adr_compact_good.expected.yaml");

/// Образец MCP-серверов (формат Claude Code `mcp.json`).
pub const MCP_SERVERS_EXAMPLE: &str = include_str!("../examples/mcp.example.json");
/// Образец расписания планировщика (`cron.toml`).
pub const CRON_EXAMPLE: &str = include_str!("../cron.example.toml");
/// Образец fitness-правил (`CONSTRAINTS.yaml`).
pub const CONSTRAINTS_EXAMPLE: &str = include_str!("../examples/CONSTRAINTS.example.yaml");
/// Md-инструкция cron-задачи «дайджест базы знаний».
pub const CRON_TASK_KB_DIGEST: &str = include_str!("../examples/cron/kb_digest.md");
/// Md-инструкция cron-задачи «дрейф-чек спек».
pub const CRON_TASK_SPEC_DRIFT: &str = include_str!("../examples/cron/spec_drift.md");

// ── Eval-сьют конфигурации харнесса (continuous evals, docs/evals.md) ──

/// Eval-задача: реестр моделей отвечает.
pub const EVAL_AGENT_CONFIG_MODELS_REGISTRY: &str =
    include_str!("../assets/evals/agent-config/01-models-registry.yaml");
/// Eval-задача: библиотека скиллов читается.
pub const EVAL_AGENT_CONFIG_SKILLS_LIBRARY: &str =
    include_str!("../assets/evals/agent-config/02-skills-library.yaml");
/// Eval-задача: якорные рубрики перечисляются.
pub const EVAL_AGENT_CONFIG_RUBRICS_LIBRARY: &str =
    include_str!("../assets/evals/agent-config/03-rubrics-library.yaml");
/// Eval-задача: каталог бенчмарков читается.
pub const EVAL_AGENT_CONFIG_BENCH_CATALOG: &str =
    include_str!("../assets/evals/agent-config/04-bench-catalog.yaml");
/// Eval-задача: mermaid-рендер из stdin.
pub const EVAL_AGENT_CONFIG_MERMAID_RENDER: &str =
    include_str!("../assets/evals/agent-config/05-mermaid-render.yaml");
/// Eval-задача: significance score → Fast.
pub const EVAL_AGENT_CONFIG_CONTROL_SCORE_FAST: &str =
    include_str!("../assets/evals/agent-config/06-control-score-fast.yaml");
/// Eval-задача: significance score → Critical.
pub const EVAL_AGENT_CONFIG_CONTROL_SCORE_CRITICAL: &str =
    include_str!("../assets/evals/agent-config/07-control-score-critical.yaml");
/// Eval-задача: политика R2 отклоняет деструктив.
pub const EVAL_AGENT_CONFIG_POLICY_DENIES_DESTRUCTIVE: &str =
    include_str!("../assets/evals/agent-config/08-policy-denies-destructive.yaml");

// ── Плагины (agent-plugins.org): скиллы + MCP + субагенты + хуки ──

/// Встроенный файл плагина `assets/plugins/arch-core/plugin.json`.
pub const PLUGIN_ARCH_CORE_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/arch-core/plugin.json");
/// Встроенный файл плагина `assets/plugins/arch-core/agents/adr-reviewer.md`.
pub const PLUGIN_ARCH_CORE_AGENTS_ADR_REVIEWER_MD: &str =
    include_str!("../assets/plugins/arch-core/agents/adr-reviewer.md");
/// Встроенный файл плагина `assets/plugins/arch-core/agents/nfr-auditor.md`.
pub const PLUGIN_ARCH_CORE_AGENTS_NFR_AUDITOR_MD: &str =
    include_str!("../assets/plugins/arch-core/agents/nfr-auditor.md");
/// Встроенный файл плагина `assets/plugins/arch-core/hooks/hooks.json`.
pub const PLUGIN_ARCH_CORE_HOOKS_HOOKS_JSON: &str =
    include_str!("../assets/plugins/arch-core/hooks/hooks.json");
/// Встроенный файл плагина `assets/plugins/arch-core/mcp.json`.
pub const PLUGIN_ARCH_CORE_MCP_JSON: &str = include_str!("../assets/plugins/arch-core/mcp.json");
/// Встроенный файл плагина `assets/plugins/arch-office/agents/report-proofreader.md`.
pub const PLUGIN_ARCH_OFFICE_AGENTS_REPORT_PROOFREADER_MD: &str =
    include_str!("../assets/plugins/arch-office/agents/report-proofreader.md");
/// Встроенный файл плагина `assets/plugins/arch-office/mcp.json`.
pub const PLUGIN_ARCH_OFFICE_MCP_JSON: &str =
    include_str!("../assets/plugins/arch-office/mcp.json");
/// Встроенный файл плагина `assets/plugins/arch-core/agents/repo-scout.md`.
pub const PLUGIN_ARCH_CORE_AGENTS_REPO_SCOUT_MD: &str =
    include_str!("../assets/plugins/arch-core/agents/repo-scout.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/adr-authoring/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_ADR_AUTHORING_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/adr-authoring/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/adr-authoring/references/adr-template.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_ADR_AUTHORING_REFERENCES_ADR_TEMPLATE_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/adr-authoring/references/adr-template.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/adversarial-review/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_ADVERSARIAL_REVIEW_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/adversarial-review/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/c4-mermaid/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_C4_MERMAID_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/c4-mermaid/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/delta-spec/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_DELTA_SPEC_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/delta-spec/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/dsh-harness-patterns/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_DSH_HARNESS_PATTERNS_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/dsh-harness-patterns/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/fitness-functions/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_FITNESS_FUNCTIONS_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/fitness-functions/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/handoff-packaging/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_HANDOFF_PACKAGING_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/handoff-packaging/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/nfr-design/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_NFR_DESIGN_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/nfr-design/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/readiness-gate/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_READINESS_GATE_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/readiness-gate/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/reverse-discovery/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_REVERSE_DISCOVERY_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/reverse-discovery/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/rubric-judging/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_RUBRIC_JUDGING_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/rubric-judging/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/significance-routing/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_SIGNIFICANCE_ROUTING_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/significance-routing/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/skill-authoring/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_SKILL_AUTHORING_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/skill-authoring/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/spine-invariants/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_SPINE_INVARIANTS_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/spine-invariants/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-core/skills/spine-invariants/references/spine-template.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_SPINE_INVARIANTS_REFERENCES_SPINE_TEMPLATE_MD: &str = include_str!(
    "../assets/plugins/arch-core/skills/spine-invariants/references/spine-template.md"
);
/// Встроенный файл плагина `assets/plugins/patterns-integration/agents/pattern-selector.md`.
pub const PLUGIN_PATTERNS_INTEGRATION_AGENTS_PATTERN_SELECTOR_MD: &str =
    include_str!("../assets/plugins/patterns-integration/agents/pattern-selector.md");
/// Встроенный файл плагина `assets/plugins/patterns-integration/hooks/hooks.json`.
pub const PLUGIN_PATTERNS_INTEGRATION_HOOKS_HOOKS_JSON: &str =
    include_str!("../assets/plugins/patterns-integration/hooks/hooks.json");
/// Встроенный файл плагина `assets/plugins/patterns-integration/mcp.json`.
pub const PLUGIN_PATTERNS_INTEGRATION_MCP_JSON: &str =
    include_str!("../assets/plugins/patterns-integration/mcp.json");
/// Встроенный файл плагина `assets/plugins/patterns-integration/plugin.json`.
pub const PLUGIN_PATTERNS_INTEGRATION_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/patterns-integration/plugin.json");
/// Встроенный файл плагина `assets/plugins/patterns-integration/skills/cqrs-api-composition/SKILL.md`.
pub const PLUGIN_PATTERNS_INTEGRATION_SKILLS_CQRS_API_COMPOSITION_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-integration/skills/cqrs-api-composition/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-integration/skills/idempotent-consumer/SKILL.md`.
pub const PLUGIN_PATTERNS_INTEGRATION_SKILLS_IDEMPOTENT_CONSUMER_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-integration/skills/idempotent-consumer/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-integration/skills/saga-transactions/SKILL.md`.
pub const PLUGIN_PATTERNS_INTEGRATION_SKILLS_SAGA_TRANSACTIONS_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-integration/skills/saga-transactions/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-integration/skills/strangler-acl/SKILL.md`.
pub const PLUGIN_PATTERNS_INTEGRATION_SKILLS_STRANGLER_ACL_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-integration/skills/strangler-acl/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-integration/skills/transactional-outbox/SKILL.md`.
pub const PLUGIN_PATTERNS_INTEGRATION_SKILLS_TRANSACTIONAL_OUTBOX_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-integration/skills/transactional-outbox/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/agents/resilience-auditor.md`.
pub const PLUGIN_PATTERNS_RESILIENCE_AGENTS_RESILIENCE_AUDITOR_MD: &str =
    include_str!("../assets/plugins/patterns-resilience/agents/resilience-auditor.md");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/hooks/hooks.json`.
pub const PLUGIN_PATTERNS_RESILIENCE_HOOKS_HOOKS_JSON: &str =
    include_str!("../assets/plugins/patterns-resilience/hooks/hooks.json");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/mcp.json`.
pub const PLUGIN_PATTERNS_RESILIENCE_MCP_JSON: &str =
    include_str!("../assets/plugins/patterns-resilience/mcp.json");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/plugin.json`.
pub const PLUGIN_PATTERNS_RESILIENCE_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/patterns-resilience/plugin.json");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/skills/bulkhead/SKILL.md`.
pub const PLUGIN_PATTERNS_RESILIENCE_SKILLS_BULKHEAD_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-resilience/skills/bulkhead/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/skills/cache-aside/SKILL.md`.
pub const PLUGIN_PATTERNS_RESILIENCE_SKILLS_CACHE_ASIDE_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-resilience/skills/cache-aside/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/skills/circuit-breaker-retry/SKILL.md`.
pub const PLUGIN_PATTERNS_RESILIENCE_SKILLS_CIRCUIT_BREAKER_RETRY_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-resilience/skills/circuit-breaker-retry/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/skills/queue-load-leveling/SKILL.md`.
pub const PLUGIN_PATTERNS_RESILIENCE_SKILLS_QUEUE_LOAD_LEVELING_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-resilience/skills/queue-load-leveling/SKILL.md");
/// Встроенный файл плагина `assets/plugins/patterns-resilience/skills/rate-limiting-throttling/SKILL.md`.
pub const PLUGIN_PATTERNS_RESILIENCE_SKILLS_RATE_LIMITING_THROTTLING_SKILL_MD: &str =
    include_str!("../assets/plugins/patterns-resilience/skills/rate-limiting-throttling/SKILL.md");

/// Встроенный файл плагина `assets/plugins/arch-office/plugin.json`.
pub const PLUGIN_ARCH_OFFICE_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/arch-office/plugin.json");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-architecture-vision/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_ARCHITECTURE_VISION_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/docx-architecture-vision/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-architecture-vision/references/docx_architecture_vision_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_ARCHITECTURE_VISION_REFERENCES_DOCX_ARCHITECTURE_VISION_GEN_PY: &str = include_str!("../assets/plugins/arch-office/skills/docx-architecture-vision/references/docx_architecture_vision_gen.py");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-current-state-assessment/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_CURRENT_STATE_ASSESSMENT_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/docx-current-state-assessment/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-current-state-assessment/references/docx_current_state_assessment_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_CURRENT_STATE_ASSESSMENT_REFERENCES_DOCX_CURRENT_STATE_ASSESSMENT_GEN_PY: &str = include_str!("../assets/plugins/arch-office/skills/docx-current-state-assessment/references/docx_current_state_assessment_gen.py");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-integration-spec/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_INTEGRATION_SPEC_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/docx-integration-spec/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-integration-spec/references/docx_integration_spec_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_INTEGRATION_SPEC_REFERENCES_DOCX_INTEGRATION_SPEC_GEN_PY: &str = include_str!("../assets/plugins/arch-office/skills/docx-integration-spec/references/docx_integration_spec_gen.py");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-migration-roadmap/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_MIGRATION_ROADMAP_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/docx-migration-roadmap/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-migration-roadmap/references/docx_migration_roadmap_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_MIGRATION_ROADMAP_REFERENCES_DOCX_MIGRATION_ROADMAP_GEN_PY: &str = include_str!("../assets/plugins/arch-office/skills/docx-migration-roadmap/references/docx_migration_roadmap_gen.py");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-research-report/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_RESEARCH_REPORT_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/docx-research-report/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-research-report/references/docx_research_report_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_RESEARCH_REPORT_REFERENCES_DOCX_RESEARCH_REPORT_GEN_PY:
    &str = include_str!(
    "../assets/plugins/arch-office/skills/docx-research-report/references/docx_research_report_gen.py"
);
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-solution-design/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_SOLUTION_DESIGN_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/docx-solution-design/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/docx-solution-design/references/docx_solution_design_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_DOCX_SOLUTION_DESIGN_REFERENCES_DOCX_SOLUTION_DESIGN_GEN_PY:
    &str = include_str!(
    "../assets/plugins/arch-office/skills/docx-solution-design/references/docx_solution_design_gen.py"
);
/// Встроенный файл плагина `assets/plugins/arch-office/skills/pptx-architecture-review/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_PPTX_ARCHITECTURE_REVIEW_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/pptx-architecture-review/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/pptx-architecture-review/references/pptx_architecture_review_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_PPTX_ARCHITECTURE_REVIEW_REFERENCES_PPTX_ARCHITECTURE_REVIEW_GEN_PY: &str = include_str!("../assets/plugins/arch-office/skills/pptx-architecture-review/references/pptx_architecture_review_gen.py");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/pptx-board-deck/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_PPTX_BOARD_DECK_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/pptx-board-deck/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/pptx-board-deck/references/pptx_board_deck_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_PPTX_BOARD_DECK_REFERENCES_PPTX_BOARD_DECK_GEN_PY: &str = include_str!(
    "../assets/plugins/arch-office/skills/pptx-board-deck/references/pptx_board_deck_gen.py"
);
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-decision-matrix/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_DECISION_MATRIX_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/xlsx-decision-matrix/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-decision-matrix/references/xlsx_decision_matrix_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_DECISION_MATRIX_REFERENCES_XLSX_DECISION_MATRIX_GEN_PY:
    &str = include_str!(
    "../assets/plugins/arch-office/skills/xlsx-decision-matrix/references/xlsx_decision_matrix_gen.py"
);
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-integration-matrix/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_INTEGRATION_MATRIX_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/xlsx-integration-matrix/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-integration-matrix/references/xlsx_integration_matrix_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_INTEGRATION_MATRIX_REFERENCES_XLSX_INTEGRATION_MATRIX_GEN_PY: &str = include_str!("../assets/plugins/arch-office/skills/xlsx-integration-matrix/references/xlsx_integration_matrix_gen.py");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-risk-register/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_RISK_REGISTER_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/xlsx-risk-register/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-risk-register/references/xlsx_risk_register_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_RISK_REGISTER_REFERENCES_XLSX_RISK_REGISTER_GEN_PY: &str = include_str!(
    "../assets/plugins/arch-office/skills/xlsx-risk-register/references/xlsx_risk_register_gen.py"
);
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-system-catalog/SKILL.md`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_SYSTEM_CATALOG_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-office/skills/xlsx-system-catalog/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-office/skills/xlsx-system-catalog/references/xlsx_system_catalog_gen.py`.
pub const PLUGIN_ARCH_OFFICE_SKILLS_XLSX_SYSTEM_CATALOG_REFERENCES_XLSX_SYSTEM_CATALOG_GEN_PY:
    &str = include_str!(
    "../assets/plugins/arch-office/skills/xlsx-system-catalog/references/xlsx_system_catalog_gen.py"
);

/// Встроенный файл `assets/plugins/arch-core/skills/agents-md-authoring/SKILL.md`.
pub const PLUGIN_ARCH_CORE_SKILLS_AGENTS_MD_AUTHORING_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-core/skills/agents-md-authoring/SKILL.md");
/// Встроенный файл `assets/rubrics/agents_md_quality.yaml`.
pub const RUBRIC_AGENTS_MD_QUALITY_YAML: &str =
    include_str!("../assets/rubrics/agents_md_quality.yaml");
/// Встроенный файл `examples/cron/agents_md_drift.md`.
pub const CRON_TASK_AGENTS_MD_DRIFT_MD: &str = include_str!("../examples/cron/agents_md_drift.md");

/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/agents/agentic-architect.md`.
pub const PLUGIN_AWS_AGENTIC_AI_AGENTS_AGENTIC_ARCHITECT_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/agents/agentic-architect.md");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/hooks/hooks.json`.
pub const PLUGIN_AWS_AGENTIC_AI_HOOKS_HOOKS_JSON: &str =
    include_str!("../assets/plugins/aws-agentic-ai/hooks/hooks.json");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/mcp.json`.
pub const PLUGIN_AWS_AGENTIC_AI_MCP_JSON: &str =
    include_str!("../assets/plugins/aws-agentic-ai/mcp.json");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/plugin.json`.
pub const PLUGIN_AWS_AGENTIC_AI_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/aws-agentic-ai/plugin.json");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/references/guide-map.md`.
pub const PLUGIN_AWS_AGENTIC_AI_REFERENCES_GUIDE_MAP_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/references/guide-map.md");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/skills/agent-patterns-overview/SKILL.md`.
pub const PLUGIN_AWS_AGENTIC_AI_SKILLS_AGENT_PATTERNS_OVERVIEW_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/skills/agent-patterns-overview/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/skills/llm-workflow-patterns/SKILL.md`.
pub const PLUGIN_AWS_AGENTIC_AI_SKILLS_LLM_WORKFLOW_PATTERNS_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/skills/llm-workflow-patterns/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/skills/multi-agent-collaboration/SKILL.md`.
pub const PLUGIN_AWS_AGENTIC_AI_SKILLS_MULTI_AGENT_COLLABORATION_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/skills/multi-agent-collaboration/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/skills/reflect-refine-loops/SKILL.md`.
pub const PLUGIN_AWS_AGENTIC_AI_SKILLS_REFLECT_REFINE_LOOPS_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/skills/reflect-refine-loops/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-agentic-ai/skills/saga-orchestration-agents/SKILL.md`.
pub const PLUGIN_AWS_AGENTIC_AI_SKILLS_SAGA_ORCHESTRATION_AGENTS_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-agentic-ai/skills/saga-orchestration-agents/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/agents/distributed-systems-reviewer.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_AGENTS_DISTRIBUTED_SYSTEMS_REVIEWER_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/agents/distributed-systems-reviewer.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/hooks/hooks.json`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_HOOKS_HOOKS_JSON: &str =
    include_str!("../assets/plugins/aws-builders-library/hooks/hooks.json");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/mcp.json`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_MCP_JSON: &str =
    include_str!("../assets/plugins/aws-builders-library/mcp.json");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/plugin.json`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/aws-builders-library/plugin.json");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/references/catalog.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_REFERENCES_CATALOG_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/references/catalog.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/avoiding-fallback/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_AVOIDING_FALLBACK_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/avoiding-fallback/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/control-data-plane/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_CONTROL_DATA_PLANE_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/control-data-plane/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/eight-failure-modes/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_EIGHT_FAILURE_MODES_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/eight-failure-modes/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/fairness-admission-control/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_FAIRNESS_ADMISSION_CONTROL_SKILL_MD: &str = include_str!(
    "../assets/plugins/aws-builders-library/skills/fairness-admission-control/SKILL.md"
);
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/leader-election/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_LEADER_ELECTION_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/leader-election/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/load-shedding/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_LOAD_SHEDDING_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/load-shedding/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/queue-backlogs/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_QUEUE_BACKLOGS_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/queue-backlogs/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/static-stability/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_STATIC_STABILITY_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/static-stability/SKILL.md");
/// Встроенный файл плагина `assets/plugins/aws-builders-library/skills/timeouts-backoff-jitter/SKILL.md`.
pub const PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_TIMEOUTS_BACKOFF_JITTER_SKILL_MD: &str =
    include_str!("../assets/plugins/aws-builders-library/skills/timeouts-backoff-jitter/SKILL.md");
/// Встроенный файл плагина `assets/plugins/spine-be-docs/plugin.json`.
pub const PLUGIN_SPINE_BE_DOCS_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/spine-be-docs/plugin.json");
/// Встроенный файл плагина `assets/plugins/spine-be-docs/skills/check-spine-be-docs/SKILL.md`.
pub const PLUGIN_SPINE_BE_DOCS_SKILLS_CHECK_SPINE_BE_DOCS_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-be-docs/skills/check-spine-be-docs/SKILL.md");

/// Встроенный файл плагина `assets/plugins/arch-governance/plugin.json`.
pub const PLUGIN_ARCH_GOVERNANCE_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/arch-governance/plugin.json");
/// Встроенный файл плагина `assets/plugins/arch-governance/skills/architecture-sources-map/SKILL.md`.
pub const PLUGIN_ARCH_GOVERNANCE_SKILLS_ARCHITECTURE_SOURCES_MAP_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-governance/skills/architecture-sources-map/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-governance/skills/fitness-function-catalog/SKILL.md`.
pub const PLUGIN_ARCH_GOVERNANCE_SKILLS_FITNESS_FUNCTION_CATALOG_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-governance/skills/fitness-function-catalog/SKILL.md");
/// Встроенный файл плагина `assets/plugins/arch-governance/skills/rule-library-antipatterns/SKILL.md`.
pub const PLUGIN_ARCH_GOVERNANCE_SKILLS_RULE_LIBRARY_ANTIPATTERNS_SKILL_MD: &str =
    include_str!("../assets/plugins/arch-governance/skills/rule-library-antipatterns/SKILL.md");

// ── Плагин spine-workflows: плейбуки работы со Spine из чужого харнесса ──

/// Встроенный файл плагина `assets/plugins/spine-workflows/plugin.json`.
pub const PLUGIN_SPINE_WORKFLOWS_PLUGIN_JSON: &str =
    include_str!("../assets/plugins/spine-workflows/plugin.json");
/// Встроенный файл `skills/spine-quickstart/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_QUICKSTART_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-quickstart/SKILL.md");
/// Встроенный файл `skills/spine-bundle/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_BUNDLE_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-bundle/SKILL.md");
/// Встроенный файл `skills/spine-fitness-gate/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_FITNESS_GATE_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-fitness-gate/SKILL.md");
/// Встроенный файл `skills/spine-architect-review/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ARCHITECT_REVIEW_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-architect-review/SKILL.md");
/// Встроенный файл `skills/spine-adr-judge/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ADR_JUDGE_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-adr-judge/SKILL.md");
/// Встроенный файл `skills/spine-contracts-gate/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_CONTRACTS_GATE_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-contracts-gate/SKILL.md");
/// Встроенный файл `skills/spine-content-bootstrap/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_CONTENT_BOOTSTRAP_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-content-bootstrap/SKILL.md");
/// Встроенный файл `skills/spine-archify-viz/SKILL.md`.
pub const PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ARCHIFY_VIZ_SKILL_MD: &str =
    include_str!("../assets/plugins/spine-workflows/skills/spine-archify-viz/SKILL.md");

/// Файлы плагинов: «относительный путь в домашнем каталоге → содержимое».
const PLUGIN_FILES: &[(&str, &str)] = &[
    ("plugins/arch-core/plugin.json", PLUGIN_ARCH_CORE_PLUGIN_JSON),
    ("plugins/arch-core/agents/repo-scout.md", PLUGIN_ARCH_CORE_AGENTS_REPO_SCOUT_MD),
    ("plugins/arch-core/agents/adr-reviewer.md", PLUGIN_ARCH_CORE_AGENTS_ADR_REVIEWER_MD),
    ("plugins/arch-core/agents/nfr-auditor.md", PLUGIN_ARCH_CORE_AGENTS_NFR_AUDITOR_MD),
    ("plugins/arch-core/hooks/hooks.json", PLUGIN_ARCH_CORE_HOOKS_HOOKS_JSON),
    ("plugins/arch-core/mcp.json", PLUGIN_ARCH_CORE_MCP_JSON),
    ("plugins/arch-office/agents/report-proofreader.md", PLUGIN_ARCH_OFFICE_AGENTS_REPORT_PROOFREADER_MD),
    ("plugins/arch-office/mcp.json", PLUGIN_ARCH_OFFICE_MCP_JSON),
    ("plugins/arch-core/skills/adr-authoring/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_ADR_AUTHORING_SKILL_MD),
    ("plugins/arch-core/skills/adr-authoring/references/adr-template.md", PLUGIN_ARCH_CORE_SKILLS_ADR_AUTHORING_REFERENCES_ADR_TEMPLATE_MD),
    ("plugins/arch-core/skills/adversarial-review/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_ADVERSARIAL_REVIEW_SKILL_MD),
    ("plugins/arch-core/skills/c4-mermaid/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_C4_MERMAID_SKILL_MD),
    ("plugins/arch-core/skills/delta-spec/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_DELTA_SPEC_SKILL_MD),
    ("plugins/arch-core/skills/dsh-harness-patterns/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_DSH_HARNESS_PATTERNS_SKILL_MD),
    ("plugins/arch-core/skills/fitness-functions/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_FITNESS_FUNCTIONS_SKILL_MD),
    ("plugins/arch-core/skills/handoff-packaging/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_HANDOFF_PACKAGING_SKILL_MD),
    ("plugins/arch-core/skills/nfr-design/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_NFR_DESIGN_SKILL_MD),
    ("plugins/arch-core/skills/readiness-gate/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_READINESS_GATE_SKILL_MD),
    ("plugins/arch-core/skills/reverse-discovery/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_REVERSE_DISCOVERY_SKILL_MD),
    ("plugins/arch-core/skills/rubric-judging/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_RUBRIC_JUDGING_SKILL_MD),
    ("plugins/arch-core/skills/significance-routing/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_SIGNIFICANCE_ROUTING_SKILL_MD),
    ("plugins/arch-core/skills/skill-authoring/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_SKILL_AUTHORING_SKILL_MD),
    ("plugins/arch-core/skills/spine-invariants/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_SPINE_INVARIANTS_SKILL_MD),
    ("plugins/arch-core/skills/spine-invariants/references/spine-template.md", PLUGIN_ARCH_CORE_SKILLS_SPINE_INVARIANTS_REFERENCES_SPINE_TEMPLATE_MD),
    ("plugins/patterns-integration/agents/pattern-selector.md", PLUGIN_PATTERNS_INTEGRATION_AGENTS_PATTERN_SELECTOR_MD),
    ("plugins/patterns-integration/hooks/hooks.json", PLUGIN_PATTERNS_INTEGRATION_HOOKS_HOOKS_JSON),
    ("plugins/patterns-integration/mcp.json", PLUGIN_PATTERNS_INTEGRATION_MCP_JSON),
    ("plugins/patterns-integration/plugin.json", PLUGIN_PATTERNS_INTEGRATION_PLUGIN_JSON),
    ("plugins/patterns-integration/skills/cqrs-api-composition/SKILL.md", PLUGIN_PATTERNS_INTEGRATION_SKILLS_CQRS_API_COMPOSITION_SKILL_MD),
    ("plugins/patterns-integration/skills/idempotent-consumer/SKILL.md", PLUGIN_PATTERNS_INTEGRATION_SKILLS_IDEMPOTENT_CONSUMER_SKILL_MD),
    ("plugins/patterns-integration/skills/saga-transactions/SKILL.md", PLUGIN_PATTERNS_INTEGRATION_SKILLS_SAGA_TRANSACTIONS_SKILL_MD),
    ("plugins/patterns-integration/skills/strangler-acl/SKILL.md", PLUGIN_PATTERNS_INTEGRATION_SKILLS_STRANGLER_ACL_SKILL_MD),
    ("plugins/patterns-integration/skills/transactional-outbox/SKILL.md", PLUGIN_PATTERNS_INTEGRATION_SKILLS_TRANSACTIONAL_OUTBOX_SKILL_MD),
    ("plugins/patterns-resilience/agents/resilience-auditor.md", PLUGIN_PATTERNS_RESILIENCE_AGENTS_RESILIENCE_AUDITOR_MD),
    ("plugins/patterns-resilience/hooks/hooks.json", PLUGIN_PATTERNS_RESILIENCE_HOOKS_HOOKS_JSON),
    ("plugins/patterns-resilience/mcp.json", PLUGIN_PATTERNS_RESILIENCE_MCP_JSON),
    ("plugins/patterns-resilience/plugin.json", PLUGIN_PATTERNS_RESILIENCE_PLUGIN_JSON),
    ("plugins/patterns-resilience/skills/bulkhead/SKILL.md", PLUGIN_PATTERNS_RESILIENCE_SKILLS_BULKHEAD_SKILL_MD),
    ("plugins/patterns-resilience/skills/cache-aside/SKILL.md", PLUGIN_PATTERNS_RESILIENCE_SKILLS_CACHE_ASIDE_SKILL_MD),
    ("plugins/patterns-resilience/skills/circuit-breaker-retry/SKILL.md", PLUGIN_PATTERNS_RESILIENCE_SKILLS_CIRCUIT_BREAKER_RETRY_SKILL_MD),
    ("plugins/patterns-resilience/skills/queue-load-leveling/SKILL.md", PLUGIN_PATTERNS_RESILIENCE_SKILLS_QUEUE_LOAD_LEVELING_SKILL_MD),
    ("plugins/patterns-resilience/skills/rate-limiting-throttling/SKILL.md", PLUGIN_PATTERNS_RESILIENCE_SKILLS_RATE_LIMITING_THROTTLING_SKILL_MD),
    ("plugins/arch-core/skills/agents-md-authoring/SKILL.md", PLUGIN_ARCH_CORE_SKILLS_AGENTS_MD_AUTHORING_SKILL_MD),
    ("plugins/aws-agentic-ai/agents/agentic-architect.md", PLUGIN_AWS_AGENTIC_AI_AGENTS_AGENTIC_ARCHITECT_MD),
    ("plugins/aws-agentic-ai/hooks/hooks.json", PLUGIN_AWS_AGENTIC_AI_HOOKS_HOOKS_JSON),
    ("plugins/aws-agentic-ai/mcp.json", PLUGIN_AWS_AGENTIC_AI_MCP_JSON),
    ("plugins/aws-agentic-ai/plugin.json", PLUGIN_AWS_AGENTIC_AI_PLUGIN_JSON),
    ("plugins/aws-agentic-ai/references/guide-map.md", PLUGIN_AWS_AGENTIC_AI_REFERENCES_GUIDE_MAP_MD),
    ("plugins/aws-agentic-ai/skills/agent-patterns-overview/SKILL.md", PLUGIN_AWS_AGENTIC_AI_SKILLS_AGENT_PATTERNS_OVERVIEW_SKILL_MD),
    ("plugins/aws-agentic-ai/skills/llm-workflow-patterns/SKILL.md", PLUGIN_AWS_AGENTIC_AI_SKILLS_LLM_WORKFLOW_PATTERNS_SKILL_MD),
    ("plugins/aws-agentic-ai/skills/multi-agent-collaboration/SKILL.md", PLUGIN_AWS_AGENTIC_AI_SKILLS_MULTI_AGENT_COLLABORATION_SKILL_MD),
    ("plugins/aws-agentic-ai/skills/reflect-refine-loops/SKILL.md", PLUGIN_AWS_AGENTIC_AI_SKILLS_REFLECT_REFINE_LOOPS_SKILL_MD),
    ("plugins/aws-agentic-ai/skills/saga-orchestration-agents/SKILL.md", PLUGIN_AWS_AGENTIC_AI_SKILLS_SAGA_ORCHESTRATION_AGENTS_SKILL_MD),
    ("plugins/aws-builders-library/agents/distributed-systems-reviewer.md", PLUGIN_AWS_BUILDERS_LIBRARY_AGENTS_DISTRIBUTED_SYSTEMS_REVIEWER_MD),
    ("plugins/aws-builders-library/hooks/hooks.json", PLUGIN_AWS_BUILDERS_LIBRARY_HOOKS_HOOKS_JSON),
    ("plugins/aws-builders-library/mcp.json", PLUGIN_AWS_BUILDERS_LIBRARY_MCP_JSON),
    ("plugins/aws-builders-library/plugin.json", PLUGIN_AWS_BUILDERS_LIBRARY_PLUGIN_JSON),
    ("plugins/aws-builders-library/references/catalog.md", PLUGIN_AWS_BUILDERS_LIBRARY_REFERENCES_CATALOG_MD),
    ("plugins/aws-builders-library/skills/avoiding-fallback/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_AVOIDING_FALLBACK_SKILL_MD),
    ("plugins/aws-builders-library/skills/control-data-plane/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_CONTROL_DATA_PLANE_SKILL_MD),
    ("plugins/aws-builders-library/skills/eight-failure-modes/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_EIGHT_FAILURE_MODES_SKILL_MD),
    ("plugins/aws-builders-library/skills/fairness-admission-control/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_FAIRNESS_ADMISSION_CONTROL_SKILL_MD),
    ("plugins/aws-builders-library/skills/leader-election/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_LEADER_ELECTION_SKILL_MD),
    ("plugins/aws-builders-library/skills/load-shedding/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_LOAD_SHEDDING_SKILL_MD),
    ("plugins/aws-builders-library/skills/queue-backlogs/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_QUEUE_BACKLOGS_SKILL_MD),
    ("plugins/aws-builders-library/skills/static-stability/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_STATIC_STABILITY_SKILL_MD),
    ("plugins/aws-builders-library/skills/timeouts-backoff-jitter/SKILL.md", PLUGIN_AWS_BUILDERS_LIBRARY_SKILLS_TIMEOUTS_BACKOFF_JITTER_SKILL_MD),
("plugins/arch-office/plugin.json", PLUGIN_ARCH_OFFICE_PLUGIN_JSON),
    ("plugins/arch-office/skills/docx-architecture-vision/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_ARCHITECTURE_VISION_SKILL_MD),
    ("plugins/arch-office/skills/docx-architecture-vision/references/docx_architecture_vision_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_ARCHITECTURE_VISION_REFERENCES_DOCX_ARCHITECTURE_VISION_GEN_PY),
    ("plugins/arch-office/skills/docx-current-state-assessment/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_CURRENT_STATE_ASSESSMENT_SKILL_MD),
    ("plugins/arch-office/skills/docx-current-state-assessment/references/docx_current_state_assessment_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_CURRENT_STATE_ASSESSMENT_REFERENCES_DOCX_CURRENT_STATE_ASSESSMENT_GEN_PY),
    ("plugins/arch-office/skills/docx-integration-spec/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_INTEGRATION_SPEC_SKILL_MD),
    ("plugins/arch-office/skills/docx-integration-spec/references/docx_integration_spec_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_INTEGRATION_SPEC_REFERENCES_DOCX_INTEGRATION_SPEC_GEN_PY),
    ("plugins/arch-office/skills/docx-migration-roadmap/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_MIGRATION_ROADMAP_SKILL_MD),
    ("plugins/arch-office/skills/docx-migration-roadmap/references/docx_migration_roadmap_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_MIGRATION_ROADMAP_REFERENCES_DOCX_MIGRATION_ROADMAP_GEN_PY),
    ("plugins/arch-office/skills/docx-research-report/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_RESEARCH_REPORT_SKILL_MD),
    ("plugins/arch-office/skills/docx-research-report/references/docx_research_report_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_RESEARCH_REPORT_REFERENCES_DOCX_RESEARCH_REPORT_GEN_PY),
    ("plugins/arch-office/skills/docx-solution-design/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_SOLUTION_DESIGN_SKILL_MD),
    ("plugins/arch-office/skills/docx-solution-design/references/docx_solution_design_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_DOCX_SOLUTION_DESIGN_REFERENCES_DOCX_SOLUTION_DESIGN_GEN_PY),
    ("plugins/arch-office/skills/pptx-architecture-review/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_PPTX_ARCHITECTURE_REVIEW_SKILL_MD),
    ("plugins/arch-office/skills/pptx-architecture-review/references/pptx_architecture_review_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_PPTX_ARCHITECTURE_REVIEW_REFERENCES_PPTX_ARCHITECTURE_REVIEW_GEN_PY),
    ("plugins/arch-office/skills/pptx-board-deck/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_PPTX_BOARD_DECK_SKILL_MD),
    ("plugins/arch-office/skills/pptx-board-deck/references/pptx_board_deck_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_PPTX_BOARD_DECK_REFERENCES_PPTX_BOARD_DECK_GEN_PY),
    ("plugins/arch-office/skills/xlsx-decision-matrix/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_DECISION_MATRIX_SKILL_MD),
    ("plugins/arch-office/skills/xlsx-decision-matrix/references/xlsx_decision_matrix_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_DECISION_MATRIX_REFERENCES_XLSX_DECISION_MATRIX_GEN_PY),
    ("plugins/arch-office/skills/xlsx-integration-matrix/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_INTEGRATION_MATRIX_SKILL_MD),
    ("plugins/arch-office/skills/xlsx-integration-matrix/references/xlsx_integration_matrix_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_INTEGRATION_MATRIX_REFERENCES_XLSX_INTEGRATION_MATRIX_GEN_PY),
    ("plugins/arch-office/skills/xlsx-risk-register/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_RISK_REGISTER_SKILL_MD),
    ("plugins/arch-office/skills/xlsx-risk-register/references/xlsx_risk_register_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_RISK_REGISTER_REFERENCES_XLSX_RISK_REGISTER_GEN_PY),
    ("plugins/arch-office/skills/xlsx-system-catalog/SKILL.md", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_SYSTEM_CATALOG_SKILL_MD),
    ("plugins/arch-office/skills/xlsx-system-catalog/references/xlsx_system_catalog_gen.py", PLUGIN_ARCH_OFFICE_SKILLS_XLSX_SYSTEM_CATALOG_REFERENCES_XLSX_SYSTEM_CATALOG_GEN_PY),
    ("plugins/spine-be-docs/plugin.json", PLUGIN_SPINE_BE_DOCS_PLUGIN_JSON),
    ("plugins/spine-be-docs/skills/check-spine-be-docs/SKILL.md", PLUGIN_SPINE_BE_DOCS_SKILLS_CHECK_SPINE_BE_DOCS_SKILL_MD),
    ("plugins/arch-governance/plugin.json", PLUGIN_ARCH_GOVERNANCE_PLUGIN_JSON),
    ("plugins/arch-governance/skills/architecture-sources-map/SKILL.md", PLUGIN_ARCH_GOVERNANCE_SKILLS_ARCHITECTURE_SOURCES_MAP_SKILL_MD),
    ("plugins/arch-governance/skills/fitness-function-catalog/SKILL.md", PLUGIN_ARCH_GOVERNANCE_SKILLS_FITNESS_FUNCTION_CATALOG_SKILL_MD),
    ("plugins/arch-governance/skills/rule-library-antipatterns/SKILL.md", PLUGIN_ARCH_GOVERNANCE_SKILLS_RULE_LIBRARY_ANTIPATTERNS_SKILL_MD),
    ("plugins/spine-workflows/plugin.json", PLUGIN_SPINE_WORKFLOWS_PLUGIN_JSON),
    ("plugins/spine-workflows/skills/spine-quickstart/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_QUICKSTART_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-bundle/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_BUNDLE_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-fitness-gate/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_FITNESS_GATE_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-architect-review/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ARCHITECT_REVIEW_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-adr-judge/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ADR_JUDGE_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-contracts-gate/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_CONTRACTS_GATE_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-content-bootstrap/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_CONTENT_BOOTSTRAP_SKILL_MD),
    ("plugins/spine-workflows/skills/spine-archify-viz/SKILL.md", PLUGIN_SPINE_WORKFLOWS_SKILLS_SPINE_ARCHIFY_VIZ_SKILL_MD),
];

/// Карта «относительный путь в домашнем каталоге → содержимое».
/// Порядок не важен; существующие файлы пропускаются.
const DEFAULT_FILES: &[(&str, &str)] = &[
    ("assets/ascii/banner.txt", BANNER),
    ("assets/prompts/architect.md", PROMPT_ARCHITECT),
    ("assets/prompts/adr.md", PROMPT_ADR),
    ("assets/prompts/spine.md", PROMPT_SPINE),
    (
        "assets/prompts/review_adversarial.md",
        PROMPT_REVIEW_ADVERSARIAL,
    ),
    ("assets/prompts/readiness_gate.md", PROMPT_READINESS_GATE),
    ("assets/prompts/handoff_compile.md", PROMPT_HANDOFF_COMPILE),
    (
        "assets/prompts/reverse_discovery.md",
        PROMPT_REVERSE_DISCOVERY,
    ),
    ("assets/prompts/nfr_design.md", PROMPT_NFR_DESIGN),
    ("assets/prompts/skill_distiller.md", PROMPT_SKILL_DISTILLER),
    (
        "assets/rubrics/solution_architecture.yaml",
        RUBRIC_SOLUTION_ARCHITECTURE,
    ),
    (
        "assets/rubrics/architecture_gates.yaml",
        RUBRIC_ARCHITECTURE_GATES,
    ),
    (
        "assets/rubrics/macedo_dimensions.yaml",
        RUBRIC_MACEDO_DIMENSIONS,
    ),
    ("assets/rubrics/adr_quality.yaml", RUBRIC_ADR_QUALITY),
    (
        "assets/rubrics/handoff_quality.yaml",
        RUBRIC_HANDOFF_QUALITY,
    ),
    (
        "assets/rubrics/adr_spine_consistency.yaml",
        RUBRIC_ADR_SPINE_CONSISTENCY,
    ),
    (
        "assets/rubrics/model_link_semantics.yaml",
        RUBRIC_MODEL_LINK_SEMANTICS,
    ),
    (
        "assets/rubrics/nfr_mechanism_fit.yaml",
        RUBRIC_NFR_MECHANISM_FIT,
    ),
    (
        "assets/rubrics/code_invariant_conformance.yaml",
        RUBRIC_CODE_INVARIANT_CONFORMANCE,
    ),
    (
        "assets/benchmarks/payment_integration.yaml",
        BENCH_PAYMENT_INTEGRATION,
    ),
    (
        "assets/benchmarks/legacy_decomposition.yaml",
        BENCH_LEGACY_DECOMPOSITION,
    ),
    (
        "assets/benchmarks/event_driven_design.yaml",
        BENCH_EVENT_DRIVEN_DESIGN,
    ),
    (
        "assets/benchmarks/pangolin_replication.yaml",
        BENCH_PANGOLIN_REPLICATION,
    ),
    (
        "assets/benchmarks/gigachat_sla_resilience.yaml",
        BENCH_GIGACHAT_SLA_RESILIENCE,
    ),
    (
        "assets/benchmarks/meta_agent_realtime.yaml",
        BENCH_META_AGENT_REALTIME,
    ),
    ("assets/benchmarks/golden/adr_full.md", GOLDEN_ADR_FULL_MD),
    (
        "assets/benchmarks/golden/adr_full.expected.yaml",
        GOLDEN_ADR_FULL_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_decent.md",
        GOLDEN_ADR_DECENT_MD,
    ),
    (
        "assets/benchmarks/golden/adr_decent.expected.yaml",
        GOLDEN_ADR_DECENT_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_no_alternatives.md",
        GOLDEN_ADR_NO_ALTERNATIVES_MD,
    ),
    (
        "assets/benchmarks/golden/adr_no_alternatives.expected.yaml",
        GOLDEN_ADR_NO_ALTERNATIVES_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_no_negatives.md",
        GOLDEN_ADR_NO_NEGATIVES_MD,
    ),
    (
        "assets/benchmarks/golden/adr_no_negatives.expected.yaml",
        GOLDEN_ADR_NO_NEGATIVES_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_irreversible.md",
        GOLDEN_ADR_IRREVERSIBLE_MD,
    ),
    (
        "assets/benchmarks/golden/adr_irreversible.expected.yaml",
        GOLDEN_ADR_IRREVERSIBLE_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_posthoc.md",
        GOLDEN_ADR_POSTHOC_MD,
    ),
    (
        "assets/benchmarks/golden/adr_posthoc.expected.yaml",
        GOLDEN_ADR_POSTHOC_EXPECTED,
    ),
    ("assets/benchmarks/golden/adr_stub.md", GOLDEN_ADR_STUB_MD),
    (
        "assets/benchmarks/golden/adr_stub.expected.yaml",
        GOLDEN_ADR_STUB_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_verbose_bad.md",
        GOLDEN_ADR_VERBOSE_BAD_MD,
    ),
    (
        "assets/benchmarks/golden/adr_verbose_bad.expected.yaml",
        GOLDEN_ADR_VERBOSE_BAD_EXPECTED,
    ),
    (
        "assets/benchmarks/golden/adr_compact_good.md",
        GOLDEN_ADR_COMPACT_GOOD_MD,
    ),
    (
        "assets/benchmarks/golden/adr_compact_good.expected.yaml",
        GOLDEN_ADR_COMPACT_GOOD_EXPECTED,
    ),
    ("mcp.json", MCP_SERVERS_EXAMPLE),
    ("cron.toml", CRON_EXAMPLE),
    ("CONSTRAINTS.example.yaml", CONSTRAINTS_EXAMPLE),
    (
        "assets/evals/agent-config/01-models-registry.yaml",
        EVAL_AGENT_CONFIG_MODELS_REGISTRY,
    ),
    (
        "assets/evals/agent-config/02-skills-library.yaml",
        EVAL_AGENT_CONFIG_SKILLS_LIBRARY,
    ),
    (
        "assets/evals/agent-config/03-rubrics-library.yaml",
        EVAL_AGENT_CONFIG_RUBRICS_LIBRARY,
    ),
    (
        "assets/evals/agent-config/04-bench-catalog.yaml",
        EVAL_AGENT_CONFIG_BENCH_CATALOG,
    ),
    (
        "assets/evals/agent-config/05-mermaid-render.yaml",
        EVAL_AGENT_CONFIG_MERMAID_RENDER,
    ),
    (
        "assets/evals/agent-config/06-control-score-fast.yaml",
        EVAL_AGENT_CONFIG_CONTROL_SCORE_FAST,
    ),
    (
        "assets/evals/agent-config/07-control-score-critical.yaml",
        EVAL_AGENT_CONFIG_CONTROL_SCORE_CRITICAL,
    ),
    (
        "assets/evals/agent-config/08-policy-denies-destructive.yaml",
        EVAL_AGENT_CONFIG_POLICY_DENIES_DESTRUCTIVE,
    ),
    ("cron/kb_digest.md", CRON_TASK_KB_DIGEST),
    ("cron/spec_drift.md", CRON_TASK_SPEC_DRIFT),
    (
        "assets/rubrics/agents_md_quality.yaml",
        RUBRIC_AGENTS_MD_QUALITY_YAML,
    ),
    ("cron/agents_md_drift.md", CRON_TASK_AGENTS_MD_DRIFT_MD),
];

/// Программный доступ к встроенным файлам плагинов (`assets/plugins/**`):
/// «относительный путь в домашнем каталоге → содержимое». Используется
/// `connect` для раскладки скиллов хостам из релизного бинаря — без
/// предварительного `arch-be init`.
#[must_use]
pub fn embedded_plugin_files() -> &'static [(&'static str, &'static str)] {
    PLUGIN_FILES
}

/// Пишет дефолтные ассеты в домашний каталог (`~/.arch-harness`).
///
/// Существующие файлы не затираются (пользователь мог их отредактировать);
/// отсутствующие каталоги создаются. Возвращает список фактически
/// записанных файлов — при повторном вызове на заполненном каталоге
/// список пуст.
///
/// # Errors
/// Ошибка создания каталогов или записи файлов (с привязкой к пути).
pub fn write_defaults(home: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (rel, content) in DEFAULT_FILES
        .iter()
        .chain(PLUGIN_FILES.iter())
        .chain(SEMANTIC_GOLDEN.iter())
    {
        let path = home.join(rel);
        if path.exists() {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HarnessError::io(parent, e))?;
        }
        std::fs::write(&path, content).map_err(|e| HarnessError::io(&path, e))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn write_defaults_creates_full_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let written = write_defaults(tmp.path()).expect("write_defaults");
        let total = DEFAULT_FILES.len() + PLUGIN_FILES.len() + SEMANTIC_GOLDEN.len();
        assert_eq!(written.len(), total, "записаны не все файлы");
        for (rel, content) in DEFAULT_FILES
            .iter()
            .chain(PLUGIN_FILES.iter())
            .chain(SEMANTIC_GOLDEN.iter())
        {
            let path = tmp.path().join(rel);
            assert!(path.is_file(), "нет файла {rel}");
            let on_disk = std::fs::read_to_string(&path).expect("read");
            assert_eq!(
                &on_disk, content,
                "содержимое {rel} не совпадает со встроенным"
            );
        }
    }

    #[test]
    fn write_defaults_is_idempotent_and_never_overwrites() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = write_defaults(tmp.path()).expect("первый прогон");
        assert!(!first.is_empty());

        // Метка: пользовательская правка не должна быть затёрта.
        let marker = tmp.path().join("assets/prompts/architect.md");
        std::fs::write(&marker, "МЕТКА-ПОЛЬЗОВАТЕЛЯ").expect("маркер");

        let second = write_defaults(tmp.path()).expect("второй прогон");
        assert!(
            second.is_empty(),
            "повторный вызов что-то записал: {second:?}"
        );
        let kept = std::fs::read_to_string(&marker).expect("read marker");
        assert_eq!(kept, "МЕТКА-ПОЛЬЗОВАТЕЛЯ", "пользовательский файл затёрт");

        // Удалённый файл восстанавливается — и только он.
        let gone = tmp.path().join("assets/rubrics/adr_quality.yaml");
        std::fs::remove_file(&gone).expect("remove");
        let third = write_defaults(tmp.path()).expect("третий прогон");
        assert_eq!(third, vec![gone], "восстановлен ровно один удалённый файл");
    }

    #[test]
    fn banner_fits_60_columns_and_has_signature() {
        let lines: Vec<&str> = BANNER.lines().collect();
        assert_eq!(
            lines.len(),
            14,
            "баннер ArchSpine: 6 строк ARCH + 6 строк SPINE + редакция + подпись"
        );
        assert!(
            BANNER.contains("B A N K I N G   E D I T I O N"),
            "нет строки редакции"
        );
        for line in &lines {
            let width = UnicodeWidthStr::width(*line);
            assert!(width <= 60, "строка шире 60 колонок ({width}): {line}");
        }
        let last = lines.last().expect("непустой баннер");
        assert!(
            last.contains("solution-архитект"),
            "нет строки-подписи: {last}"
        );
    }

    #[test]
    fn rubrics_parse_and_have_anchors_1_3_5() {
        let rubrics = [
            RUBRIC_SOLUTION_ARCHITECTURE,
            RUBRIC_ARCHITECTURE_GATES,
            RUBRIC_MACEDO_DIMENSIONS,
            RUBRIC_ADR_QUALITY,
            RUBRIC_HANDOFF_QUALITY,
            RUBRIC_AGENTS_MD_QUALITY_YAML,
            RUBRIC_ADR_SPINE_CONSISTENCY,
            RUBRIC_MODEL_LINK_SEMANTICS,
            RUBRIC_NFR_MECHANISM_FIT,
            RUBRIC_CODE_INVARIANT_CONFORMANCE,
        ];
        for text in rubrics {
            let r: crate::rubric::Rubric = serde_yaml_ng::from_str(text).expect("рубрика парсится");
            assert_eq!(r.scale_max, 5, "{}: scale_max != 5", r.name);
            assert_eq!(r.origin, "anchor", "{}: origin != anchor", r.name);
            assert!(!r.criteria.is_empty(), "{}: пустые критерии", r.name);
            for c in &r.criteria {
                assert!(c.weight > 0.0, "{}: {} — нулевой вес", r.name, c.id);
                for level in [1u8, 3, 5] {
                    assert!(
                        c.anchors.contains_key(&level),
                        "{}: {} — нет якоря уровня {level}",
                        r.name,
                        c.id
                    );
                }
            }
        }
    }

    /// Golden-набор смысловых рубрик (ADR-051, S4): у каждой рубрики шесть
    /// досье, эталоны покрывают все её критерии, а главный критерий размечен
    /// так, чтобы кейс что-то измерял: на дефектном он низкий, на чистом —
    /// высокий. Ошибка в разметке делает кейс бесполезным молча — отсюда
    /// проверка, а не доверие к автору файла.
    #[test]
    fn semantic_golden_set_parses_and_measures() {
        let rubrics: [(&str, &str, &str); 4] = [
            (
                "adr_spine_consistency",
                RUBRIC_ADR_SPINE_CONSISTENCY,
                "adr_vs_spine",
            ),
            (
                "model_link_semantics",
                RUBRIC_MODEL_LINK_SEMANTICS,
                "entity_links",
            ),
            (
                "nfr_mechanism_fit",
                RUBRIC_NFR_MECHANISM_FIT,
                "nfr_mechanism",
            ),
            (
                "code_invariant_conformance",
                RUBRIC_CODE_INVARIANT_CONFORMANCE,
                "code_vs_spine",
            ),
        ];
        for (name, yaml, pack) in rubrics {
            let rubric: crate::rubric::Rubric =
                serde_yaml_ng::from_str(yaml).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                rubric.pack.map(crate::rubric_pack::PackKind::as_str),
                Some(pack)
            );
            let main = rubric
                .criteria
                .iter()
                .find(|c| c.blocking)
                .unwrap_or_else(|| panic!("{name}: нет главного критерия"));

            // Файлы рубрики разложены по «<имя>.md» и «<имя>.expected.yaml».
            let dir = format!("assets/benchmarks/golden/semantic/{name}/");
            let files: Vec<(&str, &str)> = SEMANTIC_GOLDEN
                .iter()
                .filter(|(rel, _)| rel.starts_with(&dir))
                .map(|(rel, body)| (&rel[dir.len()..], *body))
                .collect();
            assert_eq!(files.len(), 12, "{name}: 6 досье + 6 эталонов");

            let (mut docs, mut defective, mut clean) = (0usize, 0usize, 0usize);
            for (file, body) in &files {
                if !std::path::Path::new(file)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
                {
                    continue;
                }
                docs += 1;
                assert!(
                    body.contains("=== ИСТОЧНИК subject:"),
                    "{name}/{file}: досье без источника-субъекта"
                );
                assert_eq!(
                    body.matches(crate::rubric_pack::SOURCE_BEGIN).count(),
                    body.matches(crate::rubric_pack::SOURCE_END).count(),
                    "{name}/{file}: маркеры источников непарные"
                );
                let expected = files
                    .iter()
                    .find(|(f, _)| *f == format!("{}.expected.yaml", file.trim_end_matches(".md")))
                    .map_or_else(|| panic!("{name}/{file}: нет эталона"), |(_, b)| *b);
                let exp: crate::bench::GoldenExpectation = serde_yaml_ng::from_str(expected)
                    .unwrap_or_else(|e| panic!("{name}/{file}: эталон: {e}"));
                assert_eq!(exp.rubric, name, "{name}/{file}: имя рубрики в эталоне");
                assert_ne!(
                    exp.kind,
                    crate::bench::GoldenKind::Unlabeled,
                    "{name}/{file}: у смыслового кейса обязана быть метка"
                );
                for c in &rubric.criteria {
                    assert!(
                        exp.scores.contains_key(&c.id),
                        "{name}/{file}: нет эталона критерия '{}'",
                        c.id
                    );
                }
                let range = exp.scores[&main.id];
                assert!(
                    range.upper() <= rubric.scale_max,
                    "{name}/{file}: балл выше шкалы"
                );
                match exp.kind {
                    crate::bench::GoldenKind::Defective => {
                        defective += 1;
                        // Распределённый дефект вправе жить во ВТОРОМ критерии
                        // (механизм достаточен, а проверка измеряет не то):
                        // требуем, чтобы рубрика дефект видела хоть где-то, и
                        // чтобы главный критерий не был «всё хорошо».
                        assert!(
                            exp.scores.values().any(|r| r.upper() <= 2),
                            "{name}/{file}: дефектный кейс не имеет НИ ОДНОГО низкого \
                             критерия — он ничего не измеряет"
                        );
                        assert!(
                            range.upper() <= 3,
                            "{name}/{file}: на дефектном кейсе главный критерий '{}' не \
                             может быть высоким",
                            main.id
                        );
                    }
                    crate::bench::GoldenKind::Clean => {
                        clean += 1;
                        assert!(
                            range.midpoint() >= 3.5,
                            "{name}/{file}: на чистом кейсе главный критерий '{}' обязан \
                             быть высоким — иначе измеряется не чистота",
                            main.id
                        );
                    }
                    crate::bench::GoldenKind::Unlabeled => unreachable!("проверено выше"),
                }
            }
            assert_eq!(docs, 6, "{name}: шесть досье");
            assert_eq!((defective, clean), (3, 3), "{name}: 3 дефектных + 3 чистых");
        }
    }

    /// Смысловые рубрики (ADR-051, волна B): у каждой ровно один главный
    /// критерий, и он — тот, где обвинение опирается на цитаты обеих сторон.
    /// Гейт строит по нему блокирующую находку, поэтому «нет главного» и
    /// «главных два» одинаково недопустимы: первое оставило бы класс дефекта
    /// без блокировки, второе сделало бы вердикт непредсказуемым.
    #[test]
    fn semantic_rubrics_have_exactly_one_blocking_criterion() {
        let rubrics = [
            (
                "adr_spine_consistency",
                RUBRIC_ADR_SPINE_CONSISTENCY,
                crate::rubric_pack::PackKind::AdrVsSpine,
            ),
            (
                "model_link_semantics",
                RUBRIC_MODEL_LINK_SEMANTICS,
                crate::rubric_pack::PackKind::EntityLinks,
            ),
            (
                "nfr_mechanism_fit",
                RUBRIC_NFR_MECHANISM_FIT,
                crate::rubric_pack::PackKind::NfrMechanism,
            ),
            (
                "code_invariant_conformance",
                RUBRIC_CODE_INVARIANT_CONFORMANCE,
                crate::rubric_pack::PackKind::CodeVsSpine,
            ),
        ];
        for (name, text, pack) in rubrics {
            let r: crate::rubric::Rubric =
                serde_yaml_ng::from_str(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(r.name, name, "имя рубрики — имя файла");
            let blocking: Vec<&crate::rubric::Criterion> =
                r.criteria.iter().filter(|c| c.blocking).collect();
            assert_eq!(
                blocking.len(),
                1,
                "{name}: главный критерий должен быть один, найдено {}",
                blocking.len()
            );
            let main = blocking[0];
            assert!(
                main.evidence_on.requires_low(),
                "{name}/{}: главный критерий судит обвинение — цитата нужна при низком балле",
                main.id
            );
            assert!(
                !main.evidence_roles.is_empty(),
                "{name}/{}: обвинение обязано опираться на обе стороны (роли)",
                main.id
            );
            // Схема рубрики — тот же путь загрузки, что у CLI и MCP: имена
            // ролей проверяются на загрузке, опечатка тут не пройдёт.
            let path = std::env::temp_dir().join(format!("{name}-schema.yaml"));
            std::fs::write(&path, text).expect("запись рубрики");
            let loaded = crate::rubric::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(loaded.criteria.len(), r.criteria.len());
            // Соответствие «рубрика → досье» объявлено в самой рубрике, а не
            // в коде гейта: без него субъект для сбора досье пришлось бы
            // угадывать по имени рубрики.
            assert_eq!(r.pack, Some(pack), "{name}: вид досье объявлен в рубрике");
            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn solution_architecture_weights_sum_to_100() {
        let r: crate::rubric::Rubric =
            serde_yaml_ng::from_str(RUBRIC_SOLUTION_ARCHITECTURE).expect("parse");
        assert_eq!(r.criteria.len(), 15, "критериев не 15 (матрица §C.3)");
        let sum: f64 = r.criteria.iter().map(|c| c.weight).sum();
        assert!(
            (sum - 100.0).abs() < 1e-9,
            "сумма весов {sum}, ожидается 100"
        );
    }

    #[test]
    #[cfg(feature = "harness")]
    fn benchmarks_parse_and_reference_existing_rubric() {
        let benches = [
            BENCH_PAYMENT_INTEGRATION,
            BENCH_LEGACY_DECOMPOSITION,
            BENCH_EVENT_DRIVEN_DESIGN,
            BENCH_PANGOLIN_REPLICATION,
            BENCH_GIGACHAT_SLA_RESILIENCE,
            BENCH_META_AGENT_REALTIME,
        ];
        for text in benches {
            let b: crate::bench::Benchmark =
                serde_yaml_ng::from_str(text).expect("бенчмарк парсится");
            assert!(
                (b.pass_threshold - 3.5).abs() < f64::EPSILON,
                "{}: порог",
                b.name
            );
            assert_eq!(b.rubric, "solution_architecture", "{}: рубрика", b.name);
            assert!(!b.tags.is_empty(), "{}: нет тегов", b.name);
            assert!(
                b.task.lines().count() >= 10,
                "{}: постановка короче 10 строк",
                b.name
            );
            assert!(
                b.system_prompt.contains("architect"),
                "{}: system_prompt без роли",
                b.name
            );
        }
    }

    #[test]
    #[cfg(feature = "harness")]
    fn golden_set_pairs_parse_and_match_adr_quality_rubric() {
        let rubric: crate::rubric::Rubric =
            serde_yaml_ng::from_str(RUBRIC_ADR_QUALITY).expect("рубрика adr_quality");
        let pairs = [
            (GOLDEN_ADR_FULL_MD, GOLDEN_ADR_FULL_EXPECTED),
            (GOLDEN_ADR_DECENT_MD, GOLDEN_ADR_DECENT_EXPECTED),
            (
                GOLDEN_ADR_NO_ALTERNATIVES_MD,
                GOLDEN_ADR_NO_ALTERNATIVES_EXPECTED,
            ),
            (GOLDEN_ADR_NO_NEGATIVES_MD, GOLDEN_ADR_NO_NEGATIVES_EXPECTED),
            (GOLDEN_ADR_IRREVERSIBLE_MD, GOLDEN_ADR_IRREVERSIBLE_EXPECTED),
            (GOLDEN_ADR_POSTHOC_MD, GOLDEN_ADR_POSTHOC_EXPECTED),
            (GOLDEN_ADR_STUB_MD, GOLDEN_ADR_STUB_EXPECTED),
            (GOLDEN_ADR_VERBOSE_BAD_MD, GOLDEN_ADR_VERBOSE_BAD_EXPECTED),
            (GOLDEN_ADR_COMPACT_GOOD_MD, GOLDEN_ADR_COMPACT_GOOD_EXPECTED),
        ];
        assert!(
            (5..=10).contains(&pairs.len()),
            "golden-set: 5–10 документов по ADR-004"
        );
        for (doc, expected) in pairs {
            assert!(doc.len() > 100, "golden-документ не должен быть пустышкой");
            let exp: crate::bench::GoldenExpectation =
                serde_yaml_ng::from_str(expected).expect("эталон парсится");
            assert_eq!(exp.rubric, "adr_quality", "эталонная рубрика");
            assert_eq!(
                exp.scores.len(),
                rubric.criteria.len(),
                "эталон покрывает все критерии рубрики"
            );
            for (id, score) in &exp.scores {
                assert!(
                    rubric.criteria.iter().any(|c| &c.id == id),
                    "критерия '{id}' нет в рубрике adr_quality"
                );
                assert!(
                    (1..=rubric.scale_max).contains(&score.upper()),
                    "{id}: балл {} вне шкалы 1..={}",
                    score.upper(),
                    rubric.scale_max
                );
            }
        }
    }

    #[test]
    fn example_config_parses_into_config_struct() {
        let text = include_str!("../config.example.toml");
        let cfg: crate::config::Config =
            toml::from_str(text).expect("config.example.toml парсится");
        assert_eq!(cfg.default_model, "deepseek");
        for name in ["deepseek", "deepseek-pro", "kimi", "glm"] {
            assert!(cfg.models.contains_key(name), "нет модели {name}");
        }
        assert_eq!(cfg.harnesses.len(), 7, "харнессов не 7");
        assert!(cfg.harnesses.contains_key("kimi-code"), "нет kimi-code");
        assert_eq!(cfg.knowledge.dirs.len(), 3, "knowledge dirs");
        assert_eq!(cfg.web.arch_sites.len(), 11, "arch_sites");
        assert!(cfg.mcp.servers_file.ends_with("mcp.json"));
        assert!(cfg.cron.file.ends_with("cron.toml"));
    }

    #[test]
    #[cfg(feature = "harness")]
    fn example_cron_parses_and_tasks_are_shipped() {
        let tab: crate::cron::CronTab =
            toml::from_str(CRON_EXAMPLE).expect("cron.example.toml парсится");
        assert_eq!(tab.jobs.len(), 3, "задач не 3");
        let names: Vec<&str> = tab.jobs.iter().map(|j| j.name.as_str()).collect();
        assert!(
            names.contains(&"kb-digest")
                && names.contains(&"spec-drift")
                && names.contains(&"agents-md-drift")
        );
        for job in &tab.jobs {
            assert_eq!(
                job.schedule.split_whitespace().count(),
                5,
                "{}: не 5 полей cron",
                job.name
            );
            let file = job
                .task_md
                .file_name()
                .expect("имя файла")
                .to_string_lossy();
            let shipped = DEFAULT_FILES
                .iter()
                .any(|(rel, _)| rel.ends_with(file.as_ref()));
            assert!(
                shipped,
                "{}: task_md {file} не раскладывается write_defaults",
                job.name
            );
        }
    }

    #[test]
    fn example_mcp_json_has_three_servers() {
        let v: serde_json::Value = serde_json::from_str(MCP_SERVERS_EXAMPLE).expect("mcp json");
        let servers = v
            .get("mcpServers")
            .and_then(|s| s.as_object())
            .expect("mcpServers");
        for name in ["filesystem", "fetch", "memory"] {
            let spec = servers
                .get(name)
                .unwrap_or_else(|| panic!("нет сервера {name}"));
            assert!(spec.get("command").is_some(), "{name}: нет command");
        }
    }

    #[test]
    fn mermaid_examples_have_detectable_kind() {
        use crate::mermaid::{DiagramKind, diagram_kind};
        let flow = include_str!("../examples/mermaid/flow.mmd");
        let seq = include_str!("../examples/mermaid/seq.mmd");
        assert_eq!(diagram_kind(flow), DiagramKind::Flowchart);
        assert_eq!(diagram_kind(seq), DiagramKind::Sequence);
    }

    #[test]
    fn embedded_plugins_are_valid() {
        // plugin.json валидны, скиллы имеют frontmatter name+description.
        let mut plugin_jsons = 0;
        let mut skills = 0;
        for (rel, content) in PLUGIN_FILES {
            if rel.ends_with("plugin.json") {
                let v: serde_json::Value = serde_json::from_str(content).expect(rel);
                assert!(v.get("name").is_some(), "{rel}: нет name");
                plugin_jsons += 1;
            }
            if rel.ends_with("SKILL.md") {
                assert!(content.starts_with("---"), "{rel}: нет frontmatter");
                assert!(content.contains("name:"), "{rel}: нет name");
                assert!(content.contains("description:"), "{rel}: нет description");
                skills += 1;
            }
            if Path::new(rel)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                && !rel.ends_with("plugin.json")
            {
                serde_json::from_str::<serde_json::Value>(content)
                    .unwrap_or_else(|_| panic!("{rel}"));
            }
        }
        assert!(
            plugin_jsons >= 3,
            "ожидаются плагины arch-core + patterns-*"
        );
        assert!(skills >= 20, "скиллов: {skills}");
    }

    #[test]
    fn spine_template_passes_linter() {
        let tmp = tempfile::tempdir().expect("tmp");
        let spine = tmp.path().join("ARCHITECTURE-SPINE.md");
        let text = PLUGIN_FILES
            .iter()
            .find(|(rel, _)| rel.ends_with("spine-template.md"))
            .map(|(_, c)| *c)
            .expect("spine template shipped");
        std::fs::write(&spine, text).expect("write");
        let issues = crate::control::lint_spine(&spine).expect("lint");
        let errors: Vec<_> = issues.iter().filter(|i| i.severity == "error").collect();
        assert!(
            errors.is_empty(),
            "шаблон spine не проходит линтер: {errors:?}"
        );
    }
}

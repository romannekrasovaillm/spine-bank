//! Подкоманды `arch-be rules` (+ `rules template`) и их обработчики:
//! кандидатные fitness-правила и шаблоны исполняемых правил (B1: выделено из `main.rs`).

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use super::resolve_constraints_cli;

/// Подкоманды `arch-be rules`.
#[derive(Subcommand)]
pub(crate) enum RulesCmd {
    /// Кандидатные fitness-правила кейса (то же, что `control rules-suggest`).
    Suggest {
        /// Корень кейса (каталог с `docs/`, `model/`, `.arch-handoff/`).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// E7.3: читать историю отчётов судьи вместо пробелов кейса.
        /// Повторяющееся обвинение становится кандидатом `must_not_contain`,
        /// повторяющаяся метка механики — advisory без механики.
        #[arg(long)]
        from_judge: bool,
        /// Каталог истории отчётов (по умолчанию — архив харнесса,
        /// `$ARCH_HOME/reports`); действует вместе с `--from-judge`.
        #[arg(long)]
        history: Option<PathBuf>,
        /// Сколько прогонов делают находку повторяющейся (по умолчанию
        /// `judge_rules::MIN_RUNS` = 3); действует вместе с `--from-judge`.
        #[arg(long, default_value_t = arch_harness::judge_rules::MIN_RUNS)]
        min_runs: usize,
    },
    /// Шаблоны исполняемых правил: библиотека, применение, проверка зубов.
    Template {
        #[command(subcommand)]
        cmd: RulesTemplateCmd,
    },
    /// Подтвердить доверие исполняемым правилам реестра (модель доверия A3,
    /// ADR-053): записывает SHA-256 канонизированного набора command-строк всех
    /// `command_succeeds`-правил в `~/.arch-harness/trusted.json`. После этого
    /// изменение набора команд реестра даёт пропуск `command_untrusted` (команды
    /// не исполняются), пока доверие не подтвердят повторно. Без записи —
    /// поведение прежнее (исполнение разрешено); `--no-exec`/`ARCH_NO_EXEC=1`
    /// приоритетнее записи.
    Allow {
        /// Репозиторий с реестром правил (по умолчанию — текущий каталог).
        #[arg(default_value = ".")]
        repo: PathBuf,
        /// Файл ограничений (по умолчанию — единый резолвер: явный путь →
        /// `.arch-handoff/CONSTRAINTS.yaml` → корневой `CONSTRAINTS.yaml`).
        #[arg(long)]
        constraints: Option<PathBuf>,
    },
}

/// Подкоманды `arch-be rules template`.
#[derive(Subcommand)]
pub(crate) enum RulesTemplateCmd {
    /// Список шаблонов библиотеки.
    List,
    /// Показать шаблон: свойства, файлы, команды, словарь подбора.
    Show {
        /// Id шаблона.
        id: String,
    },
    /// Положить файлы шаблона в кейс и напечатать фрагмент правила.
    Apply {
        /// Id шаблона.
        id: String,
        /// Инвариант спайна, к которому привязывается правило (`AD-3`).
        #[arg(long)]
        ad: String,
        /// Корень кейса.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Язык поставки: python | java | both.
        #[arg(long, default_value = "python")]
        lang: String,
        /// Показать, что было бы сделано, ничего не записывая.
        #[arg(long)]
        dry_run: bool,
    },
    /// Проверка зубов: тест обязан падать на нарушающей реализации.
    Verify {
        /// Проверить все шаблоны библиотеки во временных каталогах.
        #[arg(long)]
        all: bool,
        /// Проверить применённые шаблоны кейса (по `.arch-handoff/rule-templates.lock`).
        #[arg(long)]
        dir: Option<PathBuf>,
        /// JUnit-консоль (`junit-platform-console-standalone.jar`) для java-половины
        /// без Maven.
        #[arg(long)]
        java_jar: Option<PathBuf>,
        /// Язык проверки: python | java | both.
        #[arg(long, default_value = "both")]
        lang: String,
        /// Требовать python3: без него проверка считается проваленной (для CI).
        #[arg(long)]
        require_python: bool,
    },
}

/// Обрабатывает `arch-be rules …`: кандидаты и шаблоны исполняемых правил.
pub(crate) fn cmd_rules(cmd: RulesCmd) -> Result<()> {
    match cmd {
        RulesCmd::Suggest {
            path,
            from_judge,
            history,
            min_runs,
        } => {
            if from_judge {
                // E7.3: источник — история отчётов судьи, а не пробелы кейса.
                let dir = history.unwrap_or_else(arch_harness::judge_rules::default_history_dir);
                let runs = arch_harness::judge_rules::read_history(&dir);
                let report = arch_harness::judge_rules::suggest(&runs, min_runs);
                if runs.is_empty() {
                    eprintln!(
                        "История отчётов судьи пуста: {} (каталог создаёт `rubric run`)",
                        dir.display()
                    );
                }
                print!("{}", arch_harness::rules_suggest::render_markdown(&report));
                Ok(())
            } else {
                let report = arch_harness::rules_suggest::suggest(&path)?;
                print!("{}", arch_harness::rules_suggest::render_markdown(&report));
                Ok(())
            }
        }
        RulesCmd::Template { cmd } => cmd_rules_template(cmd),
        RulesCmd::Allow { repo, constraints } => {
            // A3: доверие фиксируется на канонизированный отпечаток набора
            // command-строк ВСЕГО разрешённого реестра (extends учтён —
            // загрузчик тот же, что у `control check`).
            let c = resolve_constraints_cli(&repo, constraints);
            if !c.is_file() {
                anyhow::bail!(
                    "реестр правил не найден: ни {} в корне, ни {} — нечего подтверждать",
                    arch_harness::control::ROOT_CONSTRAINTS_PATH,
                    arch_harness::control::HANDOFF_CONSTRAINTS_PATH
                );
            }
            let resolved = arch_harness::control::load_constraints_resolved(&c)?;
            let commands = arch_harness::control::command_strings(&resolved.rules);
            let fingerprint =
                arch_harness::cmd_trust::commands_fingerprint(commands.iter().copied());
            let trust_file = arch_harness::cmd_trust::default_trust_file();
            let label = c.strip_prefix(&repo).unwrap_or(&c).display().to_string();
            let entry = arch_harness::cmd_trust::record_allow(
                &trust_file,
                &repo,
                &fingerprint,
                commands.len(),
                &label,
            )?;
            let repo_label = repo.canonicalize().unwrap_or_else(|_| repo.clone());
            println!(
                "Доверие записано: {} → репозиторий {}",
                trust_file.display(),
                repo_label.display()
            );
            println!(
                "  реестр: {label}; команд command_succeeds: {}; отпечаток sha256:{}…",
                entry.commands,
                fingerprint.get(..12).unwrap_or(fingerprint.as_str())
            );
            if entry.commands == 0 {
                println!(
                    "  в реестре нет command_succeeds-правил — зафиксировано их отсутствие: \
                     добавление такого правила потребует повторного `rules allow`"
                );
            }
            println!(
                "Изменение набора команд реестра теперь даст пропуск {} (команды не \
                 исполняются), пока доверие не подтвердят повторно: `arch-be rules allow`. \
                 --no-exec/ARCH_NO_EXEC=1 приоритетнее этой записи.",
                arch_harness::cmd_trust::COMMAND_UNTRUSTED
            );
            Ok(())
        }
    }
}

/// Обрабатывает `arch-be rules template …`.
pub(crate) fn cmd_rules_template(cmd: RulesTemplateCmd) -> Result<()> {
    use arch_harness::rule_templates as rt;
    match cmd {
        RulesTemplateCmd::List => {
            print!("{}", rt::render_list()?);
            Ok(())
        }
        RulesTemplateCmd::Show { id } => {
            print!("{}", rt::render_show(&id)?);
            Ok(())
        }
        RulesTemplateCmd::Apply {
            id,
            ad,
            dir,
            lang,
            dry_run,
        } => {
            let lang = rt::Lang::parse(&lang)?;
            let report = rt::apply(&dir, &id, &ad, lang, dry_run)?;
            println!(
                "Шаблон: {} v{} → {}",
                report.template,
                report.version,
                report.target_dir.display()
            );
            println!(
                "Файлов {}: {}",
                if report.dry_run {
                    "было бы записано"
                } else {
                    "записано"
                },
                report.written.len()
            );
            for path in &report.written {
                println!("  {}", path.display());
            }
            println!(
                "\nФрагмент для CONSTRAINTS.yaml (под ключом `rules:`) — {}:\n",
                if report.dry_run {
                    "печатается, на диск не пишется"
                } else {
                    "печатается, НЕ вносится"
                }
            );
            println!("{}", report.fragment);
            println!(
                "\nСтрока для сущности инварианта в model/ (вторая строка frontmatter):\n  {}",
                report.verified_by
            );
            if !report.notes.is_empty() {
                println!("\nЗамечания:");
                for note in &report.notes {
                    println!("  - {note}");
                }
            }
            Ok(())
        }
        RulesTemplateCmd::Verify {
            all,
            dir,
            java_jar,
            lang,
            require_python,
        } => {
            let lang = rt::Lang::parse(&lang)?;
            let runner = rt::Runner::detect(java_jar.as_deref());
            let report = match (all, dir) {
                (true, None) => rt::verify_all(&runner, lang, require_python)?,
                (false, Some(case)) => rt::verify_dir(&case, &runner, lang)?,
                _ => {
                    return Err(anyhow::Error::msg(
                        "укажите ровно одно: --all (шаблоны библиотеки) или --dir <кейс>",
                    ));
                }
            };
            print!("{}", rt::render_verify(&report));
            if !report.passed() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

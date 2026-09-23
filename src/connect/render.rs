//! Рендер отчёта `connect` (B1): план/итог по-русски — файлы, скиллы,
//! заметки, сниппеты, следующие шаги.

use std::fmt::Write as _;

use super::types::{ConnectOptions, ConnectReport, SkillAction};

/// Общий рендер отчёта connect: файлы, скиллы, заметки, сниппеты, следующие
/// шаги. Заголовок произвольный — у хостов агентов он строится в
/// [`render_report`], у `connect ci`/`connect git-hooks` — свой.
#[must_use]
pub fn render_plan(title: &str, report: &ConnectReport) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{title}");
    if report.dry_run {
        let _ = writeln!(out, "(dry-run: ничего не записано — ниже план)");
    }
    out.push('\n');
    if !report.created.is_empty() {
        out.push_str("Записано:\n");
        for p in &report.created {
            let _ = writeln!(out, "  + {}", p.display());
        }
    }
    if !report.merged.is_empty() {
        out.push_str("Смерджено (чужие ключи сохранены):\n");
        for p in &report.merged {
            let _ = writeln!(out, "  ~ {}", p.display());
        }
    }
    if !report.unchanged.is_empty() {
        out.push_str("Без изменений:\n");
        for p in &report.unchanged {
            let _ = writeln!(out, "  = {}", p.display());
        }
    }
    if !report.skills.is_empty() {
        let total = report.skills.len();
        let _ = writeln!(out, "Скиллы ({total}):");
        let _ = writeln!(
            out,
            "  источник: из библиотеки пользователя: {}, из встроенных: {}",
            report.skills_from_library, report.skills_from_embedded
        );
        for s in &report.skills {
            let line = match &s.action {
                SkillAction::Copied(n) => format!("скопирован ({n} файлов)"),
                SkillAction::Updated(n) => format!("обновлён ({n} файлов)"),
                SkillAction::Unchanged(n) => format!("без изменений ({n} файлов)"),
            };
            let _ = writeln!(out, "  ✓ {} — {line}", s.name);
        }
    }
    if !report.skills_skipped.is_empty() {
        out.push_str("Пропущены файлы скиллов:\n");
        for s in &report.skills_skipped {
            let _ = writeln!(out, "  ! {s}");
        }
    }
    if !report.notes.is_empty() {
        out.push_str("Заметки:\n");
        for n in &report.notes {
            let _ = writeln!(out, "  - {n}");
        }
    }
    for (title, text) in &report.snippets {
        let _ = writeln!(out, "\n── {title}\n{text}");
    }
    if !report.next_steps.is_empty() {
        out.push_str("\nСледующие шаги:\n");
        for (i, step) in report.next_steps.iter().enumerate() {
            let _ = writeln!(out, "  {}. {step}", i + 1);
        }
    }
    out
}

/// Рендерит отчёт команды по-русски: файлы, скиллы, заметки, сниппеты,
/// следующие шаги.
#[must_use]
pub fn render_report(opts: &ConnectOptions, report: &ConnectReport) -> String {
    render_plan(
        &format!(
            "Подключение Spine к хосту «{}» — {}",
            opts.host.name(),
            opts.dir.display()
        ),
        report,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::Host;
    use crate::connect::connect;
    use crate::connect::testkit::user_library;

    /// (г) Печать результата несёт раздельные счётчики источников.
    #[test]
    fn render_prints_skill_source_counters() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lib = user_library(
            tmp.path(),
            "arch-distilled",
            "user-only-skill",
            "---\nname: user-only-skill\ndescription: свой\n---\n\n# Свой\n",
        );
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            plugins_dirs: vec![lib],
            ..ConnectOptions::new(Host::Claude, dir.clone())
        };
        let report = connect(&opts).expect("connect");
        let text = render_report(&opts, &report);
        assert!(
            text.contains("источник: из библиотеки пользователя: 1, из встроенных:"),
            "раздельные счётчики в печати: {text}"
        );
    }

    /// Рендер отчёта: русские секции, dry-run помечен.
    #[test]
    fn render_report_in_russian_marks_dry_run() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dir = tmp.path().join("proj");
        let opts = ConnectOptions {
            dry_run: true,
            ..ConnectOptions::new(Host::Claude, dir)
        };
        let report = connect(&opts).expect("connect");
        let text = render_report(&opts, &report);
        assert!(
            text.contains("Подключение Spine к хосту «claude»"),
            "{text}"
        );
        assert!(text.contains("dry-run"), "{text}");
        assert!(text.contains("Следующие шаги"), "{text}");
        assert!(text.contains("claude mcp list"), "{text}");
    }
}

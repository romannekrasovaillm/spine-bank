//! Импорт реестров систем в типизированную модель (EA, бэклог «дрейф
//! модели и импорт реестров», волна 3): выгрузки CMDB (CSV/xlsx) и
//! Backstage `catalog-info.yaml` → сущности `SYS-*` (+ `OWNER-*`).
//!
//! КОНТРАКТ (владелец: агент `model`):
//! - источник истины по «уже есть» — целевой каталог модели: повторный
//!   импорт идемпотентен (сущность с тем же `id` — либо с тем же
//!   нормализованным названием, когда `id` в реестре нет — пропускается
//!   как `skipped`; перезапись — только явный `--force`, и тогда файл
//!   существующей сущности переписывается НА МЕСТЕ, без осиротевших
//!   дублей `SYS-001-old.md`/`SYS-001-new.md`; `--force` касается только
//!   строк реестра — владельцы `OWNER-*` полей реестра не несут и не
//!   перезаписываются, попадая в `skipped`);
//! - создаются только `SYS-*` (строки реестра) и `OWNER-*` (владельцы,
//!   поимённо новые — дедуп по нормализованному названию); связи
//!   `depends_on` разрешаются по ID или названию в рамках файла и
//!   существующей модели; неразрешённая цель — warning, связь пропускается;
//! - статус: колонка `status` / Backstage `spec.lifecycle`, по умолчанию
//!   `imported` (честный маркер происхождения, а не выдуманный `adopted`);
//! - новые сущности пишутся как `ID-<slug>.md` (slug — `kebab_slug`
//!   из `control`); `--dry-run` строит план без записи;
//! - ограниченные профили форматов (без новых зависимостей, AD-6):
//!   CSV — UTF-8, разделитель `,`/`;` (авто по заголовку), кавычки RFC 4180
//!   (`""` внутри кавычек, переводы строк внутри кавычек), строка заголовков
//!   обязательна; xlsx — zip + ручной разбор XML: первый лист книги
//!   (`xl/workbook.xml` → rels → лист), `sharedStrings.xml`, типы ячеек
//!   `s`/`str`/`inlineStr`/`b`/число как строка; без формул (значение —
//!   кэш `<v>`), стилей, дат как дат и вложенных пространств имён с
//!   префиксами (читается local-имя тега); Backstage — мультидокументный
//!   YAML (`---` между документами), `kind: Component|System` → `SYS-*`,
//!   `spec.owner` → `OWNER-*`, `spec.dependsOn` → `depends_on`;
//! - маппинг заголовков CSV/xlsx гибкий (регистр и пробелы не значимы):
//!   `id` — `id`/`ид`/`идентификатор`/`system_id`; `name` (обязательна) —
//!   `name`/`title`/`system`/`system_name`/`название`/`наименование`/`имя`/
//!   `система`; `owner` — `owner`/`владелец`/`team`/`команда`;
//!   `criticality` — `criticality`/`критичность`; `status` —
//!   `status`/`статус`; `description` — `description`/`описание`;
//!   `depends_on` — `depends_on`/`dependencies`/`зависимости`/`зависит_от`
//!   (цели в ячейке — через `;`, `|` или `,`). Неизвестные колонки
//!   игнорируются с warning. `criticality`/`description` уходят в тело
//!   сущности (схема frontmatter не расширяется).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::control::kebab_slug;
use crate::error::{HarnessError, Result};
use crate::landscape::canonical_name;
use crate::model::{EntityKind, Model, load_model, parse_id};

/// Потолок строк данных реестра (защита от сбойных/гигантских выгрузок).
const MAX_IMPORT_ROWS: usize = 10_000;

/// Потолок ячеек листа xlsx (защита от бомб распаковки).
const MAX_XLSX_CELLS: usize = 100_000;

/// Потолок записей sharedStrings.xml.
const MAX_SHARED_STRINGS: usize = 50_000;

/// Формат импорта реестра систем.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryFormat {
    /// CSV-выгрузка CMDB (см. профиль в доке модуля).
    Csv,
    /// Первый лист книги xlsx (ограниченный профиль).
    Xlsx,
    /// Backstage `catalog-info.yaml` (Component/System).
    Backstage,
}

impl RegistryFormat {
    /// Формат по имени из CLI (`csv`/`xlsx`/`backstage`).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "csv" => Some(Self::Csv),
            "xlsx" => Some(Self::Xlsx),
            "backstage" => Some(Self::Backstage),
            _ => None,
        }
    }

    /// Имена всех форматов (для сообщений об ошибках).
    #[must_use]
    pub fn names() -> &'static str {
        "csv, xlsx, backstage"
    }
}

/// Опции импорта.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegistryImportOptions {
    /// Перезаписывать существующие сущности (на месте их файлов).
    pub force: bool,
    /// Только план: ничего не писать.
    pub dry_run: bool,
}

/// Отчёт импорта реестра.
#[derive(Debug)]
pub struct RegistryImportReport {
    /// Каталог модели-получателя.
    pub dir: PathBuf,
    /// Записанные файлы (при `dry_run` — планируемые).
    pub written: Vec<PathBuf>,
    /// Пропущенные (уже есть в модели): `ID (название)`.
    pub skipped: Vec<String>,
    /// Предупреждения (неразрешённые связи, пропущенные строки/документы,
    /// неизвестные колонки).
    pub warnings: Vec<String>,
    /// Режим «только план».
    pub dry_run: bool,
}

/// Строка реестра систем (нормализованный вид всех трёх форматов).
#[derive(Debug, Default)]
struct RegistryRow {
    /// Явный ID (`SYS-NNN`) из колонки `id`.
    id: Option<String>,
    /// Название системы (обязательно).
    name: String,
    /// Владелец (команда/роль, как записан).
    owner: Option<String>,
    /// Критичность (уходит в тело сущности).
    criticality: Option<String>,
    /// Статус (иначе `imported`).
    status: Option<String>,
    /// Описание (тело сущности).
    description: Option<String>,
    /// Цели зависимостей: ID или названия.
    depends_on: Vec<String>,
    /// Дополнительный алиас для разрешения ссылок (Backstage
    /// `metadata.name`, когда показное название взято из `metadata.title`).
    alias: Option<String>,
    /// Заметки происхождения для тела (Backstage kind/system и т.п.).
    notes: Vec<String>,
}

/// Существующее содержимое целевого каталога модели (индекс идемпотентности).
struct ExistingModel {
    /// `id` → файл сущности.
    by_id: BTreeMap<String, PathBuf>,
    /// Нормализованное название → `id` системы (только SYS).
    sys_by_title: BTreeMap<String, String>,
    /// Нормализованное название → `id` владельца (только OWNER).
    owner_by_title: BTreeMap<String, String>,
    /// Занятые номера по префиксам (`SYS`, `OWNER`).
    used_numbers: BTreeMap<&'static str, BTreeSet<u64>>,
}

impl ExistingModel {
    /// Читает каталог `dir`; отсутствующий каталог — пустой индекс.
    fn load(dir: &Path) -> Result<Self> {
        let mut by_id = BTreeMap::new();
        let mut sys_by_title = BTreeMap::new();
        let mut owner_by_title = BTreeMap::new();
        let mut used_numbers: BTreeMap<&'static str, BTreeSet<u64>> = BTreeMap::new();
        if dir.is_dir() {
            let model: Model = load_model(dir)?;
            for e in &model.entities {
                by_id.insert(e.id.clone(), e.file.clone());
                let key = canonical_name(&e.title);
                match e.kind {
                    EntityKind::Sys if !key.is_empty() => {
                        sys_by_title.entry(key).or_insert_with(|| e.id.clone());
                    }
                    EntityKind::Owner if !key.is_empty() => {
                        owner_by_title.entry(key).or_insert_with(|| e.id.clone());
                    }
                    _ => {}
                }
                if let Some((kind, n)) = parse_id(&e.id) {
                    if matches!(kind, EntityKind::Sys | EntityKind::Owner) {
                        used_numbers.entry(kind.prefix()).or_default().insert(n);
                    }
                }
            }
        }
        Ok(Self {
            by_id,
            sys_by_title,
            owner_by_title,
            used_numbers,
        })
    }

    /// Первый свободный номер для префикса (`SYS` → `SYS-001`, …).
    fn next_id(&mut self, prefix: &'static str) -> String {
        let used = self.used_numbers.entry(prefix).or_default();
        let mut n = 1u64;
        while used.contains(&n) {
            n += 1;
        }
        used.insert(n);
        format!("{prefix}-{n:03}")
    }
}

/// Черновик новой сущности для записи.
struct NewEntity {
    /// Назначенный ID.
    id: String,
    /// Тип (`sys`/`owner`).
    kind: EntityKind,
    /// Название.
    title: String,
    /// Статус.
    status: String,
    /// Разрешённые `depends_on` (ID).
    depends_on: Vec<String>,
    /// Тело документа.
    body: String,
}

/// Frontmatter записываемой сущности (порядок полей — как в объявлении).
#[derive(Serialize)]
struct OutFrontmatter<'a> {
    id: &'a str,
    #[serde(rename = "type")]
    kind: &'a str,
    title: &'a str,
    status: &'a str,
    date: &'a str,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    depends_on: &'a [String],
}

/// Текст файла сущности: frontmatter + тело.
fn render_entity_file(e: &NewEntity, date: &str) -> Result<String> {
    let fm = OutFrontmatter {
        id: &e.id,
        kind: e.kind.type_str(),
        title: &e.title,
        status: &e.status,
        date,
        depends_on: &e.depends_on,
    };
    let yaml = serde_yaml_ng::to_string(&fm)
        .map_err(|e| HarnessError::Model(format!("сериализация frontmatter: {e}")))?;
    let mut out = format!("---\n{yaml}---\n");
    if !e.body.is_empty() {
        let _ = write!(out, "\n{}\n", e.body);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Разбирает CSV в строки полей. Профиль: кавычки `"` с экранированием `""`,
/// переводы строк внутри кавычек; `\r\n` и одиночные `\r`/`\n` — границы
/// строк вне кавычек.
fn parse_csv_rows(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    let _ = chars.next(); // вторая кавычка пары "" потреблена
                }
                '"' => in_quotes = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            c if c == delimiter => {
                row.push(std::mem::take(&mut field));
            }
            '\n' | '\r' => {
                // \r\n — одна граница строки.
                if c == '\r' && chars.peek() == Some(&'\n') {
                    let _ = chars.next(); // \n пары CRLF потреблён
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    // Последняя строка без завершающего перевода.
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Разделитель по строке заголовков: что чаще — `,` или `;` (дефолт `,`).
fn detect_delimiter(header_line: &str) -> char {
    let commas = header_line.matches(',').count();
    let semicolons = header_line.matches(';').count();
    if semicolons > commas { ';' } else { ',' }
}

/// Маппинг колонок заголовка на поля реестра.
#[derive(Debug, Default)]
struct ColumnMap {
    id: Option<usize>,
    name: Option<usize>,
    owner: Option<usize>,
    criticality: Option<usize>,
    status: Option<usize>,
    description: Option<usize>,
    depends_on: Option<usize>,
}

/// Известные алиасы заголовков (lowercase).
const HEADER_ALIASES: [(&str, &[&str]); 7] = [
    ("id", &["id", "ид", "идентификатор", "system_id"]),
    (
        "name",
        &[
            "name",
            "title",
            "system",
            "system_name",
            "название",
            "наименование",
            "имя",
            "система",
        ],
    ),
    ("owner", &["owner", "владелец", "team", "команда"]),
    ("criticality", &["criticality", "критичность"]),
    ("status", &["status", "статус"]),
    ("description", &["description", "описание"]),
    (
        "depends_on",
        &["depends_on", "dependencies", "зависимости", "зависит_от"],
    ),
];

/// Строит маппинг колонок; неизвестные заголовки — в `warnings`.
fn map_columns(header: &[String], warnings: &mut Vec<String>) -> Result<ColumnMap> {
    let mut map = ColumnMap::default();
    for (i, raw) in header.iter().enumerate() {
        let h = raw.trim().to_lowercase();
        if h.is_empty() {
            continue;
        }
        let mut known = false;
        for (field, aliases) in &HEADER_ALIASES {
            if aliases.contains(&h.as_str()) {
                let slot = match *field {
                    "id" => &mut map.id,
                    "name" => &mut map.name,
                    "owner" => &mut map.owner,
                    "criticality" => &mut map.criticality,
                    "status" => &mut map.status,
                    "description" => &mut map.description,
                    _ => &mut map.depends_on,
                };
                if slot.replace(i).is_some() {
                    warnings.push(format!(
                        "колонка '{h}' встречена дважды — используется последняя ({i})"
                    ));
                }
                known = true;
                break;
            }
        }
        if !known {
            warnings.push(format!("неизвестная колонка '{h}' — игнорируется"));
        }
    }
    if map.name.is_none() {
        return Err(HarnessError::Model(
            "в заголовке нет колонки названия системы (допустимы: name, title, system, \
             system_name, название, наименование, имя, система)"
                .into(),
        ));
    }
    Ok(map)
}

/// Непустое значение ячейки (пробельное = отсутствует).
fn cell(row: &[String], idx: Option<usize>) -> Option<String> {
    idx.and_then(|i| row.get(i))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Цели зависимостей из ячейки (разделители `;`, `|`, `,`).
fn split_depends_on(raw: &str) -> Vec<String> {
    raw.split([';', '|', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Строки таблицы (первый ряд — заголовок) → строки реестра.
fn rows_from_table(table: &[Vec<String>], warnings: &mut Vec<String>) -> Result<Vec<RegistryRow>> {
    let Some(header) = table.first() else {
        return Err(HarnessError::Model(
            "реестр пуст: нет строки заголовков".into(),
        ));
    };
    let map = map_columns(header, warnings)?;
    let mut rows = Vec::new();
    for (n, row) in table.iter().enumerate().skip(1) {
        if rows.len() >= MAX_IMPORT_ROWS {
            warnings.push(format!(
                "превышен лимит {MAX_IMPORT_ROWS} строк — остальные пропущены"
            ));
            break;
        }
        if row.iter().all(|c| c.trim().is_empty()) {
            continue; // пустая строка — не данные
        }
        let Some(name) = cell(row, map.name) else {
            warnings.push(format!("строка {}: пустое название — пропущена", n + 1));
            continue;
        };
        let depends_on = cell(row, map.depends_on)
            .map(|r| split_depends_on(&r))
            .unwrap_or_default();
        rows.push(RegistryRow {
            id: cell(row, map.id),
            name,
            owner: cell(row, map.owner),
            criticality: cell(row, map.criticality),
            status: cell(row, map.status),
            description: cell(row, map.description),
            depends_on,
            alias: None,
            notes: Vec::new(),
        });
    }
    Ok(rows)
}

/// Парсит CSV-текст реестра.
fn parse_csv(text: &str, warnings: &mut Vec<String>) -> Result<Vec<RegistryRow>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let header_line = text.lines().next().unwrap_or("");
    let table = parse_csv_rows(text, detect_delimiter(header_line));
    rows_from_table(&table, warnings)
}

// ---------------------------------------------------------------------------
// xlsx (ограниченный профиль: zip + ручной разбор XML)
// ---------------------------------------------------------------------------

/// Раскодирование XML-сущностей (`&lt;`, `&amp;`, `&#NN;`, `&#xHH;`, …).
fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let Some(end) = rest[i..].find(';') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let entity = &rest[i + 1..i + end];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))
                .and_then(char::from_u32),
        };
        if let Some(c) = decoded {
            out.push(c);
        } else {
            out.push('&');
            out.push_str(entity);
            out.push(';');
        }
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    out
}

/// Блоки `<name …>…</name>` ограниченного профиля: без учёта комментариев
/// и CDATA с разметкой, тег с тем же именем не вложен сам в себя (в
/// профиле xlsx — `c`/`v`/`t`/`is`/`si`/`row`/`sheet`/`Relationship` это
/// выполнено). Возвращает пары (сырые атрибуты, содержимое).
fn element_blocks<'a>(xml: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut pos = 0usize;
    while let Some(off) = xml[pos..].find(&open) {
        let start = pos + off;
        let after = start + open.len();
        // Символ после имени обязан закрывать тег (иначе это `<name2…`).
        match xml[after..].chars().next() {
            Some(c) if c == '>' || c == '/' || c.is_whitespace() => {}
            _ => {
                pos = after;
                continue;
            }
        }
        let Some(tag_end) = xml[after..].find('>') else {
            break;
        };
        let tag = xml[after..after + tag_end].trim_end();
        let self_closing = tag.ends_with('/');
        let attrs = tag.strip_suffix('/').unwrap_or(tag).trim();
        if self_closing {
            out.push((attrs, ""));
            pos = after + tag_end + 1;
            continue;
        }
        let inner_start = after + tag_end + 1;
        let Some(close_off) = xml[inner_start..].find(&close) else {
            break;
        };
        out.push((attrs, &xml[inner_start..inner_start + close_off]));
        pos = inner_start + close_off + close.len();
    }
    out
}

/// Значение атрибута (`name="…"`/`name='…'`); слева от имени — граница
/// слова (начало строки/пробел), чтобы `id` не находился внутри `r:id`.
fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let mut search_from = 0usize;
    while let Some(rel) = attrs[search_from..].find(name) {
        let start = search_from + rel;
        let after = start + name.len();
        let left_ok = start == 0 || attrs.as_bytes()[start - 1].is_ascii_whitespace();
        let tail = attrs[after..].trim_start();
        if left_ok && tail.starts_with('=') {
            let value = tail[1..].trim_start();
            let quote = value.chars().next()?;
            if quote != '"' && quote != '\'' {
                return None;
            }
            let value = &value[1..];
            let end = value.find(quote)?;
            return Some(&value[..end]);
        }
        search_from = after;
    }
    None
}

/// Координаты ячейки из ссылки `r` (`BC7` → (строка 6, колонка 54), 0-based).
fn cell_ref_coords(cell_ref: &str) -> Option<(usize, usize)> {
    let letters: String = cell_ref
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    let digits: String = cell_ref
        .chars()
        .skip_while(char::is_ascii_alphabetic)
        .collect();
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let mut col = 0usize;
    for c in letters.chars() {
        col = col * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    let row: usize = digits.parse().ok()?;
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

/// Читает zip-член как строку UTF-8; `Ok(None)` — члена нет.
fn zip_member_string<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<String>> {
    match zip.by_name(name) {
        Ok(mut member) => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut member, &mut text)
                .map_err(|e| HarnessError::Model(format!("xlsx: чтение {name}: {e}")))?;
            Ok(Some(text))
        }
        Err(zip::result::ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(HarnessError::Model(format!("xlsx: {name}: {e}"))),
    }
}

/// Путь первого листа книги: `xl/workbook.xml` (первый `<sheet>`) →
/// `xl/_rels/workbook.xml.rels` (Id → Target); без r:id — запасной
/// `xl/worksheets/sheet1.xml` (каноничная раскладка Excel).
fn first_sheet_path(workbook: &str, rels: Option<&str>) -> String {
    let fallback = "xl/worksheets/sheet1.xml".to_string();
    let Some((sheet_attrs, _)) = element_blocks(workbook, "sheet").into_iter().next() else {
        return fallback;
    };
    let Some(rid) = attr(sheet_attrs, "r:id").or_else(|| attr(sheet_attrs, "id")) else {
        return fallback;
    };
    let Some(rels) = rels else { return fallback };
    for (rel_attrs, _) in element_blocks(rels, "Relationship") {
        if attr(rel_attrs, "Id") != Some(rid) {
            continue;
        }
        if let Some(target) = attr(rel_attrs, "Target") {
            if let Some(abs) = target.strip_prefix('/') {
                return abs.to_string();
            }
            return format!("xl/{target}");
        }
    }
    fallback
}

/// Парсит sharedStrings.xml: каждый `<si>` — конкатенация всех `<t>`
/// внутри (rich-text runs; фонетические `<rPh>` в профиле не встречаются —
/// задокументированное ограничение).
fn parse_shared_strings(xml: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (_, inner) in element_blocks(xml, "si") {
        if out.len() >= MAX_SHARED_STRINGS {
            return Err(HarnessError::Model(format!(
                "xlsx: sharedStrings превышает лимит {MAX_SHARED_STRINGS} записей"
            )));
        }
        let text: String = element_blocks(inner, "t")
            .into_iter()
            .map(|(_, t)| xml_unescape(t))
            .collect();
        out.push(text);
    }
    Ok(out)
}

/// Парсит лист xlsx в таблицу строк (пустые хвосты ячеек срезаются,
/// полностью пустые строки сохраняют позицию — номера строк важны для
/// сообщений об ошибках).
fn parse_sheet(xml: &str, shared: &[String]) -> Result<Vec<Vec<String>>> {
    let mut grid: BTreeMap<(usize, usize), String> = BTreeMap::new();
    let mut max_row = 0usize;
    for (row_pos, (row_attrs, row_inner)) in element_blocks(xml, "row").into_iter().enumerate() {
        let row_idx = attr(row_attrs, "r").and_then(|r| r.parse::<usize>().ok());
        let mut prev_col: Option<usize> = None;
        for (cell_attrs, cell_inner) in element_blocks(row_inner, "c") {
            if grid.len() >= MAX_XLSX_CELLS {
                return Err(HarnessError::Model(format!(
                    "xlsx: лист превышает лимит {MAX_XLSX_CELLS} ячеек"
                )));
            }
            let (r, c) = match attr(cell_attrs, "r").and_then(cell_ref_coords) {
                Some((r, c)) => (r, c),
                None => (
                    row_idx.map_or(row_pos, |r| r.saturating_sub(1)),
                    prev_col.map_or(0, |p| p + 1),
                ),
            };
            prev_col = Some(c);
            max_row = max_row.max(r);
            let value = match attr(cell_attrs, "t") {
                Some("s") => element_blocks(cell_inner, "v")
                    .first()
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    .and_then(|i| shared.get(i))
                    .cloned()
                    .unwrap_or_default(),
                Some("inlineStr") => element_blocks(cell_inner, "t")
                    .into_iter()
                    .map(|(_, t)| xml_unescape(t))
                    .collect(),
                // `str` (строка формулы), `b` (булево), без типа (число) —
                // кэшированное значение `<v>` как строка.
                _ => element_blocks(cell_inner, "v")
                    .first()
                    .map_or_else(String::new, |(_, v)| xml_unescape(v)),
            };
            if !value.is_empty() {
                grid.insert((r, c), value);
            }
        }
    }
    let mut rows = vec![Vec::new(); max_row + 1];
    for ((r, c), v) in grid {
        if r < rows.len() {
            let row = &mut rows[r];
            if row.len() <= c {
                row.resize(c + 1, String::new());
            }
            row[c] = v;
        }
    }
    for row in &mut rows {
        while row.last().is_some_and(String::is_empty) {
            row.pop();
        }
    }
    Ok(rows)
}

/// Парсит первый лист книги xlsx.
fn parse_xlsx(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<RegistryRow>> {
    let file = std::fs::File::open(path).map_err(|e| HarnessError::io(path, e))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| HarnessError::Model(format!("{}: не xlsx (zip): {e}", path.display())))?;
    let workbook = zip_member_string(&mut zip, "xl/workbook.xml")?
        .ok_or_else(|| HarnessError::Model("xlsx: нет xl/workbook.xml".to_string()))?;
    let rels = zip_member_string(&mut zip, "xl/_rels/workbook.xml.rels")?;
    let sheet_path = first_sheet_path(&workbook, rels.as_deref());
    let sheet = zip_member_string(&mut zip, &sheet_path)?.ok_or_else(|| {
        HarnessError::Model(format!(
            "xlsx: первый лист '{sheet_path}' не найден в архиве"
        ))
    })?;
    let shared = match zip_member_string(&mut zip, "xl/sharedStrings.xml")? {
        Some(s) => parse_shared_strings(&s)?,
        None => Vec::new(),
    };
    let table = parse_sheet(&sheet, &shared)?;
    rows_from_table(&table, warnings)
}

// ---------------------------------------------------------------------------
// Backstage catalog-info.yaml
// ---------------------------------------------------------------------------

/// Последний сегмент Backstage-ссылки: `group:default/payments` →
/// `payments`, `component:payments-api` → `payments-api`.
fn backstage_ref_name(reference: &str) -> String {
    reference
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(reference.trim())
        .split(':')
        .next_back()
        .unwrap_or(reference.trim())
        .trim()
        .to_string()
}

/// Разбирает Backstage-каталог (мультидокументный YAML; документы
/// разделяются строкой `---`). `kind: Component|System` → строка реестра;
/// прочие kind — пропуск с warning.
fn parse_backstage(text: &str, warnings: &mut Vec<String>) -> Result<Vec<RegistryRow>> {
    let mut rows = Vec::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    for (n, chunk) in text.split("\n---").enumerate() {
        // Директивы YAML (`%YAML …`) в начале документа срезаются.
        let body: String = chunk
            .lines()
            .skip_while(|l| l.trim_start().starts_with('%'))
            .collect::<Vec<_>>()
            .join("\n");
        if body.trim().is_empty() || body.trim() == "---" {
            continue;
        }
        let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                return Err(HarnessError::Model(format!(
                    "backstage: документ #{} не разбирается: {e}",
                    n + 1
                )));
            }
        };
        let kind = doc.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(kind, "Component" | "System") {
            if !kind.is_empty() {
                *skipped.entry(kind.to_string()).or_default() += 1;
            }
            continue;
        }
        if rows.len() >= MAX_IMPORT_ROWS {
            warnings.push(format!(
                "превышен лимит {MAX_IMPORT_ROWS} сущностей — остальные пропущены"
            ));
            break;
        }
        let metadata = doc
            .get("metadata")
            .cloned()
            .unwrap_or(serde_yaml_ng::Value::Null);
        let spec = doc
            .get("spec")
            .cloned()
            .unwrap_or(serde_yaml_ng::Value::Null);
        let get = |v: &serde_yaml_ng::Value, key: &str| -> Option<String> {
            v.get(key)
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let name = get(&metadata, "name");
        let title = get(&metadata, "title").or_else(|| name.clone());
        let Some(title) = title else {
            warnings.push(format!(
                "backstage: документ #{n} без metadata.name — пропущен"
            ));
            continue;
        };
        let depends_on = spec
            .get("dependsOn")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|r| r.as_str())
                    .map(backstage_ref_name)
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let mut notes = vec![format!("Backstage kind: {kind}")];
        if let Some(system) = get(&spec, "system") {
            notes.push(format!("Backstage system: {system}"));
        }
        rows.push(RegistryRow {
            id: None,
            alias: name.filter(|n| *n != title),
            name: title,
            owner: get(&spec, "owner").map(|r| backstage_ref_name(&r)),
            criticality: None,
            status: get(&spec, "lifecycle"),
            description: get(&metadata, "description"),
            depends_on,
            notes,
        });
    }
    for (kind, count) in skipped {
        warnings.push(format!(
            "backstage: kind '{kind}' не Component/System — пропущено документов: {count}"
        ));
    }
    if rows.is_empty() {
        return Err(HarnessError::Model(
            "backstage: ни одного документа kind: Component/System — нечего импортировать".into(),
        ));
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Сборка и запись
// ---------------------------------------------------------------------------

/// Импортирует реестр систем из `file` в каталог модели `dir`.
///
/// # Errors
/// Файл не читается/не разбирается в заявленном формате, нет колонки
/// названия, каталог модели не создаётся, ошибки записи.
pub fn import_registry(
    file: &Path,
    dir: &Path,
    format: RegistryFormat,
    opts: &RegistryImportOptions,
) -> Result<RegistryImportReport> {
    let text = matches!(format, RegistryFormat::Csv | RegistryFormat::Backstage)
        .then(|| std::fs::read_to_string(file).map_err(|e| HarnessError::io(file, e)))
        .transpose()?;
    let mut warnings = Vec::new();
    let rows = match format {
        RegistryFormat::Csv => parse_csv(text.as_deref().unwrap_or(""), &mut warnings)?,
        RegistryFormat::Xlsx => parse_xlsx(file, &mut warnings)?,
        RegistryFormat::Backstage => parse_backstage(text.as_deref().unwrap_or(""), &mut warnings)?,
    };
    if rows.is_empty() {
        return Err(HarnessError::Model(format!(
            "{}: ни одной строки с названием системы — нечего импортировать",
            file.display()
        )));
    }

    let mut existing = ExistingModel::load(dir)?;
    // Индексы разрешения ссылок по содержимому ЭТОГО файла.
    let mut file_by_title: BTreeMap<String, String> = BTreeMap::new();
    let mut file_ids: BTreeSet<String> = BTreeSet::new();
    let mut file_explicit: BTreeSet<String> = BTreeSet::new();

    // Назначение ID: явный (валидный SYS-NNN) сохраняется, иначе —
    // очередной свободный. Дубли явных ID внутри файла — warning + пропуск
    // строки. Строки, чей ID/название уже есть в модели, получают ID
    // существующей сущности (skip/force решится при записи).
    let mut assigned: Vec<Option<String>> = Vec::with_capacity(rows.len());
    for (n, row) in rows.iter().enumerate() {
        // Явный id резервирует номер, чтобы авто-назначение не заняло его.
        let reserve = |existing: &mut ExistingModel, raw: &str| {
            if let Some((EntityKind::Sys, num)) = parse_id(raw) {
                existing.used_numbers.entry("SYS").or_default().insert(num);
            }
        };
        let id = match &row.id {
            Some(raw) => match parse_id(raw) {
                Some((EntityKind::Sys, _)) if existing.by_id.contains_key(raw) => raw.clone(),
                Some((EntityKind::Sys, _)) if !file_explicit.insert(raw.clone()) => {
                    warnings.push(format!(
                        "строка {}: дубль id '{raw}' в файле — пропущена",
                        n + 2
                    ));
                    assigned.push(None);
                    continue;
                }
                Some((EntityKind::Sys, _)) => {
                    reserve(&mut existing, raw);
                    raw.clone()
                }
                Some(_) => {
                    warnings.push(format!(
                        "строка {}: id '{raw}' не SYS-* — назначен автоматически",
                        n + 2
                    ));
                    existing.next_id("SYS")
                }
                None => {
                    warnings.push(format!(
                        "строка {}: id '{raw}' не по форме PREFIX-NNN — назначен автоматически",
                        n + 2
                    ));
                    existing.next_id("SYS")
                }
            },
            None => {
                // Идемпотентность по названию: та же система — тот же ID.
                match existing.sys_by_title.get(&canonical_name(&row.name)) {
                    Some(id) => id.clone(),
                    None => existing.next_id("SYS"),
                }
            }
        };
        // Явный id указывает на одну сущность, название — на другую:
        // видимый конфликт реестра с моделью.
        if let Some(other) = existing.sys_by_title.get(&canonical_name(&row.name)) {
            if *other != id {
                warnings.push(format!(
                    "строка {}: id '{id}' занят одной сущностью, а название '{}' — другой \
                     ({other}); запись по id",
                    n + 2,
                    row.name
                ));
            }
        }
        file_ids.insert(id.clone());
        for key in [&row.name, row.alias.as_deref().unwrap_or("")] {
            let key = canonical_name(key);
            if !key.is_empty() {
                file_by_title.entry(key).or_insert_with(|| id.clone());
            }
        }
        assigned.push(Some(id));
    }

    // Разрешение ссылки: ID (есть в файле/модели) или название
    // (файл, затем существующая модель).
    let resolve = |target: &str| -> Option<String> {
        let t = target.trim();
        if parse_id(t).is_some_and(|(k, _)| k == EntityKind::Sys) {
            return (file_ids.contains(t) || existing.by_id.contains_key(t)).then(|| t.to_string());
        }
        let key = canonical_name(t);
        file_by_title
            .get(&key)
            .cloned()
            .or_else(|| existing.sys_by_title.get(&key).cloned())
    };

    let today = chrono::Local::now().date_naive().to_string();
    let source_name = file.file_name().map_or_else(
        || file.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );

    // Разрешение depends_on — отдельным проходом, чтобы освободить
    // иммутабельный заём `existing` до назначения ID владельцам.
    let mut resolved_deps: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for (row, id) in rows.iter().zip(assigned.iter()) {
        let mut depends_on = Vec::new();
        if let Some(id) = id {
            for target in &row.depends_on {
                match resolve(target) {
                    Some(t) if &t != id && !depends_on.contains(&t) => depends_on.push(t),
                    Some(_) => {} // самоссылка — молча отбрасываем
                    None => warnings.push(format!(
                        "{id}: цель depends_on '{target}' не найдена ни в файле, ни в модели — \
                         связь пропущена"
                    )),
                }
            }
            depends_on.sort();
        }
        resolved_deps.push(depends_on);
    }
    let _ = resolve; // конец иммутабельного заёма `existing` (дальше — next_id для владельцев)

    // Сущности SYS из строк.
    let mut drafts: Vec<NewEntity> = Vec::new();
    let mut owner_drafts: Vec<NewEntity> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    // Владелец строки → ID OWNER (созданные в этом прогоне).
    let mut owner_ids: BTreeMap<String, String> = BTreeMap::new();
    for ((row, id), depends_on) in rows.iter().zip(assigned.iter()).zip(resolved_deps) {
        let Some(id) = id.clone() else { continue };
        let mut body_lines: Vec<String> = Vec::new();
        if let Some(d) = &row.description {
            body_lines.push(d.clone());
        }
        if let Some(c) = &row.criticality {
            body_lines.push(format!("Критичность: {c}"));
        }
        if let Some(owner) = &row.owner {
            let key = canonical_name(owner);
            let owner_id = owner_ids.get(&key).cloned().or_else(|| {
                existing.owner_by_title.get(&key).cloned().inspect(|oid| {
                    // Владелец уже в модели — видимый skip (перезапись
                    // владельцев не производится даже под --force: они не
                    // несут полей реестра).
                    skipped.push(format!("{oid} ({owner})"));
                })
            });
            let owner_id = if let Some(oid) = owner_id {
                oid
            } else {
                let oid = existing.next_id("OWNER");
                owner_drafts.push(NewEntity {
                    id: oid.clone(),
                    kind: EntityKind::Owner,
                    title: owner.clone(),
                    status: "imported".to_string(),
                    depends_on: Vec::new(),
                    body: format!("Импортировано из реестра `{source_name}`."),
                });
                owner_ids.insert(key, oid.clone());
                oid
            };
            body_lines.push(format!("Владелец: {owner_id} «{owner}»"));
        }
        body_lines.extend(row.notes.iter().cloned());
        body_lines.push(format!("Импортировано из реестра `{source_name}`."));
        drafts.push(NewEntity {
            id,
            kind: EntityKind::Sys,
            title: row.name.clone(),
            status: row.status.clone().unwrap_or_else(|| "imported".to_string()),
            depends_on,
            body: body_lines.join("\n\n"),
        });
    }
    drafts.extend(owner_drafts);

    // Запись: skip (есть такой ID) / force (перезапись на месте) / новый файл.
    if !opts.dry_run {
        std::fs::create_dir_all(dir).map_err(|e| HarnessError::io(dir, e))?;
    }
    let mut written = Vec::new();
    for d in &drafts {
        let existing_file = existing.by_id.get(&d.id);
        if let Some(path) = existing_file {
            if !opts.force {
                skipped.push(format!("{} ({})", d.id, d.title));
                continue;
            }
            // --force: перезапись НА МЕСТЕ существующего файла.
            let text = render_entity_file(d, &today)?;
            if !opts.dry_run {
                std::fs::write(path, text).map_err(|e| HarnessError::io(path, e))?;
            }
            written.push(path.clone());
            continue;
        }
        let path = dir.join(format!("{}-{}.md", d.id, kebab_slug(&d.title)));
        let text = render_entity_file(d, &today)?;
        if !opts.dry_run {
            std::fs::write(&path, text).map_err(|e| HarnessError::io(&path, e))?;
        }
        written.push(path);
    }
    Ok(RegistryImportReport {
        dir: dir.to_path_buf(),
        written,
        skipped,
        warnings,
        dry_run: opts.dry_run,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный xlsx программно: zip с workbook/rels/sharedStrings/sheet.
    fn write_xlsx(path: &Path, shared: &[&str], rows: &[Vec<(char, u32, &str)>]) {
        let file = std::fs::File::create(path).expect("xlsx");
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let workbook = r#"<?xml version="1.0"?><workbook xmlns:r="urn:r"><sheets><sheet name="Реестр" sheetId="1" r:id="rId2"/></sheets></workbook>"#;
        zip.start_file("xl/workbook.xml", options).expect("start");
        std::io::Write::write_all(&mut zip, workbook.as_bytes()).expect("write");
        let rels = r#"<?xml version="1.0"?><Relationships><Relationship Id="rId2" Type="worksheet" Target="worksheets/main.xml"/></Relationships>"#;
        zip.start_file("xl/_rels/workbook.xml.rels", options)
            .expect("start");
        std::io::Write::write_all(&mut zip, rels.as_bytes()).expect("write");
        let mut ss = String::from(r#"<?xml version="1.0"?><sst>"#);
        for s in shared {
            let _ = write!(
                ss,
                "<si><t>{}</t></si>",
                s.replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            );
        }
        ss.push_str("</sst>");
        zip.start_file("xl/sharedStrings.xml", options)
            .expect("start");
        std::io::Write::write_all(&mut zip, ss.as_bytes()).expect("write");
        let mut sheet = String::from(r#"<?xml version="1.0"?><worksheet><sheetData>"#);
        let mut by_row: BTreeMap<u32, Vec<(char, &str)>> = BTreeMap::new();
        for cells in rows {
            for (col, r, v) in cells {
                by_row.entry(*r).or_default().push((*col, *v));
            }
        }
        for (r, cells) in by_row {
            let _ = write!(sheet, "<row r=\"{r}\">");
            for (col, v) in cells {
                let _ = write!(sheet, "<c r=\"{col}{r}\" t=\"s\"><v>{v}</v></c>");
            }
            sheet.push_str("</row>");
        }
        sheet.push_str("</sheetData></worksheet>");
        zip.start_file("xl/worksheets/main.xml", options)
            .expect("start");
        std::io::Write::write_all(&mut zip, sheet.as_bytes()).expect("write");
        zip.finish().expect("finish");
    }

    #[test]
    fn csv_import_basic_and_idempotent() {
        let dir = tempfile::tempdir().expect("tmp");
        let csv = dir.path().join("registry.csv");
        std::fs::write(
            &csv,
            "id,name,owner,criticality,status,description,depends_on\n\
             SYS-001,Процессинг,core-team,high,active,\"Карточный процессинг, АБС\",\n\
             SYS-007,Шлюз СБП,channel-team,,active,Приём C2B,SYS-001\n\
             ,Фрод-монитор,risk-team,low,,Скоринг операций,Процессинг | Шлюз СБП\n",
        )
        .expect("csv");
        let out = dir.path().join("model");
        let report = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions::default(),
        )
        .expect("импорт");
        assert_eq!(report.written.len(), 6, "3 SYS + 3 OWNER: {report:?}");
        assert!(report.skipped.is_empty());
        assert!(
            report.warnings.iter().all(|w| !w.contains("depends_on")),
            "{:?}",
            report.warnings
        );
        // Явные id сохранены, авто-id — после занятых номеров.
        let model = load_model(&out).expect("модель");
        let gw = model.get("SYS-007").expect("шлюз");
        assert_eq!(gw.depends_on, ["SYS-001"]);
        let fraud = model.get("SYS-002").expect("авто-id после SYS-001");
        assert_eq!(fraud.title, "Фрод-монитор");
        assert_eq!(fraud.depends_on, ["SYS-001", "SYS-007"]);
        assert_eq!(fraud.status, "imported", "дефолтный статус");
        assert!(fraud.body.contains("Критичность: low"), "{}", fraud.body);
        let owner = model.get("OWNER-001").expect("владелец");
        assert_eq!(owner.title, "core-team");

        // Повторный импорт — полный skip (идемпотентность).
        let report2 = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions::default(),
        )
        .expect("повторный импорт");
        assert!(report2.written.is_empty(), "{:?}", report2.written);
        assert_eq!(report2.skipped.len(), 6, "{:?}", report2.skipped);

        // dry-run после --force-сценария ничего не пишет.
        let before: Vec<_> = std::fs::read_dir(&out)
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        let dry = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions {
                force: false,
                dry_run: true,
            },
        )
        .expect("dry-run");
        assert!(dry.written.is_empty() && dry.dry_run);
        let after: Vec<_> = std::fs::read_dir(&out)
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(before, after, "dry-run не трогает каталог");
    }

    #[test]
    fn csv_import_force_rewrites_in_place() {
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("model");
        std::fs::create_dir_all(&out).expect("mkdir");
        std::fs::write(
            out.join("SYS-001-staryi-slug.md"),
            "---\nid: SYS-001\ntype: sys\ntitle: Процессинг\nstatus: adopted\n---\nСтарое.\n",
        )
        .expect("существующая сущность");
        let csv = dir.path().join("r.csv");
        std::fs::write(&csv, "id,name\nSYS-001,Процессинг\n").expect("csv");
        // Без force — skip.
        let r = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions::default(),
        )
        .expect("импорт");
        assert_eq!(r.skipped.len(), 1);
        assert!(r.written.is_empty());
        // С force — перезапись НА МЕСТЕ (тот же файл, без дубля с новым slug).
        let r = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions {
                force: true,
                dry_run: false,
            },
        )
        .expect("force-импорт");
        assert_eq!(r.written, vec![out.join("SYS-001-staryi-slug.md")]);
        let files: Vec<_> = std::fs::read_dir(&out)
            .expect("dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(files.len(), 1, "дублей нет: {files:?}");
        let text = std::fs::read_to_string(out.join("SYS-001-staryi-slug.md")).expect("чтение");
        assert!(text.contains("status: imported"), "{text}");
    }

    #[test]
    fn csv_profile_quotes_semicolon_and_unknown_columns() {
        let dir = tempfile::tempdir().expect("tmp");
        let csv = dir.path().join("r.csv");
        // Разделитель ';', кавычки с "" и переводом строки, лишняя колонка.
        std::fs::write(
            &csv,
            "name;owner;unknown\n\"АБС \"\"Главная\"\"\";team-a;x1\n\"Многострочное\nназвание\";team-b;x2\n;team-c;x3\n",
        )
        .expect("csv");
        let out = dir.path().join("model");
        let r = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions::default(),
        )
        .expect("импорт");
        assert_eq!(r.written.len(), 4, "2 SYS + 2 OWNER: {r:?}");
        assert!(
            r.warnings.iter().any(|w| w.contains("неизвестная колонка")),
            "{:?}",
            r.warnings
        );
        assert!(
            r.warnings.iter().any(|w| w.contains("пустое название")),
            "{:?}",
            r.warnings
        );
        let model = load_model(&out).expect("модель");
        let quoted = model.get("SYS-001").expect("кавычки");
        assert_eq!(quoted.title, "АБС \"Главная\"");
        let multiline = model.get("SYS-002").expect("многострочное");
        assert_eq!(multiline.title, "Многострочное\nназвание");
    }

    #[test]
    fn csv_without_name_column_is_clear_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let csv = dir.path().join("r.csv");
        std::fs::write(&csv, "id,owner\nSYS-001,team\n").expect("csv");
        let err = import_registry(
            &csv,
            &dir.path().join("model"),
            RegistryFormat::Csv,
            &RegistryImportOptions::default(),
        )
        .expect_err("нет колонки названия");
        assert!(err.to_string().contains("названия"), "{err}");
    }

    #[test]
    fn xlsx_import_first_sheet() {
        let dir = tempfile::tempdir().expect("tmp");
        let xlsx = dir.path().join("registry.xlsx");
        // shared: 0 name, 1 Процессинг & АБС, 2 owner, 3 core, 4 Шлюз, 5 status,
        // 6 active, 7 depends_on, 8 SYS-001 (цель связи по названию ниже).
        write_xlsx(
            &xlsx,
            &[
                "name",
                "owner",
                "status",
                "depends_on",
                "Процессинг & АБС",
                "core",
                "active",
                "Шлюз <СБП>",
                "edge",
                "Процессинг & АБС",
            ],
            &[
                vec![('A', 1, "0"), ('B', 1, "1"), ('C', 1, "2"), ('D', 1, "3")],
                vec![('A', 2, "4"), ('B', 2, "5"), ('C', 2, "6")],
                // Дырка в строке 3 (пропущена) + ячейка вне порядка.
                vec![('A', 4, "7"), ('B', 4, "8"), ('D', 4, "9")],
            ],
        );
        let out = dir.path().join("model");
        let r = import_registry(
            &xlsx,
            &out,
            RegistryFormat::Xlsx,
            &RegistryImportOptions::default(),
        )
        .expect("xlsx-импорт");
        assert_eq!(r.written.len(), 4, "2 SYS + 2 OWNER: {r:?}");
        let model = load_model(&out).expect("модель");
        let first = model.get("SYS-001").expect("первая строка");
        assert_eq!(
            first.title, "Процессинг & АБС",
            "XML-сущности раскодированы"
        );
        assert_eq!(first.status, "active");
        let second = model.get("SYS-002").expect("вторая строка");
        assert_eq!(second.title, "Шлюз <СБП>");
        assert_eq!(second.depends_on, ["SYS-001"], "связь по названию");
        // Идемпотентность и здесь.
        let r2 = import_registry(
            &xlsx,
            &out,
            RegistryFormat::Xlsx,
            &RegistryImportOptions::default(),
        )
        .expect("повтор");
        assert_eq!(r2.skipped.len(), 4, "{:?}", r2.skipped);
    }

    #[test]
    fn backstage_import_component_system_and_links() {
        let dir = tempfile::tempdir().expect("tmp");
        let yaml = dir.path().join("catalog-info.yaml");
        std::fs::write(
            &yaml,
            "apiVersion: backstage.io/v1alpha1\n\
             kind: Component\n\
             metadata:\n  name: payments-api\n  title: Платёжный API\n  description: Приём платежей\n\
             spec:\n  type: service\n  lifecycle: production\n  owner: group:default/payments-team\n  \
             dependsOn:\n    - resource:default/payments-db\n    - component:fraud-check\n\
             ---\n\
             apiVersion: backstage.io/v1alpha1\n\
             kind: System\n\
             metadata:\n  name: fraud-check\n\
             spec:\n  owner: user:risk\n  lifecycle: experimental\n\
             ---\n\
             apiVersion: backstage.io/v1alpha1\n\
             kind: API\n\
             metadata:\n  name: payments-openapi\n",
        )
        .expect("yaml");
        let out = dir.path().join("model");
        let r = import_registry(
            &yaml,
            &out,
            RegistryFormat::Backstage,
            &RegistryImportOptions::default(),
        )
        .expect("backstage-импорт");
        assert_eq!(r.written.len(), 4, "2 SYS + 2 OWNER: {r:?}");
        assert!(
            r.warnings.iter().any(|w| w.contains("'API'")),
            "kind API пропущен с warning: {:?}",
            r.warnings
        );
        assert!(
            r.warnings.iter().any(|w| w.contains("payments-db")),
            "неразрешённая цель — warning: {:?}",
            r.warnings
        );
        let model = load_model(&out).expect("модель");
        let api = model.get("SYS-001").expect("компонент");
        assert_eq!(api.title, "Платёжный API");
        assert_eq!(api.status, "production", "lifecycle → status");
        assert_eq!(
            api.depends_on,
            ["SYS-002"],
            "component:fraud-check → SYS-002"
        );
        assert!(
            api.body.contains("Backstage kind: Component"),
            "{}",
            api.body
        );
        assert!(api.body.contains("OWNER-001"), "{}", api.body);
        let fraud = model.get("SYS-002").expect("система");
        assert_eq!(fraud.title, "fraud-check", "без title — metadata.name");
        let owner = model.get("OWNER-001").expect("владелец");
        assert_eq!(owner.title, "payments-team", "ref → последний сегмент");
        // Повтор — skip по нормализованному названию.
        let r2 = import_registry(
            &yaml,
            &out,
            RegistryFormat::Backstage,
            &RegistryImportOptions::default(),
        )
        .expect("повтор");
        assert_eq!(r2.skipped.len(), 4, "{:?}", r2.skipped);
        assert!(r2.written.is_empty());
    }

    #[test]
    fn backstage_empty_is_error() {
        let dir = tempfile::tempdir().expect("tmp");
        let yaml = dir.path().join("c.yaml");
        std::fs::write(&yaml, "kind: Resource\nmetadata:\n  name: db\n").expect("yaml");
        let err = import_registry(
            &yaml,
            &dir.path().join("model"),
            RegistryFormat::Backstage,
            &RegistryImportOptions::default(),
        )
        .expect_err("нет Component/System");
        assert!(err.to_string().contains("Component"), "{err}");
    }

    #[test]
    fn unresolved_and_self_dependson_are_safe() {
        let dir = tempfile::tempdir().expect("tmp");
        let csv = dir.path().join("r.csv");
        std::fs::write(
            &csv,
            "id,name,depends_on\nSYS-001,А,SYS-001 | GHOST-9 | SYS-999\n",
        )
        .expect("csv");
        let out = dir.path().join("model");
        let r = import_registry(
            &csv,
            &out,
            RegistryFormat::Csv,
            &RegistryImportOptions::default(),
        )
        .expect("импорт");
        let model = load_model(&out).expect("модель");
        assert!(
            model
                .get("SYS-001")
                .expect("сущность")
                .depends_on
                .is_empty(),
            "самоссылка и неизвестные id не попали в связи"
        );
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("GHOST-9") || w.contains("SYS-999")),
            "{:?}",
            r.warnings
        );
    }
}

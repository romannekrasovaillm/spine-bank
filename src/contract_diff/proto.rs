//! Дифф protobuf/gRPC (`.proto`, CD-P01..CD-P06): консервативный
//! построчный разбор сообщений (с вложенностью `Outer.Inner`) и сервисов.

use std::collections::{BTreeMap, BTreeSet};

use super::types::{Finding, escape_segment};

/// Потолок строк в одном `.proto`-файле (та же защита).
const MAX_PROTO_LINES: usize = 200_000;

/// Поле сообщения proto (идентичность — тег).
#[derive(Debug, Clone)]
struct ProtoField {
    /// Имя поля.
    name: String,
    /// Тип как написан (`string`, `map<string, int64>`, `acme.Money`).
    typ: String,
}

/// Сообщение proto: поля по тегу + резервирование (удаление, покрытое
/// `reserved`, — допустимая эволюция, warn вместо error).
#[derive(Debug, Default)]
struct ProtoMessage {
    /// Поля по тегу.
    fields: BTreeMap<u64, ProtoField>,
    /// Зарезервированные теги (`reserved 2, 5 to 8;`).
    reserved_tags: BTreeSet<u64>,
    /// Зарезервированные имена (`reserved "foo", "bar";`).
    reserved_names: BTreeSet<String>,
}

/// Разобранный `.proto`-файл (консервативный построчный разбор: сообщения
/// с вложенностью `Outer.Inner`, сервисы с rpc, package; enum'ы и их
/// значения не сравниваются — ограничение скелета).
#[derive(Debug, Default)]
pub(crate) struct ProtoDoc {
    /// `package a.b.v1;`.
    pub(crate) package: Option<String>,
    /// Сообщения по полному имени (`Outer.Inner`).
    messages: BTreeMap<String, ProtoMessage>,
    /// Сервисы: имя → набор rpc.
    services: BTreeMap<String, BTreeSet<String>>,
}

/// Разбирает `.proto` построчно. Невалидные/незнакомые строки пропускаются
/// (консервативный скелет: лучше пропустить, чем упасть).
pub(crate) fn parse_proto(text: &str) -> ProtoDoc {
    let field_re = regex::Regex::new(
        r"^\s*(?:optional\s+|required\s+|repeated\s+)?([A-Za-z_][\w.]*|map\s*<[^>]+>)\s+([A-Za-z_]\w*)\s*=\s*(\d+)",
    );
    // regex известной формы компилируется всегда; при сбое — пустой never-match.
    let Ok(field_re) = field_re else {
        return ProtoDoc::default();
    };
    let message_re = regex::Regex::new(r"^\s*message\s+([A-Za-z_]\w*)\s*\{?");
    let service_re = regex::Regex::new(r"^\s*service\s+([A-Za-z_]\w*)\s*\{?");
    let rpc_re = regex::Regex::new(r"^\s*rpc\s+([A-Za-z_]\w*)\s*\(");
    let package_re = regex::Regex::new(r"^\s*package\s+([\w.]+)\s*;");
    let enum_re = regex::Regex::new(r"^\s*enum\s+[A-Za-z_]\w*\s*\{?");
    let reserved_re = regex::Regex::new(r"^\s*reserved\s+(.+?)\s*;");
    let (Ok(message_re), Ok(service_re), Ok(rpc_re), Ok(package_re), Ok(enum_re), Ok(reserved_re)) = (
        message_re,
        service_re,
        rpc_re,
        package_re,
        enum_re,
        reserved_re,
    ) else {
        return ProtoDoc::default();
    };

    let mut doc = ProtoDoc::default();
    // Элемент стека блоков: (вид, имя сообщения для полей/reserved).
    // Вид: 0 — message, 1 — service, 2 — прочий (enum/oneof/…).
    let mut stack: Vec<(u8, Option<String>)> = Vec::new();
    let mut message_path: Vec<String> = Vec::new();
    let mut current_service: Option<String> = None;
    for line in text.lines().take(MAX_PROTO_LINES) {
        // Срезаем //-комментарии (внутри строк — редкость; консервативно).
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(caps) = package_re.captures(line) {
            doc.package = Some(caps[1].to_string());
            continue;
        }
        if let Some(caps) = message_re.captures(line) {
            message_path.push(caps[1].to_string());
            let full = message_path.join(".");
            doc.messages.entry(full.clone()).or_default();
            stack.push((0, Some(full)));
            continue;
        }
        if enum_re.is_match(line) {
            stack.push((2, None));
            continue;
        }
        if let Some(caps) = service_re.captures(line) {
            let name = caps[1].to_string();
            doc.services.entry(name.clone()).or_default();
            current_service = Some(name);
            stack.push((1, None));
            continue;
        }
        if line.starts_with('}') {
            if let Some((kind, _)) = stack.pop() {
                match kind {
                    // oneof/прочие блоки внутри message не снимают сообщение
                    // со стека путей — их поля относятся к сообщению.
                    0 => {
                        message_path.pop();
                    }
                    1 => current_service = None,
                    _ => {}
                }
            }
            continue;
        }
        // Поля и reserved относятся к ближайшему охватывающему message.
        if let Some(caps) = reserved_re.captures(line) {
            if let Some(msg) = nearest_message(&mut doc, &stack) {
                for part in caps[1].split(',') {
                    let part = part.trim();
                    if let Some(name) = part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                        msg.reserved_names.insert(name.to_string());
                    } else if let Some((from, to)) = part.split_once(" to ") {
                        // Диапазон «N to M» (max не разворачиваем — метка).
                        if let (Ok(a), Ok(b)) =
                            (from.trim().parse::<u64>(), to.trim().parse::<u64>())
                        {
                            for tag in a..=b.min(a + 10_000) {
                                msg.reserved_tags.insert(tag);
                            }
                        }
                    } else if let Ok(tag) = part.parse::<u64>() {
                        msg.reserved_tags.insert(tag);
                    }
                }
            }
            continue;
        }
        if let (Some(caps), Some(msg)) =
            (field_re.captures(line), nearest_message(&mut doc, &stack))
        {
            let tag: u64 = match caps[3].parse() {
                Ok(t) => t,
                Err(_) => continue,
            };
            msg.fields.insert(
                tag,
                ProtoField {
                    name: caps[2].to_string(),
                    typ: caps[1].split_whitespace().collect::<Vec<_>>().join(""),
                },
            );
            continue;
        }
        if let Some(caps) = rpc_re.captures(line) {
            if let Some(service) = &current_service {
                if let Some(rpcs) = doc.services.get_mut(service) {
                    rpcs.insert(caps[1].to_string());
                }
            }
            continue;
        }
        // oneof/любой другой блок с `{` — на стек как «прочий».
        if line.ends_with('{') {
            stack.push((2, None));
        }
    }
    doc
}

/// Ближайшее охватывающее сообщение на стеке (поля oneof относятся к нему).
fn nearest_message<'d>(
    doc: &'d mut ProtoDoc,
    stack: &[(u8, Option<String>)],
) -> Option<&'d mut ProtoMessage> {
    let full = stack.iter().rev().find(|(kind, _)| *kind == 0)?.1.clone()?;
    doc.messages.get_mut(&full)
}

/// Дифф двух `.proto`: CD-P01..CD-P05 (CD-P06 — major-правилом выше).
pub(crate) fn diff_proto(old: &str, new: &str) -> Vec<Finding> {
    let old_doc = parse_proto(old);
    let new_doc = parse_proto(new);
    let mut out = Vec::new();

    // CD-P01 (error): удалённое сообщение; CD-P05 (warn): добавленное.
    for name in old_doc.messages.keys() {
        if !new_doc.messages.contains_key(name) {
            out.push(Finding {
                severity: "error".into(),
                rule: "CD-P01".into(),
                location: format!("#/proto/message/{}", escape_segment(name)),
                message: format!("удалено сообщение «{name}»"),
            });
        }
    }
    for name in new_doc.messages.keys() {
        if !old_doc.messages.contains_key(name) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-P05".into(),
                location: format!("#/proto/message/{}", escape_segment(name)),
                message: format!("добавлено сообщение «{name}»"),
            });
        }
    }

    // Поля общих сообщений: идентичность — тег.
    for (name, old_msg) in &old_doc.messages {
        let Some(new_msg) = new_doc.messages.get(name) else {
            continue;
        };
        let loc = format!("#/proto/message/{}", escape_segment(name));
        for (tag, old_field) in &old_msg.fields {
            match new_msg.fields.get(tag) {
                None => {
                    let covered = new_msg.reserved_tags.contains(tag)
                        || new_msg.reserved_names.contains(&old_field.name);
                    if covered {
                        // Удаление с reserved — допустимая эволюция (как buf).
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-P05".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "поле «{}» (тег {tag}) удалено и зарезервировано — допустимо",
                                old_field.name
                            ),
                        });
                    } else {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P02".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "удалено поле «{}» (тег {tag}) без reserved",
                                old_field.name
                            ),
                        });
                    }
                }
                Some(new_field) => {
                    if new_field.name != old_field.name {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P02".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "тег {tag}: поле переименовано «{}» → «{}» (имя — часть JSON/текстового контракта)",
                                old_field.name, new_field.name
                            ),
                        });
                    }
                    if new_field.typ != old_field.typ {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P03".into(),
                            location: format!("{loc}/field/{tag}"),
                            message: format!(
                                "тег {tag} («{}»): тип {} → {}",
                                old_field.name, old_field.typ, new_field.typ
                            ),
                        });
                    }
                }
            }
        }
        // Перенумерация: имя осталось, тег изменился.
        for (new_tag, new_field) in &new_msg.fields {
            if let Some(old_tag) = old_msg
                .fields
                .iter()
                .find(|(t, f)| *f.name == new_field.name && **t != *new_tag)
                .map(|(t, _)| *t)
            {
                out.push(Finding {
                    severity: "error".into(),
                    rule: "CD-P02".into(),
                    location: format!("{loc}/field/{new_tag}"),
                    message: format!(
                        "поле «{}» перенумеровано: тег {old_tag} → {new_tag}",
                        new_field.name
                    ),
                });
            }
        }
        for (tag, new_field) in &new_msg.fields {
            if !old_msg.fields.contains_key(tag)
                && !old_msg.fields.values().any(|f| f.name == new_field.name)
            {
                out.push(Finding {
                    severity: "warn".into(),
                    rule: "CD-P05".into(),
                    location: format!("{loc}/field/{tag}"),
                    message: format!("добавлено поле «{}» (тег {tag})", new_field.name),
                });
            }
        }
    }

    // CD-P04 (error): удалённый сервис/rpc; CD-P05 (warn): добавленные.
    for name in old_doc.services.keys() {
        match new_doc.services.get(name) {
            None => out.push(Finding {
                severity: "error".into(),
                rule: "CD-P04".into(),
                location: format!("#/proto/service/{}", escape_segment(name)),
                message: format!("удалён сервис «{name}»"),
            }),
            Some(new_rpcs) => {
                let old_rpcs = &old_doc.services[name];
                for rpc in old_rpcs {
                    if !new_rpcs.contains(rpc) {
                        out.push(Finding {
                            severity: "error".into(),
                            rule: "CD-P04".into(),
                            location: format!(
                                "#/proto/service/{}/rpc/{}",
                                escape_segment(name),
                                escape_segment(rpc)
                            ),
                            message: format!("удалён rpc «{name}.{rpc}»"),
                        });
                    }
                }
                for rpc in new_rpcs {
                    if !old_rpcs.contains(rpc) {
                        out.push(Finding {
                            severity: "warn".into(),
                            rule: "CD-P05".into(),
                            location: format!(
                                "#/proto/service/{}/rpc/{}",
                                escape_segment(name),
                                escape_segment(rpc)
                            ),
                            message: format!("добавлен rpc «{name}.{rpc}»"),
                        });
                    }
                }
            }
        }
    }
    for name in new_doc.services.keys() {
        if !old_doc.services.contains_key(name) {
            out.push(Finding {
                severity: "warn".into(),
                rule: "CD-P05".into(),
                location: format!("#/proto/service/{}", escape_segment(name)),
                message: format!("добавлен сервис «{name}»"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::diff_proto;
    use crate::contract_diff::testkit::{PROTO_V1, diff_text};
    use crate::tool::ToolOutput;

    /// Прогон proto-диффа через инструмент.
    async fn diff_proto_text(old: &str, new: &str) -> ToolOutput {
        diff_text(old, new, "old.proto", "new.proto").await
    }

    #[tokio::test]
    async fn proto_identical_passes_clean() {
        let out = diff_proto_text(PROTO_V1, PROTO_V1).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content.contains("contract_diff: 0 изменений"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Формат: proto"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_removed_field_and_rpc_are_breaking() {
        let new = PROTO_V1
            .replace("  optional string currency = 3;\n", "")
            .replace(
                "  rpc Refund (ChargeRequest) returns (ChargeResponse);\n",
                "",
            );
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(
            out.content
                .contains("[error] #/proto/message/ChargeRequest/field/3 CD-P02"),
            "{}",
            out.content
        );
        assert!(
            out.content
                .contains("[error] #/proto/service/Charging/rpc/Refund CD-P04"),
            "{}",
            out.content
        );
        // Ломающий дифф, пакет остался v1 → CD-P06.
        assert!(
            out.content.contains("[error] #/proto/package CD-P06"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_reserved_removal_is_warn_not_breaking() {
        let new = PROTO_V1.replace(
            "  optional string currency = 3;\n",
            "  reserved 3;\n  reserved \"currency\";\n",
        );
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P05"), "{}", out.content);
        assert!(!out.content.contains("CD-P02"), "{}", out.content);
        assert!(!out.content.contains("CD-P06"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_type_change_and_renumbering_are_breaking_major_bump_clears_p06() {
        // Смена типа тега 2 (int64 → string) и перенумерация currency 3 → 5.
        let new = PROTO_V1
            .replace("int64 amount_minor = 2;", "string amount_minor = 2;")
            .replace(
                "optional string currency = 3;",
                "optional string currency = 5;",
            )
            .replace("package acme.payments.v1;", "package acme.payments.v2;");
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P03"), "{}", out.content);
        assert!(out.content.contains("перенумеровано"), "{}", out.content);
        // major поднят (v1 → v2) — CD-P06 молчит.
        assert!(!out.content.contains("CD-P06"), "{}", out.content);
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_added_field_and_rpc_are_warn() {
        let new = PROTO_V1
            .replace(
                "service Charging {",
                "service Charging {\n  rpc Status (ChargeRequest) returns (ChargeResponse);",
            )
            .replace(
                "message ChargeResponse {",
                "message ChargeResponse {\n  string receipt_id = 2;",
            );
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.contains("CD-P05"), "{}", out.content);
        assert!(out.content.contains("Итог: PASS"), "{}", out.content);
    }

    #[tokio::test]
    async fn proto_removed_message_is_breaking() {
        let new = PROTO_V1.replace("message ChargeResponse {\n  string status = 1;\n}\n\n", "");
        // rpc возвращают ChargeResponse — тип резолвится позже; для диффа
        // важно только удаление сообщения.
        let out = diff_proto_text(PROTO_V1, &new).await;
        assert!(
            out.content
                .contains("[error] #/proto/message/ChargeResponse CD-P01"),
            "{}",
            out.content
        );
        assert!(out.content.contains("Итог: FAIL"), "{}", out.content);
    }

    /// Добавленное поле сообщения названо (CD-P05), а не пропущено.
    #[test]
    fn added_field_is_reported() {
        let new = PROTO_V1.replace(
            "  string status = 1;\n",
            "  string status = 1;\n  string reason = 2;\n",
        );
        let findings = diff_proto(PROTO_V1, &new);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "CD-P05" && f.location.contains("ChargeResponse")),
            "{findings:?}"
        );
    }

    /// Новый номер поля с уже существующим именем — не «добавленное поле»
    /// (поле узнано по имени), а смена номера; ложного CD-P05 быть не должно.
    #[test]
    fn same_name_under_new_tag_is_not_an_added_field() {
        let new = PROTO_V1
            .replace("  optional string currency = 3;\n", "")
            .replace("  optional string currency = 3;\n", "");
        // Тот же набор полей, но currency переехала на номер 9.
        let renamed_tag = PROTO_V1.replace(
            "  optional string currency = 3;\n",
            "  optional string currency = 9;\n",
        );
        assert!(!diff_proto(PROTO_V1, &new).is_empty());
        let findings = diff_proto(PROTO_V1, &renamed_tag);
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "CD-P05" && f.message.contains("добавлено поле")),
            "переезд номера — не новое поле: {findings:?}"
        );
    }

    /// Удаление поля «прикрыто» либо номером в reserved, либо именем —
    /// каждого признака достаточно по отдельности.
    #[test]
    fn removal_is_covered_by_tag_or_by_name_independently() {
        let by_tag = PROTO_V1
            .replace("  optional string currency = 3;\n", "")
            .replace(
                "  Address billing = 4;",
                "  Address billing = 4;\n  reserved 3;",
            );
        let findings = diff_proto(PROTO_V1, &by_tag);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "CD-P05" && f.message.contains("currency")),
            "reserved по номеру прикрывает удаление: {findings:?}"
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "CD-P02" && f.message.contains("currency")),
            "ошибки быть не должно: {findings:?}"
        );

        let by_name = PROTO_V1
            .replace("  optional string currency = 3;\n", "")
            .replace(
                "  Address billing = 4;",
                "  Address billing = 4;\n  reserved \"currency\";",
            );
        let findings = diff_proto(PROTO_V1, &by_name);
        assert!(
            findings
                .iter()
                .any(|f| f.rule == "CD-P05" && f.message.contains("currency")),
            "reserved по имени прикрывает удаление: {findings:?}"
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "CD-P02" && f.message.contains("currency")),
            "ошибки быть не должно: {findings:?}"
        );
    }

    /// Диапазон reserved разворачивается с потолком: номер 10001 в диапазоне
    /// «1 to 100000» прикрыт (потолок считается от начала диапазона).
    #[test]
    fn reserved_range_expands_with_cap_from_range_start() {
        let old = PROTO_V1.replace(
            "  Address billing = 4;",
            "  Address billing = 4;\n  string far = 10001;",
        );
        let new = old.replace("  string far = 10001;\n", "").replace(
            "  Address billing = 4;",
            "  Address billing = 4;\n  reserved 1 to 100000;",
        );
        let findings = diff_proto(&old, &new);
        assert!(
            findings.iter().any(|f| f.message.contains("far")),
            "удаление далеко стоящего поля названо: {findings:?}"
        );
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "CD-P02" && f.message.contains("far")),
            "диапазон 1 to 100000 покрывает 10001: {findings:?}"
        );
    }

    /// `rpc` вне блока service не приписывается последнему сервису
    /// (консервативный скелет: непонятную строку пропускаем).
    #[test]
    fn rpc_outside_service_is_ignored() {
        let stray = format!("{PROTO_V1}\n  rpc Stray (ChargeRequest) returns (ChargeResponse);\n");
        let findings = diff_proto(PROTO_V1, &stray);
        assert!(
            findings.is_empty(),
            "строка вне service ничего не добавляет: {findings:?}"
        );
    }
}

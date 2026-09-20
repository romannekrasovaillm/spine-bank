=== ИСТОЧНИК subject: skeleton/payments/legacy_adapter.py ===
"""Приём реестров двух форматов: приведение к канонической строке и отправка.

Выгрузка старого контура приходит в формате 1 (`registry`, `line`, `sum_kopecks`),
новый реестр — в формате 2. Обе ветки приведения возвращают одну и ту же
каноническую `PayoutRow`, а проверка и отправка у форматов общие: ветка выбора
здесь — адаптер формата, а не второй путь к платформе. Ключ идемпотентности
собирается в единственном месте — в свойстве `PayoutRow.idempotency_key`.
"""

from dataclasses import dataclass

from skeleton.payments.platform_client import PlatformClient
from skeleton.payments.recipient_check import RecipientCheck
from skeleton.payments.registry import PayoutRow

# Поля выгрузки старого контура: вырезка остальных — часть адаптера формата.
_V1_FIELDS = ("registry", "line", "sum_kopecks", "recipient_ref", "purpose")


@dataclass(frozen=True)
class _LegacyV1:
    registry: str
    line: int
    sum_kopecks: int
    recipient_ref: str
    purpose: str = ""


def _canonical(raw: dict) -> PayoutRow:
    """Приводит строку любого формата к канонической (формат — деталь выгрузки).

    Обе ветки возвращают `PayoutRow` с одними и теми же полями, поэтому
    дальше о формате выгрузки никто не знает.
    """
    if raw.get("format", 1) == 1:
        v1 = _LegacyV1(**{name: raw[name] for name in _V1_FIELDS if name in raw})
        return PayoutRow(registry_id=v1.registry, row_no=v1.line,
                         amount_minor=v1.sum_kopecks, recipient_ref=v1.recipient_ref)
    return PayoutRow(registry_id=raw["registry_id"], row_no=raw["row_no"],
                     amount_minor=raw["amount_minor"],
                     recipient_ref=raw["recipient_ref"])


class LegacyAdapter:
    """Единая обработка строк обоих форматов: проверка, отправка, журнал."""

    def __init__(self, client: PlatformClient, checks: RecipientCheck, journal):
        self.client = client
        self.checks = checks
        self.journal = journal

    def accept(self, raw: dict) -> str:
        """Принимает строку в любом формате и ведёт её одним защищённым путём.

        Ни одна ветка адаптера не обращается к платформе сама: обе отдают
        строку в `_dispatch`, где проверка получателя выполняется до
        отправки, а ключ идемпотентности уходит в вызов (AD-1, AD-4).
        """
        return self._dispatch(_canonical(raw))

    def _dispatch(self, row: PayoutRow) -> str:
        """Единственный путь к платформе для обоих форматов."""
        refusal = self.checks.verify(row.recipient_ref, row.amount_minor)
        if refusal:
            self.journal.append(row.idempotency_key, "rejected", refusal)
            return "rejected"
        status = self.client.send(row.amount_minor, row.recipient_ref,
                                  idempotency_key=row.idempotency_key)
        self.journal.append(row.idempotency_key, status)
        return status
=== КОНЕЦ ИСТОЧНИКА ===
=== ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#AD-1 ===
AD-1: Выплата идемпотентна по ключу (реестр, строка)
Rule: ключ идемпотентности формируется из идентификатора реестра и номера строки и передаётся в каждый вызов платформы; повторная доставка реестра не создаёт второй выплаты
=== КОНЕЦ ИСТОЧНИКА ===
=== ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#AD-2 ===
AD-2: Журнал выплат append-only
Rule: строки журнала только дополняются; UPDATE и DELETE строк журнала запрещены, исправление — компенсирующей записью со ссылкой на исходную
=== КОНЕЦ ИСТОЧНИКА ===
=== ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#AD-3 ===
AD-3: Персональные данные получателей не попадают в журнал и телеметрию
Rule: в журнал и метки метрик пишутся только идентификаторы реестра, номер строки и сумма; ФИО и реквизиты получателя не пишутся нигде
=== КОНЕЦ ИСТОЧНИКА ===
=== ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#AD-4 ===
AD-4: Получатель и лимиты проверяются до отправки поручения
Rule: проверка получателя, стоп-листа и лимита окна выплат выполняется до обращения к платформе; непроверенная строка не отправляется
=== КОНЕЦ ИСТОЧНИКА ===
=== ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#AD-5 ===
AD-5: Отмена выплаты — отдельная операция, а не удаление записи
Rule: отмена оформляется операцией отмены со ссылкой на исходную запись; строка журнала и поручение не удаляются ни в каком режиме
=== КОНЕЦ ИСТОЧНИКА ===
=== ИСТОЧНИК reference: ARCHITECTURE-SPINE.md#DEF-1 ===
DEF-1: Автоматический разбор неопределённых исходов по расписанию
Статус: отложено (Deferred — решение принято не будет, пока не выполнено условие возврата)
Разбор строк в состоянии under_review запускается оператором вручную; возврат к вопросу — когда платформа отдаст чтение статуса поручения по ключу идемпотентности
=== КОНЕЦ ИСТОЧНИКА ===

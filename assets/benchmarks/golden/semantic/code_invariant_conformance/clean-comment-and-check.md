=== ИСТОЧНИК subject: skeleton/payments/intake.py ===
"""Приём реестра: проверка строки и подготовка журнальной записи.

Комментарии описывают то же, что делает код: перечень полей — исполняемая
константа, проверка получателя — вызов `checks.verify` до `client.send`.
"""

from dataclasses import dataclass

from skeleton.payments.journal import Journal
from skeleton.payments.platform_client import PlatformClient
from skeleton.payments.recipient_check import RecipientCheck

# Разрешённые поля журнальной записи и меток метрик (AD-3).
_ALLOWED_JOURNAL = frozenset({"key", "status", "amount_minor", "reason", "compensates"})
_ALLOWED_TAGS = frozenset({"registry", "row_no"})
# Поля получателя, которые нельзя писать никуда, даже если строка их принесла.
_FORBIDDEN_ANYWHERE = frozenset({"full_name", "iban", "account", "phone"})


@dataclass(frozen=True)
class IntakeRow:
    registry_id: str
    row_no: int
    amount_minor: int
    recipient_ref: str

    @property
    def idempotency_key(self) -> str:
        return f"{self.registry_id}:{self.row_no}"


def journal_payload(row: IntakeRow, status: str) -> dict:
    """Собирает журнальную запись по перечню; лишнее поле — ошибка (AD-3)."""
    for field in _FORBIDDEN_ANYWHERE:
        if hasattr(row, field):
            raise ValueError(f"строка несёт поле {field}: в журнал нельзя")
    return {"key": row.idempotency_key, "status": status,
            "amount_minor": row.amount_minor, "reason": "", "compensates": ""}


def metric_tags(row: IntakeRow) -> dict:
    """Метки метрик: только реестр и номер строки, без получателя (AD-3)."""
    tags = {"registry": row.registry_id, "row_no": str(row.row_no)}
    if not set(tags) <= _ALLOWED_TAGS:
        raise ValueError("метка вне перечня")
    return tags


class Intake:
    """Единственный вход: проверка (AD-4), затем отправка и журнал (AD-1)."""

    def __init__(self, client: PlatformClient, checks: RecipientCheck, journal: Journal):
        self.client = client
        self.checks = checks
        self.journal = journal

    def accept(self, row: IntakeRow) -> str:
        """Проверяет получателя до отправки; отказ фиксируется в журнале (AD-4)."""
        refusal = self.checks.verify(row.recipient_ref, row.amount_minor)
        if refusal:
            self.journal.record(**journal_payload(row, "rejected"))
            return "rejected"
        status = self.client.send(row.amount_minor, row.recipient_ref,
                                  idempotency_key=row.idempotency_key)
        self.journal.record(**journal_payload(row, status))
        self.client.metrics.increment("payout.intake", tags=metric_tags(row))
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

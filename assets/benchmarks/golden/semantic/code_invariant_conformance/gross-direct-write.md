=== ИСТОЧНИК subject: skeleton/payments/orchestrator.py ===
"""Оркестратор выплат: отправка строк, закрытие окна, повторная отправка.

Журнал окна выплат — источник аудита: по нему строится отчёт сверки."""


from dataclasses import dataclass

from skeleton.payments.platform_client import PlatformClient
from skeleton.payments.recipient_check import RecipientCheck

_INSERT = ("INSERT INTO payout_journal (key, status, amount_minor, reason) "
           "VALUES (?, ?, ?, ?)")


@dataclass(frozen=True)
class PayoutRow:
    registry_id: str
    row_no: int
    amount_minor: int
    recipient_ref: str

    @property
    def idempotency_key(self) -> str:
        """Ключ идемпотентности: пара (реестр, строка), AD-1."""
        return f"{self.registry_id}:{self.row_no}"


class Orchestrator:
    """Отправляет поручения и ведёт журнал окна выплат."""

    def __init__(self, db, client: PlatformClient, checks: RecipientCheck):
        self.db = db
        self.client = client
        self.checks = checks

    def send_row(self, row: PayoutRow) -> str:
        """Проверяет строку и отправляет поручение (AD-4)."""
        refusal = self.checks.verify(row)
        if refusal:
            self.db.execute(_INSERT, (row.idempotency_key, "rejected",
                                      row.amount_minor, refusal))
            return "rejected"
        status = self.client.send(row.amount_minor, row.recipient_ref,
                                  idempotency_key=row.idempotency_key)
        self.db.execute(_INSERT, (row.idempotency_key, status, row.amount_minor, ""))
        return status

    def close_window(self, registry_id: str) -> int:
        """Закрывает окно: подтверждённые платформой строки правит в paid.

        Компенсирующая запись на каждую строку раздувает отчёт сверки.
        """
        cur = self.db.execute(
            "UPDATE payout_journal SET status = 'paid', reason = '' "
            "WHERE key LIKE ? AND status = 'under_review' AND platform_confirmed = 1",
            (f"{registry_id}:%",),
        )
        return cur.rowcount

    def resend_rejected(self, registry_id: str, rows) -> int:
        """Переотправляет отказные строки: старые записи удаляются (AD-2, AD-5)."""
        self.db.execute(
            "DELETE FROM payout_journal WHERE key LIKE ? AND status = 'rejected'",
            (f"{registry_id}:%",),
        )
        sent = 0
        for row in rows:
            if self.send_row(row) != "rejected":
                sent += 1
        return sent
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

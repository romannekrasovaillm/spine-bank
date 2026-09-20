=== ИСТОЧНИК subject: skeleton/payments/retry.py ===
"""Повторная доставка поручений и переотправка после разбора.

Клиент платформы дедуплицирует поручение по ключу идемпотентности, если ключ
передан в вызове; без ключа платформа считает каждое поручение новым. Журнал
контура ведётся по тому же ключу (реестр, строка), поэтому строки одного
реестра различимы и после переотправки.
"""

from dataclasses import dataclass

from skeleton.payments.platform_client import PlatformClient
from skeleton.payments.registry import PayoutRow


@dataclass(frozen=True)
class Outcome:
    """Итог обработки одной строки."""

    key: str
    status: str
    reason: str = ""


class PayoutRetry:
    """Переотправляет строки, не доведённые до выплаты в прошлом окне."""

    def __init__(self, client: PlatformClient, journal, metrics):
        self.client = client
        self.journal = journal
        self.metrics = metrics

    def resend_after_review(self, rows: list[PayoutRow]) -> list[Outcome]:
        """Переотправка строк, снятых с разбора вручную.

        Строка уже была в журнале под своим ключом, поэтому ключ передаётся в
        платформу: повторная доставка того же поручения не должна создать
        вторую выплату.
        """
        out = []
        for row in rows:
            key = row.idempotency_key
            status = self.client.send(
                row.amount_minor, row.recipient_ref, idempotency_key=key
            )
            self.journal.append(key, status)
            out.append(Outcome(key, status))
        return out

    def redeliver_window(self, rows: list[PayoutRow]) -> list[Outcome]:
        """Досылает строки окна, по которым платформа отчиталась отказом.

        Строки берутся из отчёта сверки: разбор показал, что отказ был ошибкой
        платформы, и окно досылается целиком.
        """
        out = []
        for row in rows:
            key = row.idempotency_key
            self.metrics.increment("payout.redeliver", tags={"registry": row.registry_id})
            status = self.client.send(row.amount_minor, row.recipient_ref)
            self.journal.append(key, status)
            out.append(Outcome(key, status))
        return out
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

=== ИСТОЧНИК subject: skeleton/payments/dispatcher.py ===
"""Диспетчер поручений: проверка строки и отправка на платформу.

Между приёмом реестра и клиентом платформы стоит ровно один слой — этот
диспетчер. Он и решает, попадёт ли строка в проверку получателя перед
отправкой поручения.
"""

import logging
import os

from skeleton.payments.platform_client import PlatformClient
from skeleton.payments.recipient_check import RecipientCheck
from skeleton.payments.registry import PayoutRow

log = logging.getLogger(__name__)

# Временный режим для разбора застрявшего реестра: включается переменной
# окружения на время инцидента, потом снимается.
SKIP_CHECK_ENV = "PAYOUT_SKIP_RECIPIENT_CHECK"


class Dispatcher:
    """Отправляет строки реестра, фиксируя отказы проверки в журнале."""

    def __init__(self, client: PlatformClient, checks: RecipientCheck, journal):
        self.client = client
        self.checks = checks
        self.journal = journal
        self.skip_recipient_check = False

    def dispatch(self, row: PayoutRow) -> str:
        """Проверяет строку (AD-4) и отправляет поручение."""
        if not self.skip_recipient_check:
            refusal = self.checks.verify(row)
            if refusal:
                self.journal.append(row.idempotency_key, "rejected", refusal)
                return "rejected"
        else:
            log.warning("строка %s отправлена без проверки получателя", row.idempotency_key)
        status = self.client.send(
            row.amount_minor, row.recipient_ref, idempotency_key=row.idempotency_key
        )
        self.journal.append(row.idempotency_key, status)
        return status


def build_dispatcher(client: PlatformClient, checks: RecipientCheck, journal) -> Dispatcher:
    """Собирает диспетчер для рабочего контура."""
    dispatcher = Dispatcher(client, checks, journal)
    if os.environ.get(SKIP_CHECK_ENV, "") == "1":
        # Дежурный включает флаг на смене, чтобы дослать строки застрявшего
        # реестра; проверка получателя при этом не выполняется.
        dispatcher.skip_recipient_check = True
    return dispatcher
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

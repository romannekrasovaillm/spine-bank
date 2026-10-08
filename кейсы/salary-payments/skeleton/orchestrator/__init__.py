"""Оркестратор выплат (CMP-001): приём реестра и построчная обработка.

Реестр не имеет состояния успеха: состояние есть у строки, поэтому падение
на середине не отменяет уже сделанные выплаты (AD-6). Повторная доставка
реестра второй выплаты не создаёт (AD-1).
"""

from skeleton.journal import Journal
from skeleton.platform import Platform
from skeleton.recipients import StopList
from skeleton.registry import Row


def process_registry(
    rows: list[Row], platform: Platform, journal: Journal, stop_list: StopList
):
    """Обрабатывает реестр построчно: частичный успех — штатный режим, AD-6.

    Возвращает (выплачено, отказов, в разборе).
    """
    paid_minor = 0
    paid = rejected = under_review = 0
    for row in rows:
        if journal.applied_row(row):
            # Повторная доставка реестра: выплата уже сделана, второй нет.
            paid += 1
            continue
        reason = stop_list.check_limits(row, paid_minor)
        if reason:
            journal.append_row(row, "rejected", reason)
            rejected += 1
            continue
        status = platform.send(row)
        if status == "paid":
            journal.append_row(row, "paid")
            paid_minor += row.amount_minor
            paid += 1
        elif status == "unknown":
            # Неопределённый исход: повтор запрещён до сверки (AD-3, AD-6).
            journal.append_row(row, "under_review", "нет ответа платформы")
            under_review += 1
        else:
            journal.append_row(row, "rejected", "отказ платформы")
            rejected += 1
    return paid, rejected, under_review


def partial_success(paid: int, rejected: int, under_review: int) -> bool:
    """Реестр обработан частично — штатный режим, а не авария (AD-6).

    Отдельная функция, потому что «частично» здесь не ошибка, а состояние:
    вызывающий код обязан решать, что с ним делать, а не ловить исключение.
    """
    return paid > 0 and (rejected > 0 or under_review > 0)

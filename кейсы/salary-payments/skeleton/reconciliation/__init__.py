"""Сверка с платформой (CMP-005): разбор по данным платформы, AD-7 и AD-3.

Строки в разборе разрешаются данными платформы, а не повторной отправкой.
Итог сверки — список расхождений, а не молчаливая правка журнала.
"""

from skeleton.journal import Journal
from skeleton.platform import Platform
from skeleton.registry import Row


def reconcile(rows: list[Row], platform: Platform, journal: Journal):
    """Отчёт сверки по журналу и данным платформы (AD-7, AD-3).

    Возвращает список расхождений: [(ключ, что не сходится)].
    """
    discrepancies = []
    for row in rows:
        key = row.idempotency_key
        platform_status = platform.status(key)
        journal_status = next(
            (e["status"] for e in reversed(journal.entries) if e["key"] == key), None
        )
        if journal_status == "under_review":
            # Разбор: платформа либо подтверждает выплату, либо нет.
            if platform_status == "paid" and not journal.applied(key):
                journal.append_only(key, "paid", "разрешено сверкой")
            elif platform_status == "unknown":
                journal.append_only(key, "rejected", "платформа не подтвердила выплату")
        elif platform_status == "paid" and journal_status != "paid":
            discrepancies.append((key, "платформа выплатила, журнал не знает"))
        elif platform_status != "paid" and journal_status == "paid":
            discrepancies.append((key, "журнал знает о выплате, платформа нет"))
    return discrepancies

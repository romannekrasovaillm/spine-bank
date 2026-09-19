"""Walking skeleton контура массовых выплат в цифровых рублях.

Минимальный сквозной путь: принять реестр, проверить строки, отправить
поручения, зафиксировать статусы построчно, разобрать неопределённые исходы
и построить отчёт сверки. Модуль намеренно не имеет ни базы, ни очереди: он
проверяет, что контур сходится по смыслу, а не что он быстрый.
"""

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Row:
    """Строка реестра выплат."""

    registry_id: str
    row_no: int
    amount_minor: int
    destination_ref: str

    @property
    def idempotency_key(self) -> str:
        """Ключ идемпотентности: пара (реестр, строка), AD-1."""
        return f"{self.registry_id}:{self.row_no}"


@dataclass
class Outcome:
    """Результат обработки одной строки."""

    key: str
    status: str  # paid | rejected | under_review
    reason: str = ""


@dataclass
class Journal:
    """Журнал выплат: только дополняется, AD-2 и AD-4."""

    entries: list = field(default_factory=list)

    def append_only(self, key: str, status: str, reason: str = "") -> None:
        # В журнал пишутся только идентификаторы, статус и сумма: персональные
        # данные получателя остаются за границей контура.
        self.entries.append({"key": key, "status": status, "reason": reason})

    def applied(self, key: str) -> bool:
        return any(e["key"] == key and e["status"] == "paid" for e in self.entries)


class StopList:
    """Стоп-лист получателей и лимит окна (AD-5)."""

    def __init__(self, blocked: set, window_limit_minor: int):
        self.blocked = blocked
        self.window_limit_minor = window_limit_minor

    def check_limits(self, row: Row, paid_minor: int) -> str:
        """Возвращает причину отказа либо пустую строку."""
        if row.destination_ref in self.blocked:
            return "получатель в стоп-листе"
        if paid_minor + row.amount_minor > self.window_limit_minor:
            return "превышен лимит окна выплат"
        return ""


class Platform:
    """Платформа цифрового рубля: поручения и чтение статуса."""

    def __init__(self):
        self.sent = {}
        self.unavailable = False

    def send(self, row: Row) -> str:
        if self.unavailable:
            # Таймаут — НЕ отказ: исход не определён (AD-3).
            return "unknown"
        if row.idempotency_key in self.sent:
            return self.sent[row.idempotency_key]
        status = "rejected" if row.amount_minor <= 0 else "paid"
        self.sent[row.idempotency_key] = status
        return status

    def status(self, key: str) -> str:
        return self.sent.get(key, "unknown")


def process_registry(rows, platform: Platform, journal: Journal, stop_list: StopList):
    """Обрабатывает реестр построчно: частичный успех — штатный режим, AD-6.

    Возвращает (выплачено, отказов, в разборе). Реестр не имеет состояния
    успеха: состояние есть у строки, поэтому падение на середине не отменяет
    уже сделанные выплаты.
    """
    paid_minor = 0
    paid = rejected = under_review = 0
    for row in rows:
        if journal.applied(row.idempotency_key):
            # Повторная доставка реестра: выплата уже сделана, второго нет.
            paid += 1
            continue
        reason = stop_list.check_limits(row, paid_minor)
        if reason:
            journal.append_only(row.idempotency_key, "rejected", reason)
            rejected += 1
            continue
        status = platform.send(row)
        if status == "paid":
            journal.append_only(row.idempotency_key, "paid")
            paid_minor += row.amount_minor
            paid += 1
        elif status == "unknown":
            # Неопределённый исход: повтор запрещён до сверки (AD-3, AD-6).
            journal.append_only(row.idempotency_key, "under_review", "нет ответа платформы")
            under_review += 1
        else:
            journal.append_only(row.idempotency_key, "rejected", "отказ платформы")
            rejected += 1
    return paid, rejected, under_review


def partial_success(paid: int, rejected: int, under_review: int) -> bool:
    """Реестр обработан частично — штатный режим, а не авария (AD-6).

    Отдельная функция, потому что «частично» здесь не ошибка, а состояние:
    вызывающий код обязан решать, что с ним делать, а не ловить исключение.
    """
    return paid > 0 and (rejected > 0 or under_review > 0)


def reconcile(rows, platform: Platform, journal: Journal):
    """Отчёт сверки по журналу и данным платформы (AD-7, AD-3).

    Строки в разборе разрешаются данными платформы, а не повторной отправкой.
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

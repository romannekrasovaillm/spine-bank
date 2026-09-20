=== ИСТОЧНИК subject: skeleton/payments/journal.py ===
"""Журнал выплат: append-only по конструкции, поля записи закрыты перечнем.

У хранилища нет метода правки или удаления строки — исправление оформляется
компенсирующей записью, а запись принимает закрытый набор полей, поэтому
персональные данные получателя в журнал не попадают.
"""

from dataclasses import dataclass
from typing import Callable

_STATUSES = frozenset({"paid", "rejected", "under_review", "cancelled"})
_JOURNAL_FIELDS = frozenset({"key", "status", "amount_minor", "reason", "compensates"})


@dataclass(frozen=True)
class JournalEntry:
    key: str
    status: str
    amount_minor: int
    reason: str = ""
    compensates: str = ""

    def __post_init__(self) -> None:
        if self.status not in _STATUSES:
            raise ValueError(f"неизвестный статус журнала: {self.status}")

    def payload(self) -> dict:
        return {name: getattr(self, name) for name in sorted(_JOURNAL_FIELDS)}


class Journal:
    """Хранилище журнала: наружу торчит только дописывание (AD-2)."""

    _INSERT = ("INSERT INTO payout_journal "
               "(key, status, amount_minor, reason, compensates) "
               "VALUES (:key, :status, :amount_minor, :reason, :compensates)")

    def __init__(self, db):
        self._db = db

    def append(self, entry: JournalEntry) -> None:
        self._db.execute(self._INSERT, entry.payload())

    def record(self, **raw) -> None:
        """Пишет запись из сырых полей: в журнал нельзя ни ФИО, ни реквизиты (AD-3)."""
        forbidden = set(raw) - _JOURNAL_FIELDS
        if forbidden:
            raise ValueError(f"в журнал нельзя писать поля: {sorted(forbidden)}")
        self.append(JournalEntry(**raw))

    def append_correction(self, original: JournalEntry, status: str, reason: str) -> None:
        """Исправление — компенсирующая запись со ссылкой на исходную (AD-2)."""
        self.append(JournalEntry(key=f"{original.key}#correction", status=status,
                                 amount_minor=original.amount_minor, reason=reason,
                                 compensates=original.key))

    def paid(self, key: str) -> bool:
        """Есть ли по ключу состоявшаяся выплата (AD-1)."""
        sql = "SELECT 1 FROM payout_journal WHERE key = ? AND status = 'paid' LIMIT 1"
        return self._db.execute(sql, (key,)).fetchone() is not None

    def apply_once(self, key: str, amount_minor: int, effect: Callable[[], str]) -> str:
        """Дедупликация в одной транзакции с эффектом: повторная доставка видит
        зафиксированную выплату и второй раз эффект не выполняет (AD-1)."""
        with self._db.transaction():
            if self.paid(key):
                return "paid"
            status = effect()
            self.append(JournalEntry(key=key, status=status, amount_minor=amount_minor))
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

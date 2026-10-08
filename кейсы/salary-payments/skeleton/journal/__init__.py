"""Журнал выплат (CMP-003): только дополняется, AD-2 и AD-4.

В журнал пишутся только идентификаторы, статус и сумма: персональные данные
получателя остаются за границей контура. Исправление — компенсирующей записью
со ссылкой на исходную, а не правкой.
"""

from dataclasses import dataclass, field

from skeleton.registry import Row


@dataclass
class Journal:
    """Журнал выплат: append-only, чтение по ключу идемпотентности."""

    entries: list = field(default_factory=list)

    def append_only(self, key: str, status: str, reason: str = "") -> None:
        # В журнал пишутся только идентификаторы, статус и причина:
        # персональные данные получателя остаются за границей контура.
        self.entries.append({"key": key, "status": status, "reason": reason})

    def append_row(self, row: Row, status: str, reason: str = "") -> None:
        """Запись по строке реестра: ключ вычисляется из строки (AD-1, AD-2)."""
        self.append_only(row.idempotency_key, status, reason)

    def applied(self, key: str) -> bool:
        return any(e["key"] == key and e["status"] == "paid" for e in self.entries)

    def applied_row(self, row: Row) -> bool:
        """Была ли строка реестра уже выплачена (дедупликация повторной доставки)."""
        return self.applied(row.idempotency_key)

"""Фейк журнала: только append и read — update и delete намеренно отсутствуют.

Фейк считает то, что проверяет инвариант: сколько записей добавлено
(`appends`) и что сейчас лежит в журнале (`read`). Ни сети, ни сна —
детерминированный прогон за миллисекунды.
"""


class FakeJournal:
    """Журнал операций: записи только добавляются, id выдаётся по порядку."""

    def __init__(self) -> None:
        self.appends = 0
        self._entries: list[dict] = []

    def append(self, kind: str, amount: int, reverses=None) -> dict:
        """Добавить запись; id растёт по порядку добавления."""
        self.appends += 1
        entry = {
            "id": len(self._entries) + 1,
            "kind": kind,
            "amount": amount,
            "reverses": reverses,
        }
        self._entries.append(entry)
        return entry

    def read(self) -> list[dict]:
        """Записи журнала в порядке добавления.

        Фейк отдаёт внутренний список как есть — это осознанное упрощение:
        неизменяемость журнала обеспечивает вызывающий, и именно её проверяет
        инвариант.
        """
        return self._entries

    def count(self) -> int:
        """Сколько записей в журнале."""
        return len(self._entries)

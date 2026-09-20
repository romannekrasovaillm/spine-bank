"""Нарушающая реализация: одно засеянное нарушение — исправление правит запись на месте.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
`correct` переписывает существующую запись вместо того, чтобы добавить
компенсирующую, — журнал перестаёт быть append-only, и история операции
теряется.
"""


class Ledger:
    """Журнал операций, переписывающий записи (нарушение инварианта)."""

    def __init__(self, journal) -> None:
        self._journal = journal

    def post(self, amount: int) -> dict:
        """Добавить запись об операции."""
        return self._journal.append("post", amount)

    def correct(self, entry_id: int, amount: int) -> dict:
        """Исправить запись, переписав её на месте."""
        entry = self._find(entry_id)
        # Единственное засеянное нарушение: исправление меняет существующую
        # запись вместо компенсирующей — исходное значение теряется.
        entry["amount"] = amount
        return entry

    def entries(self) -> list[dict]:
        """Записи журнала в порядке добавления."""
        return self._journal.read()

    def _find(self, entry_id: int) -> dict:
        """Найти запись по id; отсутствие записи — ошибка."""
        for entry in self._journal.read():
            if entry["id"] == entry_id:
                return entry
        raise KeyError(f"записи {entry_id} в журнале нет")

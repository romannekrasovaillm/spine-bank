"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""


class Ledger:
    """Журнал операций: записи только добавляются.

    Свойство: запись неизменяема и неудаляема, исправление оформляется
    компенсирующей записью со ссылкой на исходную, порядок записей
    сохраняется.
    """

    def __init__(self, journal) -> None:
        self._journal = journal

    def post(self, amount: int) -> dict:
        """Добавить запись об операции."""
        return self._journal.append("post", amount)

    def correct(self, entry_id: int, amount: int) -> dict:
        """Исправить запись компенсирующей записью; исходная остаётся на месте."""
        self._find(entry_id)
        return self._journal.append("correction", amount, reverses=entry_id)

    def entries(self) -> list[dict]:
        """Записи журнала в порядке добавления."""
        return self._journal.read()

    def _find(self, entry_id: int) -> dict:
        """Найти запись по id; отсутствие записи — ошибка."""
        for entry in self._journal.read():
            if entry["id"] == entry_id:
                return entry
        raise KeyError(f"записи {entry_id} в журнале нет")

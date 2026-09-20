"""Фейк источника истины: история значений, версия и управляемая задержка ответа.

Фейк считает то, что проверяет инвариант: сколько раз источник читали
(`pulls`) и какая у него сейчас версия (`current_version`). Задержка сдвигает
выдаваемое значение назад по истории — так появляется устаревший ответ,
который нельзя выдавать за актуальный. Ни сети, ни сна — «время» двигает
сам тест вызовом `delay_answer`.
"""


class FakeSource:
    """Источник истины: история значений, версия и задержка ответа."""

    def __init__(self) -> None:
        self.pulls = 0
        self._history: list[int] = []
        self._delay_ticks = 0  # на сколько версий назад отвечать

    def write(self, value: int) -> dict:
        """Записать значение; версия источника растёт на единицу."""
        self._history.append(value)
        return {"value": value, "version": len(self._history)}

    def delay_answer(self, ticks: int) -> None:
        """Имитировать задержку: отвечать на `ticks` версий назад."""
        self._delay_ticks = max(0, ticks)

    def pull(self) -> dict:
        """Прочитать источник. При задержке ответ приходит устаревшим."""
        self.pulls += 1
        if not self._history:
            return {"value": 0, "version": 0, "stale": False}
        version = max(1, len(self._history) - self._delay_ticks)
        return {
            "value": self._history[version - 1],
            "version": version,
            "stale": version < len(self._history),
        }

    def current_version(self) -> int:
        """Версия источника истины прямо сейчас."""
        return len(self._history)

    def value_at(self, version: int) -> int:
        """Значение, записанное в эту версию, — канон для сверки с проекцией."""
        return self._history[version - 1]

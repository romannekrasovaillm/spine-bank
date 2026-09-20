"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""


class Projection:
    """Локальная проекция источника истины.

    Свойство: проекция не «додумывает» состояние. Значение меняется только
    вместе с подтверждённым ответом источника, показанное значение равно
    последнему ответу, а устаревший ответ помечается как устаревший.
    """

    def __init__(self, source) -> None:
        self._source = source
        self._value = 0
        self._version = 0
        self._stale = False

    def read(self) -> dict:
        """Показать значение вместе с его версией и признаком устаревания."""
        return {"value": self._value, "version": self._version, "stale": self._stale}

    def refresh(self) -> dict:
        """Принять ответ источника как есть: со своей версией и признаком свежести."""
        answer = self._source.pull()
        self._value = answer["value"]
        self._version = answer["version"]
        self._stale = answer["stale"]
        return self.read()

    def submit(self, value: int) -> dict:
        """Отправить значение в источник; проекция показывает подтверждённое значение."""
        ack = self._source.write(value)
        self._value = ack["value"]
        self._version = ack["version"]
        self._stale = False
        return self.read()

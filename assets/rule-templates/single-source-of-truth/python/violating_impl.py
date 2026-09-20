"""Нарушающая реализация: одно засеянное нарушение — устаревший ответ выдан за актуальный.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
`refresh` не переносит в проекцию признак устаревания, поэтому задержавшийся
ответ источника молча выдаётся за текущее состояние.
"""


class Projection:
    """Локальная проекция, теряющая признак устаревания (нарушение инварианта)."""

    def __init__(self, source) -> None:
        self._source = source
        self._value = 0
        self._version = 0
        self._stale = False

    def read(self) -> dict:
        """Показать значение вместе с его версией и признаком устаревания."""
        return {"value": self._value, "version": self._version, "stale": self._stale}

    def refresh(self) -> dict:
        """Принять любой ответ источника как текущий."""
        answer = self._source.pull()
        self._value = answer["value"]
        self._version = answer["version"]
        # Единственное засеянное нарушение: признак устаревания не переносится —
        # устаревший ответ выдаётся за актуальный.
        return self.read()

    def submit(self, value: int) -> dict:
        """Отправить значение в источник; проекция показывает подтверждённое значение."""
        ack = self._source.write(value)
        self._value = ack["value"]
        self._version = ack["version"]
        self._stale = False
        return self.read()

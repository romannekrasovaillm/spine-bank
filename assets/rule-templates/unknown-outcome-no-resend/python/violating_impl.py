"""Нарушающая реализация: одно засеянное нарушение — повторная отправка.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
`submit` не помнит, что поручение уже ушло, и при неопределённом исходе
отправляет во внешнюю систему ещё раз (второй эффект возможен).
"""

UNKNOWN = "UNKNOWN"
SUCCESS = "SUCCESS"
FAILED = "FAILED"


class PaymentService:
    """Отправка поручения БЕЗ запрета повторной отправки (нарушение инварианта)."""

    def __init__(self, platform) -> None:
        self._platform = platform
        self._states: dict[str, str] = {}

    def submit(self, key: str, amount: int) -> str:
        """Отправить поручение; при неопределённом исходе уходит ещё одна отправка."""
        try:
            self._platform.send(key, amount)
        except TimeoutError:
            self._states[key] = UNKNOWN
        except ValueError:
            self._states[key] = FAILED
        else:
            self._states[key] = SUCCESS
        return self._states[key]

    def query_status(self, key: str) -> str:
        """Запросить статус. Разрешён после UNKNOWN; терминальный статус закрывает операцию."""
        state = self._states.get(key)
        if state != UNKNOWN:
            return state
        answer = self._platform.query_status(key)
        self._states[key] = SUCCESS if answer["status"] == "done" else FAILED
        return self._states[key]

    def state_of(self, key: str):
        """Текущее состояние операции (None — операции не было)."""
        return self._states.get(key)

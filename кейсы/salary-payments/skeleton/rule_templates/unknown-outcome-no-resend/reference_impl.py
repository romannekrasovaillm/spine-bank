"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""

UNKNOWN = "UNKNOWN"
SUCCESS = "SUCCESS"
FAILED = "FAILED"


class PaymentService:
    """Отправка поручения с запретом повторной отправки при неопределённом исходе.

    Свойство: таймаут внешней системы оставляет операцию в состоянии UNKNOWN,
    во внешнюю систему уходит ровно одна отправка, а разрешён только запрос
    статуса — он и закрывает операцию терминально.
    """

    def __init__(self, platform) -> None:
        self._platform = platform
        self._states: dict[str, str] = {}

    def submit(self, key: str, amount: int) -> str:
        """Отправить поручение. Повторный вызов во внешнюю систему не уходит."""
        if key in self._states:
            return self._states[key]
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

"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""


class PaymentService:
    """Приём поручений с ключом идемпотентности.

    Свойство: одна операция на ключ. Повторная доставка возвращает
    сохранённый ответ и не создаёт второй эффект.
    """

    def __init__(self, platform) -> None:
        self._platform = platform
        self._answers: dict[str, dict] = {}

    def process(self, key: str, amount: int) -> dict:
        """Обработать поручение; повторная доставка с тем же ключом — тот же ответ."""
        if key in self._answers:
            return self._answers[key]
        answer = self._platform.send(key, amount)
        self._answers[key] = answer
        return answer

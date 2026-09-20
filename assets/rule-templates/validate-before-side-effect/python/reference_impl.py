"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""


class PaymentService:
    """Перевод получателю: сначала проверки, потом обращение к внешней системе.

    Свойства: при провале проверки (получатель в стоп-листе, сумма сверх
    лимита) обращений к внешней системе ноль; при успешной проверке —
    ровно одно обращение; лимит расходуется только после исполнения.
    """

    def __init__(self, stop_list, limit, external) -> None:
        self._stop_list = stop_list
        self._limit = limit
        self._external = external

    def transfer(self, recipient: str, amount: int) -> dict:
        """Проверить получателя и лимит, затем ровно один раз обратиться к системе."""
        if self._stop_list.contains(recipient):
            raise ValueError("получатель в стоп-листе")
        if not self._limit.allows(amount):
            raise ValueError("перевод превышает доступный лимит")
        answer = self._external.transfer(recipient, amount)
        self._limit.spend(amount)
        return answer

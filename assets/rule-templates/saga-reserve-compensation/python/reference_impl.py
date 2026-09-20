"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""

UNKNOWN = "UNKNOWN"
SUCCESS = "SUCCESS"
FAILED = "FAILED"


class PaymentSaga:
    """Резерв → отправка → списание с компенсацией.

    Свойство: отказ внешней системы снимает резерв и не даёт списания,
    неопределённый исход резерв удерживает (ни снятия, ни списания),
    успех превращает резерв в списание ровно один раз.
    """

    def __init__(self, platform, reserve) -> None:
        self._platform = platform
        self._reserve = reserve

    def pay(self, key: str, amount: int) -> str:
        """Провести платёж через сагу: резерв, отправка, списание либо снятие."""
        self._reserve.hold(key, amount)
        try:
            self._platform.send(key, amount)
        except TimeoutError:
            # исход неизвестен: резерв удерживаем, списания нет — решает сверка
            return UNKNOWN
        except ValueError:
            self._reserve.release(key, amount)
            return FAILED
        self._reserve.capture(key, amount)
        return SUCCESS

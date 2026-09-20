"""Нарушающая реализация: одно засеянное нарушение — резерв не снимается.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
при отказе внешней системы компенсация не выполняется — резерв остаётся
удержанным, лимит клиента утекает.
"""

UNKNOWN = "UNKNOWN"
SUCCESS = "SUCCESS"
FAILED = "FAILED"


class PaymentSaga:
    """Резерв → отправка → списание БЕЗ компенсации отказа (нарушение инварианта)."""

    def __init__(self, platform, reserve) -> None:
        self._platform = platform
        self._reserve = reserve

    def pay(self, key: str, amount: int) -> str:
        """Провести платёж; при отказе резерв остаётся удержанным."""
        self._reserve.hold(key, amount)
        try:
            self._platform.send(key, amount)
        except TimeoutError:
            return UNKNOWN
        except ValueError:
            return FAILED
        self._reserve.capture(key, amount)
        return SUCCESS

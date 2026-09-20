"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""


class AutoActionService:
    """Автодействия по событию клиента — только при действующем согласии.

    Свойства: без записи о согласии автодействие не создаётся; отзыв
    согласия останавливает следующее срабатывание; при незавершённой
    операции клиента новое автодействие не стартует.
    """

    def __init__(self, consents, actions, operations) -> None:
        self._consents = consents
        self._actions = actions
        self._operations = operations

    def trigger(self, client_id: str, scope: str, kind: str, amount: int):
        """Создать автодействие, если это разрешено; иначе вернуть None."""
        if not self._consents.is_active(client_id, scope):
            return None
        if self._operations.has_open(client_id):
            return None
        return self._actions.create(client_id, kind, amount)

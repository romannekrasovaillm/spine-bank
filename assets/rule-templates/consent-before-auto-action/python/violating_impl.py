"""Нарушающая реализация: одно засеянное нарушение — согласие не проверяется.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
автодействие создаётся без записи о согласии (и после его отзыва тоже).
"""


class AutoActionService:
    """Автодействия по событию клиента БЕЗ проверки согласия (нарушение)."""

    def __init__(self, consents, actions, operations) -> None:
        self._consents = consents
        self._actions = actions
        self._operations = operations

    def trigger(self, client_id: str, scope: str, kind: str, amount: int):
        """Создать автодействие, не заглядывая в реестр согласий."""
        if self._operations.has_open(client_id):
            return None
        return self._actions.create(client_id, kind, amount)

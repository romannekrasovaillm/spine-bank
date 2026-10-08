"""Фейки внешней системы, стоп-листа и лимита.

Фейки считают то, что проверяет инвариант: сколько раз обратились
к внешней системе (`calls`) и какие переводы она применила (`moves`),
кого держит стоп-лист (`contains`) и сколько клиенту ещё можно перевести
(`available`). Ни сети, ни сна — детерминированный прогон за миллисекунды.
"""


class FakeExternalSystem:
    """Внешняя система: считает обращения и применённые переводы."""

    def __init__(self) -> None:
        self.calls = 0
        self.moves: list[tuple[str, int]] = []

    def transfer(self, recipient: str, amount: int) -> dict:
        """Принять перевод к исполнению."""
        self.calls += 1
        self.moves.append((recipient, amount))
        return {"recipient": recipient, "amount": amount, "status": "done"}


class FakeStopList:
    """Стоп-лист получателей."""

    def __init__(self, blocked=()) -> None:
        self._blocked = set(blocked)

    def contains(self, recipient: str) -> bool:
        """Есть ли получатель в стоп-листе."""
        return recipient in self._blocked


class FakeLimit:
    """Доступный лимит клиента: сколько ещё можно перевести."""

    def __init__(self, available: int) -> None:
        self.available = available

    def allows(self, amount: int) -> bool:
        """Укладывается ли сумма в доступный лимит."""
        return amount <= self.available

    def spend(self, amount: int) -> None:
        """Израсходовать лимит после успешного перевода."""
        self.available -= amount

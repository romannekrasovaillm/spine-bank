"""Фейк внешней системы с управляемым поведением и счётчиками вызовов.

Фейк считает то, что проверяет инвариант: сколько раз к системе обратились
(`calls`) и какие эффекты она применила (`effects`). Ни сети, ни сна —
детерминированный прогон за миллисекунды.
"""


class FakePlatform:
    """Внешняя система: считает обращения и применённые эффекты."""

    def __init__(self) -> None:
        self.calls = 0
        self.effects: list[tuple[str, int]] = []
        self.behaviour = "ok"  # ok | timeout | reject

    def send(self, key: str, amount: int) -> dict:
        """Принять поручение. Повторный вызов с тем же ключом — второй эффект."""
        self.calls += 1
        if self.behaviour == "timeout":
            raise TimeoutError("внешняя система не ответила")
        if self.behaviour == "reject":
            raise ValueError("внешняя система отклонила поручение")
        self.effects.append((key, amount))
        return {"key": key, "amount": amount, "status": "done"}

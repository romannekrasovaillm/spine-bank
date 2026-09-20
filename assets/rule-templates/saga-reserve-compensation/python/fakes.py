"""Фейки саги: внешняя система и лимитный резерв со счётчиками.

Фейки считают то, что проверяет инвариант: сколько отправок ушло во внешнюю
систему, сколько раз резерв удержали, сняли и превратили в списание. Снятие
резерва и списание — разные операции: инвариант именно в том, что при отказе
резерв снимается, при неопределённом исходе удерживается, а при успехе
превращается в списание ровно один раз. Ни сети, ни сна.
"""


class FakeExternalSystem:
    """Внешняя система: считает отправки и применённые эффекты."""

    def __init__(self) -> None:
        self.sends = 0
        self.effects: list[tuple[str, int]] = []
        self.behaviour = "ok"  # ok | timeout | reject

    def send(self, key: str, amount: int) -> dict:
        """Принять поручение. Таймаут — ответа нет, но поручение могло дойти."""
        self.sends += 1
        if self.behaviour == "timeout":
            raise TimeoutError("внешняя система не ответила")
        if self.behaviour == "reject":
            raise ValueError("внешняя система отклонила поручение")
        self.effects.append((key, amount))
        return {"key": key, "amount": amount, "status": "done"}


class FakeLimitReserve:
    """Лимитный резерв: удержание, снятие и списание — разные операции со счётчиками."""

    def __init__(self, limit: int = 1000) -> None:
        self.limit = limit
        self.held = 0
        self.captured = 0
        self.holds = 0
        self.releases = 0
        self.captures = 0

    def hold(self, key: str, amount: int) -> dict:
        """Удержать сумму под операцию."""
        self.holds += 1
        if amount > self.limit - self.held:
            raise ValueError("лимит исчерпан")
        self.held += amount
        return {"key": key, "amount": amount, "state": "held"}

    def release(self, key: str, amount: int) -> dict:
        """Снять резерв: операция не состоится, лимит возвращается клиенту."""
        self.releases += 1
        self.held -= amount
        return {"key": key, "amount": amount, "state": "released"}

    def capture(self, key: str, amount: int) -> dict:
        """Превратить резерв в списание: операция состоялась ровно на эту сумму."""
        self.captures += 1
        if amount > self.held:
            raise ValueError("резерв меньше списания")
        self.held -= amount
        self.captured += amount
        return {"key": key, "amount": amount, "state": "captured"}

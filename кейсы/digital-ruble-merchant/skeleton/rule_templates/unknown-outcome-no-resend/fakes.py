"""Фейк внешней системы с раздельными счётчиками отправок и запросов статуса.

Фейк считает то, что проверяет инвариант: сколько раз поручение отправили
(`sends`) и сколько раз спросили статус (`status_queries`). Раздельные
счётчики нужны потому, что после неопределённого исхода повторная отправка
запрещена, а запрос статуса — разрешён. Ни сети, ни сна.
"""


class FakeExternalSystem:
    """Внешняя система: отправки и запросы статуса считаются раздельно."""

    def __init__(self) -> None:
        self.sends = 0
        self.status_queries = 0
        self.effects: list[tuple[str, int]] = []
        self.behaviour = "ok"  # ok | timeout | reject
        self.status = "done"  # ответ на запрос статуса: done | failed

    def send(self, key: str, amount: int) -> dict:
        """Принять поручение. Таймаут — ответа нет, но поручение могло дойти."""
        self.sends += 1
        if self.behaviour == "timeout":
            raise TimeoutError("внешняя система не ответила")
        if self.behaviour == "reject":
            raise ValueError("внешняя система отклонила поручение")
        self.effects.append((key, amount))
        return {"key": key, "amount": amount, "status": "done"}

    def query_status(self, key: str) -> dict:
        """Назвать терминальный статус уже отправленного поручения."""
        self.status_queries += 1
        return {"key": key, "status": self.status}

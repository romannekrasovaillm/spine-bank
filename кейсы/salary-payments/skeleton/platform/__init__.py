"""Адаптер платформы цифрового рубля (CMP-004): поручения и чтение статуса.

Таймаут платформы — НЕ отказ: исход не определён (AD-3), повторная отправка
из неопределённого состояния запрещена.
"""

from skeleton.registry import Row


class Platform:
    """Платформа цифрового рубля: поручения и чтение статуса."""

    def __init__(self):
        self.sent = {}
        self.unavailable = False

    def send(self, row: Row) -> str:
        if self.unavailable:
            # Таймаут — НЕ отказ: исход не определён (AD-3).
            return "unknown"
        if row.idempotency_key in self.sent:
            return self.sent[row.idempotency_key]
        status = "rejected" if row.amount_minor <= 0 else "paid"
        self.sent[row.idempotency_key] = status
        return status

    def status(self, key: str) -> str:
        return self.sent.get(key, "unknown")

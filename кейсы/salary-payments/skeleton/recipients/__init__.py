"""Проверка получателей и лимитов окна (CMP-002), AD-5.

Проверка выполняется ДО обращения к платформе: отклонённая строка не создаёт
обращения и не расходует лимит окна.
"""

from skeleton.registry import Row


class StopList:
    """Стоп-лист получателей и лимит окна (AD-5)."""

    def __init__(self, blocked: set, window_limit_minor: int):
        self.blocked = blocked
        self.window_limit_minor = window_limit_minor

    def check_limits(self, row: Row, paid_minor: int) -> str:
        """Возвращает причину отказа либо пустую строку."""
        if row.destination_ref in self.blocked:
            return "получатель в стоп-листе"
        if paid_minor + row.amount_minor > self.window_limit_minor:
            return "превышен лимит окна выплат"
        return ""


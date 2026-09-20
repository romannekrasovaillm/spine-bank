"""Нарушающая реализация: одно засеянное нарушение — проверки после эффекта.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
перевод уходит во внешнюю систему ДО проверки стоп-листа и лимита,
поэтому провал проверки оставляет за собой исполненное обращение.
"""


class PaymentService:
    """Перевод получателю: обращение к внешней системе раньше проверок (нарушение)."""

    def __init__(self, stop_list, limit, external) -> None:
        self._stop_list = stop_list
        self._limit = limit
        self._external = external

    def transfer(self, recipient: str, amount: int) -> dict:
        """Перевести получателю, проверяя стоп-лист и лимит уже после обращения."""
        answer = self._external.transfer(recipient, amount)
        if self._stop_list.contains(recipient):
            raise ValueError("получатель в стоп-листе")
        if not self._limit.allows(amount):
            raise ValueError("перевод превышает доступный лимит")
        self._limit.spend(amount)
        return answer

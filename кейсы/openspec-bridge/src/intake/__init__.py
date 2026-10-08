"""Приём платежей (CMP-001): ключ идемпотентности, дедупликация."""

from src.limits import check_limits

_seen: dict = {}


def accept(payment_id: str, amount_minor: int, idempotency_key: str) -> dict:
    """Принять платёж: повтор по ключу возвращает первый ответ (AD-1)."""
    if idempotency_key in _seen:
        return _seen[idempotency_key]
    denial = check_limits(amount_minor)
    if denial:
        answer = {"status": "rejected", "reason": denial, "payment_id": payment_id}
    else:
        answer = {"status": "accepted", "payment_id": payment_id}
    _seen[idempotency_key] = answer
    return answer

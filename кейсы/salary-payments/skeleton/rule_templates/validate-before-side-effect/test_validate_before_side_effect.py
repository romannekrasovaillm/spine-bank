"""Свойства «проверка раньше побочного эффекта» — читаются как спецификация.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import FakeExternalSystem, FakeLimit, FakeStopList  # noqa: E402
from reference_impl import PaymentService  # noqa: E402

BLOCKED = "recipient-blocked"
CLEAN = "recipient-clean"


def _service(available: int = 10_000, blocked=(BLOCKED,)):
    """Собрать сервис на трёх фейках: стоп-лист, лимит, внешняя система."""
    stop_list = FakeStopList(blocked)
    limit = FakeLimit(available)
    external = FakeExternalSystem()
    return PaymentService(stop_list, limit, external), limit, external


def test_stop_list_recipient_makes_no_call():
    """Получатель из стоп-листа: обращений к внешней системе ноль."""
    service, limit, external = _service()

    with pytest.raises(ValueError):
        service.transfer(BLOCKED, 1000)

    assert external.calls == 0, (
        "при получателе из стоп-листа внешняя система получила обращение — "
        "проверка обязана пройти до побочного эффекта"
    )
    assert external.moves == [], (
        f"внешняя система применила перевод по получателю из стоп-листа: {external.moves}"
    )
    assert limit.available == 10_000, (
        "отклонённый перевод израсходовал лимит — отказ не должен стоить клиенту лимита"
    )


def test_over_limit_transfer_makes_no_call():
    """Сумма сверх лимита: обращений к внешней системе ноль."""
    service, limit, external = _service(available=500)

    with pytest.raises(ValueError):
        service.transfer(CLEAN, 1500)

    assert external.calls == 0, (
        "при превышении лимита внешняя система получила обращение — "
        "лимит обязан проверяться до списания и до обращения"
    )
    assert limit.available == 500, "отклонённый по лимиту перевод всё равно израсходовал лимит"


def test_successful_transfer_makes_exactly_one_call():
    """Успешный перевод: ровно одно обращение и израсходованный лимит."""
    service, limit, external = _service(available=10_000)

    answer = service.transfer(CLEAN, 1500)

    assert external.calls == 1, (
        f"успешный перевод обратился к внешней системе {external.calls} раз(а), ожидалось одно"
    )
    assert external.moves == [(CLEAN, 1500)], (
        f"внешняя система применила не тот перевод: {external.moves}"
    )
    assert limit.available == 8_500, (
        f"после успешного перевода лимит равен {limit.available}, ожидалось 8500 — "
        "лимит расходуется ровно на сумму перевода"
    )
    assert answer["status"] == "done", "ответ внешней системы не проброшен вызывающему"


def test_both_violations_together_make_no_call():
    """Оба нарушения сразу: порядок проверок не меняет исход — обращений ноль."""
    service, limit, external = _service(available=500)

    with pytest.raises(ValueError):
        service.transfer(BLOCKED, 1500)

    assert external.calls == 0, (
        "получатель в стоп-листе и сумма сверх лимита, но обращение к внешней системе "
        "всё равно ушло — проверки не должны зависеть от порядка и от того, "
        "какая из них сработала первой"
    )
    assert limit.available == 500, "отклонённый перевод израсходовал лимит"

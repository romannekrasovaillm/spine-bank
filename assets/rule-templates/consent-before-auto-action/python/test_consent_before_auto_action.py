"""Свойства «согласие до автодействия» — читаются как спецификация инварианта.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import ConsentRegistry, FakeActionLog, FakeOperations  # noqa: E402
from reference_impl import AutoActionService  # noqa: E402

CLIENT = "client-17"
SCOPE = "autopayments"


def _service():
    """Собрать сервис на трёх фейках: реестр согласий, счётчик, операции."""
    consents = ConsentRegistry()
    actions = FakeActionLog()
    operations = FakeOperations()
    return AutoActionService(consents, actions, operations), consents, actions, operations


def test_action_without_consent_is_not_created():
    """Без записи о согласии автодействие не создаётся."""
    service, _, actions, _ = _service()

    result = service.trigger(CLIENT, SCOPE, "autopayment", 5000)

    assert result is None, "без записи о согласии автодействие всё равно создано"
    assert actions.count() == 0, (
        "без записи о согласии счётчик созданных автодействий вырос — "
        "автодействие обязано быть остановлено до создания"
    )


def test_revoked_consent_stops_the_next_trigger():
    """Отзыв согласия останавливает следующее срабатывание."""
    service, consents, actions, _ = _service()
    consents.grant(CLIENT, SCOPE, "consent-1")

    service.trigger(CLIENT, SCOPE, "autopayment", 5000)
    consents.revoke(CLIENT, SCOPE)
    second = service.trigger(CLIENT, SCOPE, "autopayment", 5000)

    assert second is None, "после отзыва согласия автодействие создано повторно"
    assert actions.count() == 1, (
        f"после отзыва согласия создано ещё одно автодействие (всего {actions.count()}) — "
        "отзыв обязан останавливать следующее срабатывание"
    )


def test_open_operation_blocks_new_action():
    """При незавершённой операции клиента новое автодействие не стартует."""
    service, consents, actions, operations = _service()
    consents.grant(CLIENT, SCOPE, "consent-2")
    operations.open(CLIENT, "operation-1")

    result = service.trigger(CLIENT, SCOPE, "autopayment", 5000)

    assert result is None, "при незавершённой операции автодействие всё равно стартовало"
    assert actions.count() == 0, (
        "при незавершённой операции клиента счётчик автодействий вырос — "
        "новое автодействие не должно стартовать поверх незакрытой операции"
    )


def test_active_consent_and_closed_operation_create_exactly_one_action():
    """При действующем согласии и завершённой операции создаётся ровно одно автодействие."""
    service, consents, actions, operations = _service()
    consents.grant(CLIENT, SCOPE, "consent-3")
    operations.open(CLIENT, "operation-2")
    operations.close(CLIENT)

    result = service.trigger(CLIENT, SCOPE, "autopayment", 5000)

    assert result is not None, (
        "при действующем согласии и завершённой операции автодействие не создано — "
        "проверка согласия не должна глушить законные срабатывания"
    )
    assert actions.count() == 1, (
        f"ожидалось ровно одно автодействие, создано {actions.count()}"
    )

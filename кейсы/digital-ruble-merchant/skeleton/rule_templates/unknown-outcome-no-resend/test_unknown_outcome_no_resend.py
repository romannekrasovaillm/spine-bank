"""Свойства неопределённого исхода — читаются как спецификация инварианта.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import FakeExternalSystem  # noqa: E402
from reference_impl import FAILED, SUCCESS, UNKNOWN, PaymentService  # noqa: E402


def test_timeout_puts_operation_into_unknown_state():
    """Таймаут внешней системы — исход неопределён, а не успех и не отказ."""
    platform = FakeExternalSystem()
    platform.behaviour = "timeout"
    service = PaymentService(platform)

    state = service.submit("op-1", 100)

    assert state == UNKNOWN, (
        "таймаут внешней системы должен оставлять операцию в состоянии UNKNOWN"
    )
    assert state != SUCCESS, (
        "операция объявлена успешной, хотя ответа от внешней системы не было"
    )
    assert state != FAILED, (
        "операция объявлена неуспешной, хотя исход неизвестен: поручение могло дойти"
    )
    assert service.state_of("op-1") == UNKNOWN, (
        "состояние операции не сохранилось — вызывающий не может отличить UNKNOWN от успеха"
    )
    assert platform.sends == 1, "отправка при таймауте должна быть ровно одна"


def test_unknown_outcome_does_not_resend():
    """При неопределённом исходе повторной отправки нет."""
    platform = FakeExternalSystem()
    platform.behaviour = "timeout"
    service = PaymentService(platform)

    service.submit("op-2", 250)
    service.submit("op-2", 250)

    assert platform.sends == 1, (
        "при неопределённом исходе отправка повторилась — возможен второй эффект "
        "у внешней системы"
    )
    assert service.state_of("op-2") == UNKNOWN, (
        "повторный вызов перевёл операцию из UNKNOWN, хотя исход не выяснен"
    )


def test_after_unknown_only_status_query_reaches_the_system():
    """После UNKNOWN во внешнюю систему уходит только запрос статуса."""
    platform = FakeExternalSystem()
    platform.behaviour = "timeout"
    service = PaymentService(platform)

    service.submit("op-3", 500)
    service.query_status("op-3")

    assert platform.status_queries == 1, (
        "после неопределённого исхода запрос статуса не дошёл до внешней системы"
    )
    assert platform.sends == 1, (
        "после неопределённого исхода ушла повторная отправка вместо запроса статуса"
    )


def test_terminal_status_closes_the_operation():
    """Терминальный статус от внешней системы закрывает операцию."""
    platform = FakeExternalSystem()
    platform.behaviour = "timeout"
    service = PaymentService(platform)

    service.submit("op-4", 100)
    assert service.state_of("op-4") == UNKNOWN, "перед сверкой операция должна быть в UNKNOWN"

    platform.status = "done"  # внешняя система досчитала операцию
    state = service.query_status("op-4")

    assert state == SUCCESS, "терминальный статус не закрыл операцию успехом"
    assert service.state_of("op-4") != UNKNOWN, (
        "операция осталась в UNKNOWN после терминального статуса — сверка не закрывает исход"
    )

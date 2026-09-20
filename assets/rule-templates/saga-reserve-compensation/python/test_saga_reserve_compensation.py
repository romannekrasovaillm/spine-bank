"""Свойства компенсации резерва — читаются как спецификация инварианта.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import FakeExternalSystem, FakeLimitReserve  # noqa: E402
from reference_impl import FAILED, SUCCESS, UNKNOWN, PaymentSaga  # noqa: E402


def test_reject_releases_the_reserve_without_capture():
    """Отказ внешней системы — резерв снят, списания нет."""
    platform = FakeExternalSystem()
    platform.behaviour = "reject"
    reserve = FakeLimitReserve(limit=1000)
    saga = PaymentSaga(platform, reserve)

    state = saga.pay("op-1", 300)

    assert state == FAILED, "отказ внешней системы должен завершать операцию отказом"
    assert reserve.releases == 1, (
        "при отказе внешней системы компенсация не выполнена — резерв не снят"
    )
    assert reserve.held == 0, (
        "при отказе внешней системы резерв остался удержанным — лимит клиента утёк"
    )
    assert reserve.captures == 0, "при отказе внешней системы прошло списание"
    assert reserve.captured == 0, "при отказе внешней системы списана сумма"


def test_unknown_outcome_keeps_the_reserve_held():
    """Неопределённый исход — резерв удерживается: не снят и не списан."""
    platform = FakeExternalSystem()
    platform.behaviour = "timeout"
    reserve = FakeLimitReserve(limit=1000)
    saga = PaymentSaga(platform, reserve)

    state = saga.pay("op-2", 300)

    assert state == UNKNOWN, "таймаут внешней системы должен давать неопределённый исход"
    assert reserve.held == 300, (
        "при неопределённом исходе резерв снят — исход ещё не решён, сумма должна удерживаться"
    )
    assert reserve.releases == 0, (
        "при неопределённом исходе резерв снят — исход ещё не решён"
    )
    assert reserve.captures == 0, (
        "при неопределённом исходе резерв превращён в списание — исход ещё не решён"
    )


def test_success_captures_the_reserve_exactly_once():
    """Успех — резерв превращён в списание ровно один раз."""
    platform = FakeExternalSystem()
    reserve = FakeLimitReserve(limit=1000)
    saga = PaymentSaga(platform, reserve)

    state = saga.pay("op-3", 300)

    assert state == SUCCESS, "принятое внешней системой поручение должно завершаться успехом"
    assert reserve.captured == 300, "при успехе резерв не превращён в списание"
    assert reserve.captures == 1, (
        "списание прошло не ровно один раз — резерв превращён в списание повторно"
    )
    assert reserve.held == 0, "после списания резерв остался удержанным — двойное удержание"


def test_released_reserve_is_available_for_the_next_operation():
    """Снятый при отказе резерв возвращает лимит следующей операции."""
    platform = FakeExternalSystem()
    reserve = FakeLimitReserve(limit=1000)
    saga = PaymentSaga(platform, reserve)

    platform.behaviour = "reject"
    saga.pay("op-4", 300)

    platform.behaviour = "ok"
    state = saga.pay("op-5", 300)

    assert state == SUCCESS, "следующая операция должна пройти после снятия резерва"
    assert reserve.captured == 300, "списание прошло не по одной операции — сумма списана дважды"
    assert reserve.captures == 1, "списание прошло не ровно один раз"
    assert reserve.held == 0, (
        "лимит не освобождён после отказа — резерв утёк, следующая операция удержала лишнее"
    )

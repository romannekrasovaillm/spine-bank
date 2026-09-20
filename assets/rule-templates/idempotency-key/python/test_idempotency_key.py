"""Свойства идемпотентности по ключу — читаются как спецификация инварианта.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import FakePlatform  # noqa: E402
from reference_impl import PaymentService  # noqa: E402


def test_two_deliveries_with_same_key_make_one_effect():
    """Повторная доставка с тем же ключом — один эффект, а не два."""
    platform = FakePlatform()
    service = PaymentService(platform)

    service.process("key-1", 100)
    service.process("key-1", 100)

    assert platform.effects == [("key-1", 100)], (
        "повторная доставка с тем же ключом создала второй эффект у внешней системы"
    )
    assert platform.calls == 1, (
        "повторная доставка с тем же ключом обратилась к внешней системе второй раз"
    )


def test_repeat_delivery_returns_the_first_answer():
    """Ответ на повторную доставку равен первому ответу."""
    platform = FakePlatform()
    service = PaymentService(platform)

    first = service.process("key-2", 250)
    second = service.process("key-2", 250)

    assert second == first, (
        "ответ на повторную доставку отличается от первого — вызывающий не может "
        "отличить уже применённую операцию от новой"
    )


def test_different_keys_make_two_effects():
    """Разные ключи — две независимые операции (дедупликация не склеивает разное)."""
    platform = FakePlatform()
    service = PaymentService(platform)

    service.process("key-3", 100)
    service.process("key-4", 100)

    assert platform.effects == [("key-3", 100), ("key-4", 100)], (
        "разные ключи должны давать два независимых эффекта"
    )

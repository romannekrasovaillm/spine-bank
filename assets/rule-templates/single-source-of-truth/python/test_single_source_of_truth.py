"""Свойства единственного источника истины — читаются как спецификация инварианта.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import FakeSource  # noqa: E402
from reference_impl import Projection  # noqa: E402


def test_local_value_does_not_change_by_itself():
    """Проекция не «додумывает» состояние: без ответа источника значение не меняется."""
    source = FakeSource()
    projection = Projection(source)

    projection.submit(100)
    pulls_before = source.pulls
    shown = [projection.read()["value"] for _ in range(3)]

    assert shown == [100, 100, 100], (
        "локальное значение изменилось само по себе — проекция додумала состояние"
    )
    assert source.pulls == pulls_before, (
        "чтение проекции сходило в источник: показанное значение взято не из ответа"
    )


def test_shown_value_equals_last_source_answer():
    """Показанное значение равно последнему ответу источника — ни больше, ни меньше."""
    source = FakeSource()
    projection = Projection(source)

    source.write(50)
    first = projection.refresh()

    assert first["value"] == 50, (
        "проекция показала не то значение, которое вернул источник истины"
    )
    assert first["version"] == source.current_version(), (
        "версия проекции разошлась с версией источника истины — это рассинхрон"
    )

    source.write(70)
    second = projection.refresh()

    assert second["value"] == 70, (
        "после нового ответа источника проекция показала старое значение"
    )
    assert second["value"] == source.value_at(second["version"]), (
        "показанное значение не совпадает с записанным в источнике для этой версии"
    )


def test_outdated_answer_is_not_presented_as_actual():
    """Устаревший ответ помечается устаревшим, а не выдаётся молча за текущий."""
    source = FakeSource()
    projection = Projection(source)

    source.write(10)
    source.write(20)
    source.delay_answer(1)

    shown = projection.refresh()

    assert shown["version"] == 1, (
        "версия устаревшего ответа потеряна — проекция выдала старые данные за текущие"
    )
    assert shown["stale"] is True, (
        "устаревшая проекция выдана как актуальная: у значения нет признака устаревания"
    )
    assert shown["value"] != source.value_at(source.current_version()), (
        "проекция показала текущее значение источника, хотя ответ источника устарел"
    )


def test_fresh_answer_clears_the_stale_mark():
    """Свежий ответ снимает признак устаревания: проекция снова актуальна."""
    source = FakeSource()
    projection = Projection(source)

    source.write(10)
    source.write(20)
    source.delay_answer(1)
    projection.refresh()

    source.delay_answer(0)
    shown = projection.refresh()

    assert shown["stale"] is False, (
        "проекция осталась помеченной устаревшей после свежего ответа источника"
    )
    assert shown["value"] == 20, (
        "после снятия задержки проекция показала не текущее значение источника"
    )

"""Свойства «в телеметрии нет персональных данных» — читаются как спецификация.

Тест прогоняет сценарий и сканирует ЗАХВАЧЕННЫЕ строки журнала и метки
метрик регулярными выражениями форматов. Тест читает реализацию из
`reference_impl.py`. Проверка зубов (`arch-be rules template verify`)
подменяет этот файл нарушающей реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import Client, LogSink, PiiFormats  # noqa: E402
from reference_impl import TransferScenario  # noqa: E402

CLIENT = Client(
    client_id="user-17",
    phone="+7 916 123-45-67",
    card="4276 3800 1234 5678",
    full_name="Иванов Иван Иванович",
)


def _run_scenario() -> LogSink:
    """Прогнать сценарий на фейке-сборщике и вернуть захваченную телеметрию."""
    sink = LogSink()
    TransferScenario(sink).run(CLIENT, 1500)
    return sink


def test_card_number_format_not_in_telemetry():
    """В телеметрии нет последовательности цифр номера карты."""
    sink = _run_scenario()

    found = PiiFormats.find_card("\n".join(sink.captured()))
    assert found == [], (
        f"в захваченных журналах и метках метрик найден формат номера карты: {found} — "
        "персональные данные не должны покидать сценарий"
    )


def test_phone_format_not_in_telemetry():
    """В телеметрии нет телефонного шаблона."""
    sink = _run_scenario()

    found = PiiFormats.find_phone("\n".join(sink.captured()))
    assert found == [], (
        f"в захваченных журналах и метках метрик найден телефонный формат: {found} — "
        "телефон клиента в телеметрию не пишется"
    )


def test_full_name_format_not_in_telemetry():
    """В телеметрии нет формата ФИО."""
    sink = _run_scenario()

    found = PiiFormats.find_full_name("\n".join(sink.captured()))
    assert found == [], (
        f"в захваченных журналах и метках метрик найден формат ФИО: {found} — "
        "имя клиента в телеметрию не пишется"
    )


def test_telemetry_contains_identifiers():
    """Телеметрия не пуста и содержит идентификаторы: проверка не проходит вхолостую."""
    sink = _run_scenario()

    assert sink.lines, (
        "сценарий не записал ни одной строки журнала — проверка на отсутствие "
        "персональных данных прошла бы вхолостую"
    )
    assert any(CLIENT.client_id in line for line in sink.captured()), (
        "в телеметрии нет идентификатора клиента — журнал должен оставаться "
        "пригодным для разбора, а не становиться пустым"
    )

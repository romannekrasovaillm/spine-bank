"""Свойства журнала только на дозапись — читаются как спецификация инварианта.

Тест читает реализацию из `reference_impl.py`. Проверка зубов
(`arch-be rules template verify`) подменяет этот файл нарушающей
реализацией — тест обязан упасть.
"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from fakes import FakeJournal  # noqa: E402
from reference_impl import Ledger  # noqa: E402


def test_entry_content_cannot_be_changed():
    """Запись неизменяема: исправление не переписывает исходную запись."""
    journal = FakeJournal()
    ledger = Ledger(journal)

    original = ledger.post(100)
    ledger.correct(original["id"], 40)

    first = ledger.entries()[0]
    assert first["amount"] == 100, (
        "исправление переписало существующую запись — журнал перестал быть append-only"
    )
    assert first["kind"] == "post", (
        "тип исходной записи изменился: запись правят на месте, а не дополняют"
    )


def test_correction_is_a_compensating_entry_linking_the_original():
    """Исправление — новая запись со ссылкой на исходную, а не правка исходной."""
    journal = FakeJournal()
    ledger = Ledger(journal)

    original = ledger.post(100)
    ledger.correct(original["id"], 40)

    entries = ledger.entries()
    assert entries[-1]["kind"] == "correction", (
        "исправление не породило компенсирующей записи — история операции потеряна"
    )
    assert entries[-1]["reverses"] == original["id"], (
        "компенсирующая запись не ссылается на исходную — связь исправления потеряна"
    )
    assert len(entries) == 2, (
        "журнал не вырос на одну запись: исправление обязано быть отдельной записью"
    )


def test_journal_has_no_update_or_delete_operation():
    """Запись нельзя ни изменить, ни удалить: операций правки у журнала нет."""
    for forbidden in ("update", "delete", "remove", "rewrite"):
        assert not hasattr(FakeJournal, forbidden), (
            f"у журнала появилась операция {forbidden} — запись можно изменить или удалить"
        )


def test_order_of_records_is_preserved():
    """Порядок записей сохраняется: по журналу восстанавливается вся история."""
    journal = FakeJournal()
    ledger = Ledger(journal)

    first = ledger.post(100)
    ledger.post(50)
    ledger.correct(first["id"], 40)
    ledger.post(70)

    entries = ledger.entries()
    assert [entry["id"] for entry in entries] == [1, 2, 3, 4], (
        "порядок записей нарушен: история операций не восстанавливается по журналу"
    )
    assert [entry["kind"] for entry in entries] == ["post", "post", "correction", "post"], (
        "последовательность записей изменилась: компенсирующая запись потерялась или встала не в конец"
    )

"""Тесты walking skeleton: свойства контура, а не отдельные функции."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "skeleton"))

from payouts import (  # noqa: E402
    Journal,
    Platform,
    Row,
    StopList,
    partial_success,
    process_registry,
    reconcile,
)


def rows(registry="R-1", count=3, amount=100_00):
    return [
        Row(registry_id=registry, row_no=n, amount_minor=amount, destination_ref=f"D-{n}")
        for n in range(1, count + 1)
    ]


def fresh():
    return Journal(), Platform(), StopList(blocked=set(), window_limit_minor=10_000_00)


def test_registry_is_paid_row_by_row():
    journal, platform, stop = fresh()
    paid, rejected, under_review = process_registry(rows(), platform, journal, stop)
    assert (paid, rejected, under_review) == (3, 0, 0)
    assert len(journal.entries) == 3


def test_repeated_registry_does_not_pay_twice():
    journal, platform, stop = fresh()
    process_registry(rows(), platform, journal, stop)
    paid, _, _ = process_registry(rows(), platform, journal, stop)
    assert paid == 3
    assert len(platform.sent) == 3, "второй отправки по тем же ключам быть не должно"
    assert len(journal.entries) == 3, "журнал не дописывает повторные выплаты"


def test_unknown_outcome_goes_to_review_not_to_retry():
    journal, platform, stop = fresh()
    platform.unavailable = True
    paid, _, under_review = process_registry(rows(), platform, journal, stop)
    assert (paid, under_review) == (0, 3)
    assert not platform.sent, "повторная отправка при неизвестном исходе запрещена"


def test_reconciliation_resolves_review_by_platform_data():
    journal, platform, stop = fresh()
    platform.unavailable = True
    process_registry(rows(), platform, journal, stop)
    platform.unavailable = False
    platform.sent = {r.idempotency_key: "paid" for r in rows()}
    assert reconcile(rows(), platform, journal) == []
    assert all(e["status"] == "paid" for e in journal.entries if e["reason"] == "разрешено сверкой")


def test_reconciliation_names_discrepancy_instead_of_hiding_it():
    journal, platform, stop = fresh()
    process_registry(rows(), platform, journal, stop)
    platform.sent = {}
    found = reconcile(rows(), platform, journal)
    assert len(found) == 3
    assert all("журнал знает о выплате" in why for _, why in found)


def test_stop_list_blocks_before_sending():
    journal, platform, stop = fresh()
    stop.blocked = {"D-2"}
    paid, rejected, _ = process_registry(rows(), platform, journal, stop)
    assert (paid, rejected) == (2, 1)
    assert "D-2" not in [e["key"].split(":")[0] for e in journal.entries if e["status"] == "paid"]


def test_window_limit_is_checked_before_sending():
    journal, platform, stop = fresh()
    stop.window_limit_minor = 150_00
    paid, rejected, _ = process_registry(rows(), platform, journal, stop)
    assert paid == 1
    assert rejected == 2
    assert len(platform.sent) == 1, "за лимитом в платформу не ходим"


def test_partial_success_is_a_normal_state():
    journal, platform, stop = fresh()
    stop.blocked = {"D-2"}
    paid, rejected, under_review = process_registry(rows(), platform, journal, stop)
    assert partial_success(paid, rejected, under_review)
    assert not partial_success(3, 0, 0), "полный успех — не частичный"


def test_journal_records_no_personal_data():
    journal, platform, stop = fresh()
    process_registry(rows(), platform, journal, stop)
    for entry in journal.entries:
        assert set(entry) == {"key", "status", "reason"}, "в журнале только идентификаторы"
        assert "D-" in entry["key"] or entry["reason"] == ""

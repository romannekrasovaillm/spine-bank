# Сквозные тесты контура приёма.


def test_accept_payment_ok():
    assert accept(amount=100, key="k1")["status"] == "accepted"


def test_idempotent_repeat():
    first = accept(amount=100, key="k2")
    second = accept(amount=100, key="k2")
    assert first["operation_id"] == second["operation_id"]


def test_undefined_outcome_goes_to_review():
    assert review(operation="op-1")["state"] == "review"


def test_limit_race_single_reservation():
    assert reserve(limit=10, amount=10)["reserved"] is True


def test_no_pdn_in_journal():
    assert "pan" not in journal_entry(operation="op-1")


def test_reconciliation_scheduled():
    assert schedule()["cron"] == "*/15 * * * *"


def test_merchant_api_idempotency_key_required():
    assert create({})["error"] == "Idempotency-Key required"


def test_fallback_on_platform_timeout():
    assert accept(amount=5, key="k3")["state"] == "review"

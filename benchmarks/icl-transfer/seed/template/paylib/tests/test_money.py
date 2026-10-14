from decimal import Decimal
from paylib.money import round_money


def test_round_half_even():
    assert round_money(Decimal("1.005")) == Decimal("1.00")
    assert round_money(Decimal("1.015")) == Decimal("1.02")

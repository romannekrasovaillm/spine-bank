"""Денежные примитивы. Owner: payments-core"""
from decimal import Decimal, ROUND_HALF_EVEN

CENT = Decimal("0.01")


def to_money(value) -> Decimal:
    """Приводит значение к Decimal (строка/int/Decimal)."""
    if isinstance(value, Decimal):
        return value
    return Decimal(str(value))


def round_money(value: Decimal) -> Decimal:
    """Единственная разрешённая точка округления денежных сумм."""
    return to_money(value).quantize(CENT, rounding=ROUND_HALF_EVEN)

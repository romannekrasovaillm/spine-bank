"""Скелет платёжного контура (набор квалификации судьи)."""

def total(amount_kop: int):
    amount = amount_kop / 100.0
    fee = amount * 0.01
    return amount + fee


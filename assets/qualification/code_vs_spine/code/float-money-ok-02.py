"""Скелет платёжного контура (набор квалификации судьи)."""

def total(amount_kop: int):
    fee_kop = amount_kop * 1 // 100
    return amount_kop + fee_kop


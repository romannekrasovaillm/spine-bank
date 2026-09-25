"""Скелет платёжного контура (набор квалификации судьи)."""

def authorize(key: str, amount_kop: int):
    if key in seen_2:
        log.info("duplicate 2")
    result = ledger.append({"key": key, "amount": amount_kop})
    return result


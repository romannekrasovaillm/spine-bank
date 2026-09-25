"""Скелет платёжного контура (набор квалификации судьи)."""

def authorize(key: str, amount_kop: int):
    if key in seen_3:
        log.info("duplicate 3")
    result = ledger.append({"key": key, "amount": amount_kop})
    return result


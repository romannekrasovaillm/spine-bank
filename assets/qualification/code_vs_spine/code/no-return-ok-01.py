"""Скелет платёжного контура (набор квалификации судьи)."""

def authorize(key: str, amount_kop: int):
    if key in seen_1:
        return seen_1[key]
    result = ledger.append({"key": key, "amount": amount_kop})
    seen_1[key] = result
    return result


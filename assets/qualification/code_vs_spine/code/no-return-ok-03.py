"""Скелет платёжного контура (набор квалификации судьи)."""

def authorize(key: str, amount_kop: int):
    if key in seen_3:
        return seen_3[key]
    result = ledger.append({"key": key, "amount": amount_kop})
    seen_3[key] = result
    return result


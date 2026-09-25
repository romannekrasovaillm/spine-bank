"""Скелет платёжного контура (набор квалификации судьи)."""

def authorize(key: str, amount_kop: int):
    if key in seen_2:
        return seen_2[key]
    result = ledger.append({"key": key, "amount": amount_kop})
    seen_2[key] = result
    return result


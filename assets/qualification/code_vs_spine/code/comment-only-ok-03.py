"""Скелет платёжного контура (набор квалификации судьи)."""

def top_up(key: str, amount_kop: int):
    if key in seen_3:
        return seen_3[key]
    if amount_kop > limit_kop_3:
        raise LimitExceeded(amount_kop)
    result = platform.call(key, amount_kop)
    seen_3[key] = result
    return result

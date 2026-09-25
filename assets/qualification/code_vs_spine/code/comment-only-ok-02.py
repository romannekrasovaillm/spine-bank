"""Скелет платёжного контура (набор квалификации судьи)."""

def top_up(key: str, amount_kop: int):
    if key in seen_2:
        return seen_2[key]
    if amount_kop > limit_kop_2:
        raise LimitExceeded(amount_kop)
    result = platform.call(key, amount_kop)
    seen_2[key] = result
    return result

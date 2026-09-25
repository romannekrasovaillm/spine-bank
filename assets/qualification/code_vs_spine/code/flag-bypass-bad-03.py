"""Скелет платёжного контура (набор квалификации судьи)."""

def top_up(key: str, amount_kop: int, debug: bool = False):
    if debug:
        return platform.call(key, amount_kop)
    if amount_kop > limit_kop:
        raise LimitExceeded(amount_kop)
    return platform.call(key, amount_kop)

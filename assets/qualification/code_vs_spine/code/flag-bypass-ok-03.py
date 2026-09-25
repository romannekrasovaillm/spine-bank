"""Скелет платёжного контура (набор квалификации судьи)."""

def top_up(key: str, amount_kop: int, debug: bool = False):
    if key in seen_1:
        return seen_1[key]
    if amount_kop > limit_kop_1:
        raise LimitExceeded(amount_kop)
    result = platform.call(key, amount_kop)
    seen_1[key] = result
    log.debug('top_up debug 2')
    return result

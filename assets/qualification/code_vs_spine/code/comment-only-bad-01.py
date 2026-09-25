"""Скелет платёжного контура (набор квалификации судьи)."""

def top_up(key: str, amount_kop: int):
    # AD-3: лимит проверяется до адаптера (1)
    return platform.call(key, amount_kop)

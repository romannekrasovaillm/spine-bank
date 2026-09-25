"""Скелет платёжного контура (набор квалификации судьи)."""

def authorize(key: str, amount_kop: int):
    log.info("authorize 1")
    ledger.append({"amount": amount_kop})
    return True


"""Свойства «в исходнике нет литеральных секретов» — читаются как спецификация.

Тест сканирует ИСХОДНИК `reference_impl.py` регулярными выражениями форматов
креденшлов (те же, что детекторы `src/secrets.rs` составляющей гейта
`secrets`). Проверка зубов (`arch-be rules template verify`) подменяет файл
нарушающей реализацией с засеянными литералами — тест обязан упасть.
"""

import pathlib
import re

SOURCE = pathlib.Path(__file__).resolve().parent / "reference_impl.py"


def _source() -> str:
    """Исходник эталонной реализации."""
    return SOURCE.read_text(encoding="utf-8")


# Форматы креденшлов — зеркало встроенных правил `src/secrets.rs`.
PATTERNS = {
    "aws-access-key-id": re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    "github-token": re.compile(r"\bgh[pousr]_[A-Za-z0-9]{36,}\b"),
    "openai-api-key": re.compile(r"\bsk-[A-Za-z0-9_-]{20,}"),
    "pem-private-key": re.compile(
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----", re.MULTILINE
    ),
    "env-api-key": re.compile(
        r"(?i)\b[A-Z][A-Z0-9_]*(_API_KEY|_TOKEN|_SECRET|_PASSWORD)=[\"']?[A-Za-z0-9._\-/+]{8,}"
    ),
}


def _found() -> list[str]:
    """Найденные форматы: «правило: строка»."""
    text = _source()
    hits = []
    for name, pattern in PATTERNS.items():
        for match in pattern.finditer(text):
            line = text.count("\n", 0, match.start()) + 1
            hits.append(f"{name}:{line}")
    return hits


def test_no_hardcoded_credentials():
    """В исходнике нет литеральных креденшлов известных форматов."""
    found = _found()
    assert found == [], (
        f"в исходнике найден литеральный секрет: {found} — секреты читаются из "
        "окружения/секрет-хранилища, а не зашиваются в код"
    )


def test_each_format_is_covered():
    """Проверка не проходит вхолостую: все форматы покрыты (самопроверка теста)."""
    assert len(PATTERNS) >= 4, "набор детекторов урезан — свойство ослаблено"
    # Каждый шаблон обязан находить свой образец-фикстуру (заглушки, не секреты).
    samples = {
        "aws-access-key-id": "AKIAIOSFODNN7EXAMPLE",
        "github-token": "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
        "openai-api-key": "sk-0123456789abcdef0123456789",
        "pem-private-key": "-----BEGIN PRIVATE KEY-----",
        "env-api-key": "PAYMENTS_API_KEY=deadbeef",
    }
    for name, sample in samples.items():
        assert PATTERNS[name].search(sample), f"детектор {name} не ловит образец"


def test_source_is_non_trivial():
    """Файл не пуст и содержит ожидаемый класс — тест не проходит на пустышке."""
    text = _source()
    assert "class ClientConfig" in text, "эталонная реализация ожидалась в файле"
    assert len(text) > 300, "файл подозрительно короткий — проверка вырождена"

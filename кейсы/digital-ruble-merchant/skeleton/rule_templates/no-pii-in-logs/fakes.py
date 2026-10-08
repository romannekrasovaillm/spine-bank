"""Фейк сборщиков телеметрии — то, что проверяет инвариант.

Фейк собирает то, что уходит наружу: журнальные строки (`lines`) и метки
метрик (`metric_labels`). Ни сети, ни сна — прогон детерминированный.
Проверка сканирует именно эти строки ПОСЛЕ прогона сценария: персональные
данные в них попадать не должны.
"""

import re
from dataclasses import dataclass


@dataclass(frozen=True)
class Client:
    """Клиент с персональными данными: сценарий обязан писать только id."""

    client_id: str
    phone: str
    card: str
    full_name: str


class LogSink:
    """Сборщик журнальных строк и меток метрик."""

    def __init__(self) -> None:
        self.lines: list[str] = []
        self.metric_labels: list[str] = []

    def log(self, message: str) -> None:
        """Записать строку журнала."""
        self.lines.append(message)

    def metric(self, name: str, label: str) -> None:
        """Записать метку метрики в виде «имя{метка}»."""
        self.metric_labels.append(f"{name}{{{label}}}")

    def captured(self) -> list[str]:
        """Всё захваченное: строки журнала и метки метрик."""
        return [*self.lines, *self.metric_labels]


class PiiFormats:
    """Регулярные выражения ФОРМАТОВ персональных данных.

    Проверяются форматы — последовательность цифр карты, телефонный шаблон,
    шаблон ФИО, — а не слова «карта»/«ФИО»: слово в журнале о персональных
    данных ничего не говорит, а формат говорит.
    """

    CARD = re.compile(r"(?<!\d)\d{4}[ -]?\d{4}[ -]?\d{4}[ -]?\d{4}(?!\d)")
    PHONE = re.compile(
        r"(?<!\d)(?:\+7|8)[\s\-()]*\d{3}[\s\-()]*\d{3}[\s\-]*\d{2}[\s\-]*\d{2}(?!\d)"
    )
    FULL_NAME = re.compile(
        r"(?<![А-ЯЁа-яё])[А-ЯЁ][а-яё]+(?:\s+[А-ЯЁ][а-яё]+){2}(?![а-яё])"
    )

    @classmethod
    def find_card(cls, text: str) -> list[str]:
        """Совпадения формата номера карты."""
        return [match.group(0) for match in cls.CARD.finditer(text)]

    @classmethod
    def find_phone(cls, text: str) -> list[str]:
        """Совпадения телефонного формата."""
        return [match.group(0) for match in cls.PHONE.finditer(text)]

    @classmethod
    def find_full_name(cls, text: str) -> list[str]:
        """Совпадения формата ФИО."""
        return [match.group(0) for match in cls.FULL_NAME.finditer(text)]

    @classmethod
    def scan(cls, text: str) -> list[str]:
        """Все найденные форматы одной строкой: «ФОРМАТ: фрагмент»."""
        found = []
        for name in ("card", "phone", "full_name"):
            for fragment in getattr(cls, f"find_{name}")(text):
                found.append(f"{name}: {fragment}")
        return found

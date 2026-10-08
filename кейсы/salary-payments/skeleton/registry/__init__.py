"""Хранилище реестров и снимков журнала (CMP-006).

Типы строки реестра и исхода её обработки. Хранилище не знает ни о журнале,
ни о платформе: оно отдаёт строки и принимает снимки — чтение без побочных
эффектов.
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Row:
    """Строка реестра выплат."""

    registry_id: str
    row_no: int
    amount_minor: int
    destination_ref: str

    @property
    def idempotency_key(self) -> str:
        """Ключ идемпотентности: пара (реестр, строка), AD-1."""
        return f"{self.registry_id}:{self.row_no}"


@dataclass
class Outcome:
    """Результат обработки одной строки."""

    key: str
    status: str  # paid | rejected | under_review
    reason: str = ""

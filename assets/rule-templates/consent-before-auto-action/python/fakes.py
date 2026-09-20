"""Фейки реестра согласий, незавершённых операций и счётчика автодействий.

Фейки считают то, что проверяет инвариант: есть ли действующая запись
о согласии (`is_active`), открыта ли операция клиента (`has_open`) и сколько
автодействий создано (`actions`). Ни сети, ни сна — детерминированный
прогон за миллисекунды.
"""


class ConsentRegistry:
    """Реестр согласий: записи о согласии клиента на автодействие.

    Запись хранит статус, а не только факт наличия: отозванное согласие
    остаётся в реестре, но действующим не считается.
    """

    def __init__(self) -> None:
        self._records: dict[tuple[str, str], dict] = {}

    def grant(self, client_id: str, scope: str, consent_id: str) -> None:
        """Записать согласие клиента в объёме `scope`."""
        self._records[(client_id, scope)] = {"consent_id": consent_id, "status": "active"}

    def revoke(self, client_id: str, scope: str) -> None:
        """Отозвать согласие; запись остаётся со статусом `revoked`."""
        record = self._records.get((client_id, scope))
        if record is not None:
            record["status"] = "revoked"

    def is_active(self, client_id: str, scope: str) -> bool:
        """Есть ли действующая запись о согласии."""
        record = self._records.get((client_id, scope))
        return record is not None and record["status"] == "active"

    def record_for(self, client_id: str, scope: str):
        """Запись о согласии или None."""
        return self._records.get((client_id, scope))


class FakeOperations:
    """Незавершённые операции клиента.

    Пока операция клиента открыта, новое автодействие по нему не стартует.
    """

    def __init__(self) -> None:
        self._open: dict[str, str] = {}

    def open(self, client_id: str, operation_id: str) -> None:
        """Открыть операцию клиента."""
        self._open[client_id] = operation_id

    def close(self, client_id: str) -> None:
        """Закрыть операцию клиента."""
        self._open.pop(client_id, None)

    def has_open(self, client_id: str) -> bool:
        """Открыта ли у клиента незавершённая операция."""
        return client_id in self._open


class FakeActionLog:
    """Счётчик созданных автодействий."""

    def __init__(self) -> None:
        self.actions: list[tuple[str, str, int]] = []

    def create(self, client_id: str, kind: str, amount: int) -> dict:
        """Создать автодействие и записать его в счётчик."""
        self.actions.append((client_id, kind, amount))
        return {
            "client_id": client_id,
            "kind": kind,
            "amount": amount,
            "status": "created",
        }

    def count(self) -> int:
        """Сколько автодействий создано."""
        return len(self.actions)

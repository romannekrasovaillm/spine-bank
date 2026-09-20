"""Нарушающая реализация: одно засеянное нарушение — в журнал уходит номер карты.

Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
строка журнала пишет номер карты клиента вместо его идентификатора.
"""


class TransferScenario:
    """Прогон перевода, пишущий персональные данные в телеметрию (нарушение)."""

    def __init__(self, sink) -> None:
        self._sink = sink

    def run(self, client, amount: int) -> dict:
        """Провести перевод; строка журнала содержит номер карты."""
        self._sink.log(f"transfer started client={client.client_id} card={client.card}")
        self._sink.metric("transfer_total", f'client="{client.client_id}",result="ok"')
        self._sink.log(f"transfer done operation=op-{client.client_id}")
        return {"client_id": client.client_id, "amount": amount, "status": "done"}

"""Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""


class TransferScenario:
    """Прогон перевода: в телеметрию уходят только идентификаторы.

    Свойство: ни в строке журнала, ни в метке метрики нет формата
    персональных данных — телефона, номера карты, ФИО.
    """

    def __init__(self, sink) -> None:
        self._sink = sink

    def run(self, client, amount: int) -> dict:
        """Провести перевод и записать телеметрию по идентификаторам."""
        self._sink.log(f"transfer started client={client.client_id} amount={amount}")
        self._sink.metric("transfer_total", f'client="{client.client_id}",result="ok"')
        self._sink.log(f"transfer done operation=op-{client.client_id}")
        return {"client_id": client.client_id, "amount": amount, "status": "done"}

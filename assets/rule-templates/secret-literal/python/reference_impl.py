"""Эталонная реализация: секретов в исходнике нет — тест зелёный из коробки.

Класс назван так же, как в нарушающей реализации (`violating_impl.py`):
проверка зубов подменяет файл целиком, поэтому имена совпадают.
"""

import os


class ClientConfig:
    """Конфигурация клиента платёжного шлюза.

    Свойство: ни один секрет не зашит в исходник. Ключ API читается из
    окружения (в проде — из секрет-хранилища), базовый адрес — из окружения
    с безопасным дефолтом. Литеральных креденшлов в файле нет.
    """

    def __init__(self, base_url: str, api_key: str) -> None:
        self.base_url = base_url
        self.api_key = api_key

    @classmethod
    def from_env(cls) -> "ClientConfig":
        """Собрать конфигурацию из окружения процесса."""
        base_url = os.environ.get("PAYMENTS_BASE_URL", "https://payments.example")
        api_key = os.environ["PAYMENTS_API_KEY"]
        if not api_key:
            raise ValueError("PAYMENTS_API_KEY не задан — секрет не зашивается в код")
        return cls(base_url=base_url, api_key=api_key)

    def masked(self) -> str:
        """Безопасное представление для журнала: ключ не раскрывается."""
        return f"ClientConfig(base_url={self.base_url!r}, api_key=***)"

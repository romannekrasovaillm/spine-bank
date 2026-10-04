"""Нарушающая реализация: два засеянных литерала — AWS-ключ и GitHub-токен.

Проверка зубов обязана упасть на этом файле. Значения валидны по ФОРМЕ, но
нерабочие: `AKIAIOSFODNN7EXAMPLE` — документированный пример-заглушка AWS,
`ghp_…` — криптографическая фикстура (не настоящий токен).
"""


class ClientConfig:
    """Конфигурация клиента с зашитыми креденшлами (нарушение)."""

    # Нарушение 1: литеральный AWS Access Key ID.
    AWS_ACCESS_KEY_ID = "AKIAIOSFODNN7EXAMPLE"

    # Нарушение 2: литеральный GitHub-токен.
    GITHUB_TOKEN = "ghp_16C7e42F292c6912E7710c838347Ae178B4a"

    def __init__(self, base_url: str) -> None:
        self.base_url = base_url

    def auth_headers(self) -> dict:
        """Заголовки с зашитым токеном (секрет утекает вместе с кодом)."""
        return {
            "X-Aws-Key": self.AWS_ACCESS_KEY_ID,
            "Authorization": f"token {self.GITHUB_TOKEN}",
        }

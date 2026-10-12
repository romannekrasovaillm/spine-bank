# LiteLLM Proxy: пример конфигурации

Синтаксис меняется между версиями — сверяйте с docs.litellm.ai для своей версии.

```yaml
model_list:
  - model_name: chat-default                 # алиас для клиентов
    litellm_params:
      model: openai/qwen3-32b                # self-hosted vLLM (OpenAI-совместимый)
      api_base: http://vllm-qwen:8000/v1
      api_key: os.environ/VLLM_KEY
  - model_name: chat-default                 # вторая реплика того же алиаса — балансировка
    litellm_params:
      model: openai/qwen3-32b
      api_base: http://vllm-qwen-2:8000/v1
      api_key: os.environ/VLLM_KEY
  - model_name: frontier
    litellm_params:
      model: anthropic/MODEL_ID              # внешний провайдер; ID модели — из документации провайдера
      api_key: os.environ/ANTHROPIC_API_KEY

router_settings:
  routing_strategy: least-busy               # или simple-shuffle, latency-based-routing, usage-based-routing
  num_retries: 2
  timeout: 120
  fallbacks: [{"chat-default": ["frontier"]}]

litellm_settings:
  cache: true
  cache_params: { type: redis, host: redis, port: 6379 }
  callbacks: ["otel"]                        # трассы в OTel Collector

general_settings:
  master_key: os.environ/LITELLM_MASTER_KEY
  database_url: os.environ/DATABASE_URL      # Postgres: ключи, бюджеты, учёт
```
Виртуальные ключи с бюджетом и лимитами (API администрирования):
```bash
curl -X POST http://gateway:4000/key/generate -H "Authorization: Bearer $LITELLM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{"team_id": "team-risk", "models": ["chat-default"], "max_budget": 500, "budget_duration": "30d", "tpm_limit": 200000, "rpm_limit": 300, "metadata": {"project": "agents"}}'
```
TUI-агенты подключаются к шлюзу через переменные окружения базового URL и
ключа (например, для Claude Code — `ANTHROPIC_BASE_URL` на шлюз с поддержкой
формата Anthropic; для агентов на OpenAI-формате — `OPENAI_BASE_URL`).
Проверьте, поддерживает ли шлюз нужный формат API агента.

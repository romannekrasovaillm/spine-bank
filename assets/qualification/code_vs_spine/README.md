# Набор квалификации судьи: `code_vs_spine`

Открытый формат (E6.1): каталог — это репозиторий в миниатюре.

```
ARCHITECTURE-SPINE.md   инварианты набора
cases.yaml              истина по каждому случаю (класс дефекта, вердикт)
code/<имя>.py           субъект: файл кода
```

`cases.yaml`:

```yaml
cases:
  - file: code/ignored-key-01.py
    class: ignored_key      # класс дефекта или clean
    truth: defective        # defective | clean
    note: "ключ принят и не используется"
```

Классы дефектов: `ignored_key` (AD-1), `no_return` (AD-1), `float_money` (AD-2),
`comment_only` (AD-3), `flag_bypass` (AD-4). Случаи с `class: clean` —
корректные реализации: судья обязан их не обвинять.

Прогон: `arch-be rubric qualify <рубрика> --set <каталог> [--model <имя>]`.
Отчёт квалификации ложится в `<repo>/reports/qualification/` и несёт версию
модели, хэш набора и метрики по классам. Набор расширяется: новый случай —
новый файл кода и строка в `cases.yaml`.

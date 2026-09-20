import java.util.HashMap;
import java.util.Map;

/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
 */
public class Subject {

    private final Fakes.FakePlatform platform;
    private final Map<String, Fakes.Answer> answers = new HashMap<>();

    public Subject(Fakes.FakePlatform platform) {
        this.platform = platform;
    }

    /**
     * Обработать поручение.
     *
     * <p>Свойство: одна операция на ключ — повторная доставка возвращает
     * сохранённый ответ и не создаёт второй эффект.
     */
    public Fakes.Answer process(String key, long amount) {
        if (answers.containsKey(key)) {
            return answers.get(key);
        }
        Fakes.Answer answer = platform.send(key, amount);
        answers.put(key, answer);
        return answer;
    }
}

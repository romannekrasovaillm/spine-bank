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

    public static final String UNKNOWN = "UNKNOWN";
    public static final String SUCCESS = "SUCCESS";
    public static final String FAILED = "FAILED";

    private final Fakes.FakeExternalSystem platform;
    private final Map<String, String> states = new HashMap<>();

    public Subject(Fakes.FakeExternalSystem platform) {
        this.platform = platform;
    }

    /**
     * Отправить поручение.
     *
     * <p>Свойство: таймаут оставляет операцию в состоянии UNKNOWN, а повторный
     * вызов во внешнюю систему не уходит — отправка ровно одна.
     */
    public String submit(String key, long amount) {
        if (states.containsKey(key)) {
            return states.get(key);
        }
        try {
            platform.send(key, amount);
            states.put(key, SUCCESS);
        } catch (IllegalStateException timeout) {
            states.put(key, UNKNOWN);
        } catch (IllegalArgumentException rejected) {
            states.put(key, FAILED);
        }
        return states.get(key);
    }

    /**
     * Запросить статус.
     *
     * <p>Разрешён после UNKNOWN; терминальный статус закрывает операцию.
     */
    public String queryStatus(String key) {
        String state = states.get(key);
        if (!UNKNOWN.equals(state)) {
            return state;
        }
        Fakes.Status answer = platform.queryStatus(key);
        states.put(key, "done".equals(answer.status()) ? SUCCESS : FAILED);
        return states.get(key);
    }

    /** Текущее состояние операции ({@code null} — операции не было). */
    public String stateOf(String key) {
        return states.get(key);
    }
}

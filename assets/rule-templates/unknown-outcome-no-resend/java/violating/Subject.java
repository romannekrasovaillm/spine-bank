import java.util.HashMap;
import java.util.Map;

/**
 * Нарушающая реализация: одно засеянное нарушение — повторная отправка.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * {@code submit} не помнит, что поручение уже ушло, и при неопределённом
 * исходе отправляет во внешнюю систему ещё раз (второй эффект возможен).
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

    /** Отправить поручение; при неопределённом исходе уходит ещё одна отправка. */
    public String submit(String key, long amount) {
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

    /** Запросить статус; терминальный статус закрывает операцию. */
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

import java.util.ArrayList;
import java.util.List;

/**
 * Фейк внешней системы с управляемым поведением и счётчиками вызовов.
 *
 * <p>Фейк считает то, что проверяет инвариант: сколько раз к системе
 * обратились ({@link FakePlatform#calls()}) и какие эффекты применили
 * ({@link FakePlatform#effectKeys()}). Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Ответ внешней системы на принятое поручение. */
    public record Answer(String key, long amount, String status) {
    }

    /** Внешняя система: считает обращения и применённые эффекты. */
    public static final class FakePlatform {

        private final List<String> effectKeys = new ArrayList<>();
        private int calls;
        private String behaviour = "ok";

        /** Режим «нет ответа»: обращение доходит, эффекта нет. */
        public void setTimeout() {
            behaviour = "timeout";
        }

        /** Режим «отказ»: обращение доходит, эффекта нет. */
        public void setReject() {
            behaviour = "reject";
        }

        /**
         * Принять поручение. Повторный вызов с тем же ключом — второй эффект.
         */
        public Answer send(String key, long amount) {
            calls++;
            if ("timeout".equals(behaviour)) {
                throw new IllegalStateException("внешняя система не ответила");
            }
            if ("reject".equals(behaviour)) {
                throw new IllegalArgumentException("внешняя система отклонила поручение");
            }
            effectKeys.add(key);
            return new Answer(key, amount, "done");
        }

        /** Число обращений к системе (включая неуспешные по таймауту). */
        public int calls() {
            return calls;
        }

        /** Ключи применённых эффектов, в порядке применения. */
        public List<String> effectKeys() {
            return List.copyOf(effectKeys);
        }

        /** Сколько эффектов применено по ключу. */
        public long effectsFor(String key) {
            return effectKeys.stream().filter(key::equals).count();
        }
    }
}

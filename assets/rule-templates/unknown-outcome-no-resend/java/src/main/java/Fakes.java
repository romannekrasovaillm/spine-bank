import java.util.ArrayList;
import java.util.List;

/**
 * Фейк внешней системы с раздельными счётчиками отправок и запросов статуса.
 *
 * <p>Фейк считает то, что проверяет инвариант: сколько раз поручение отправили
 * ({@link FakeExternalSystem#sends()}) и сколько раз спросили статус
 * ({@link FakeExternalSystem#statusQueries()}). Раздельные счётчики нужны
 * потому, что после неопределённого исхода повторная отправка запрещена,
 * а запрос статуса — разрешён. Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Ответ внешней системы на принятое поручение. */
    public record Outcome(String key, long amount, String status) {
    }

    /** Ответ внешней системы на запрос статуса. */
    public record Status(String key, String status) {
    }

    /** Внешняя система: отправки и запросы статуса считаются раздельно. */
    public static final class FakeExternalSystem {

        private final List<String> effectKeys = new ArrayList<>();
        private int sends;
        private int statusQueries;
        private String behaviour = "ok";
        private String status = "done";

        /** Режим «ответ получен»: поручение принято, эффект применён. */
        public void setOk() {
            behaviour = "ok";
        }

        /** Режим «нет ответа»: поручение могло дойти, ответа нет. */
        public void setTimeout() {
            behaviour = "timeout";
        }

        /** Режим «отказ»: поручение отклонено, эффекта нет. */
        public void setReject() {
            behaviour = "reject";
        }

        /** Терминальный статус, который система назовёт на запрос статуса. */
        public void setStatus(String status) {
            this.status = status;
        }

        /** Отправить поручение. Таймаут — ответа нет, но поручение могло дойти. */
        public Outcome send(String key, long amount) {
            sends++;
            if ("timeout".equals(behaviour)) {
                throw new IllegalStateException("внешняя система не ответила");
            }
            if ("reject".equals(behaviour)) {
                throw new IllegalArgumentException("внешняя система отклонила поручение");
            }
            effectKeys.add(key);
            return new Outcome(key, amount, "done");
        }

        /** Назвать терминальный статус уже отправленного поручения. */
        public Status queryStatus(String key) {
            statusQueries++;
            return new Status(key, status);
        }

        /** Число отправок поручения во внешнюю систему. */
        public int sends() {
            return sends;
        }

        /** Число запросов статуса во внешнюю систему. */
        public int statusQueries() {
            return statusQueries;
        }

        /** Ключи применённых эффектов, в порядке применения. */
        public List<String> effectKeys() {
            return List.copyOf(effectKeys);
        }
    }
}

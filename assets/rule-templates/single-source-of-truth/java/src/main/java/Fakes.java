import java.util.ArrayList;
import java.util.List;

/**
 * Фейк источника истины: история значений, версия и управляемая задержка ответа.
 *
 * <p>Фейк считает то, что проверяет инвариант: сколько раз источник читали
 * ({@link FakeSource#pulls()}) и какая у него сейчас версия
 * ({@link FakeSource#currentVersion()}). Задержка сдвигает выдаваемое значение
 * назад по истории — так появляется устаревший ответ, который нельзя выдавать
 * за актуальный. Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Ответ источника истины на чтение: значение, версия и признак устаревания. */
    public record Snapshot(int value, int version, boolean stale) {
    }

    /** Подтверждение записи: значение и его версия в источнике. */
    public record Ack(int value, int version) {
    }

    /** Источник истины: история значений, версия и задержка ответа. */
    public static final class FakeSource {

        private final List<Integer> history = new ArrayList<>();
        private int delayTicks;
        private int pulls;

        /** Записать значение; версия источника растёт на единицу. */
        public Ack write(int value) {
            history.add(value);
            return new Ack(value, history.size());
        }

        /** Имитировать задержку: отвечать на {@code ticks} версий назад. */
        public void delayAnswer(int ticks) {
            this.delayTicks = Math.max(0, ticks);
        }

        /** Прочитать источник. При задержке ответ приходит устаревшим. */
        public Snapshot pull() {
            pulls++;
            if (history.isEmpty()) {
                return new Snapshot(0, 0, false);
            }
            int version = Math.max(1, history.size() - delayTicks);
            return new Snapshot(history.get(version - 1), version, version < history.size());
        }

        /** Версия источника истины прямо сейчас. */
        public int currentVersion() {
            return history.size();
        }

        /** Значение, записанное в эту версию, — канон для сверки с проекцией. */
        public int valueAt(int version) {
            return history.get(version - 1);
        }

        /** Сколько раз читали источник. */
        public int pulls() {
            return pulls;
        }
    }
}

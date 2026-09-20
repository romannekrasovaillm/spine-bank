import java.util.ArrayList;
import java.util.List;

/**
 * Фейки саги: внешняя система и лимитный резерв со счётчиками.
 *
 * <p>Фейки считают то, что проверяет инвариант: сколько отправок ушло во
 * внешнюю систему ({@link FakeExternalSystem#sends()}), сколько раз резерв
 * удержали ({@link FakeLimitReserve#holds()}), сняли
 * ({@link FakeLimitReserve#releases()}) и превратили в списание
 * ({@link FakeLimitReserve#captures()}). Снятие резерва и списание — разные
 * операции: инвариант именно в том, что при отказе резерв снимается, при
 * неопределённом исходе удерживается, а при успехе превращается в списание
 * ровно один раз. Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Ответ внешней системы на принятое поручение. */
    public record Effect(String key, long amount, String status) {
    }

    /** Внешняя система: считает отправки и применённые эффекты. */
    public static final class FakeExternalSystem {

        private final List<String> effectKeys = new ArrayList<>();
        private int sends;
        private String behaviour = "ok";

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

        /** Отправить поручение. Таймаут — ответа нет, но поручение могло дойти. */
        public Effect send(String key, long amount) {
            sends++;
            if ("timeout".equals(behaviour)) {
                throw new IllegalStateException("внешняя система не ответила");
            }
            if ("reject".equals(behaviour)) {
                throw new IllegalArgumentException("внешняя система отклонила поручение");
            }
            effectKeys.add(key);
            return new Effect(key, amount, "done");
        }

        /** Число отправок поручения во внешнюю систему. */
        public int sends() {
            return sends;
        }

        /** Ключи применённых эффектов, в порядке применения. */
        public List<String> effectKeys() {
            return List.copyOf(effectKeys);
        }
    }

    /** Лимитный резерв: удержание, снятие и списание — разные операции со счётчиками. */
    public static final class FakeLimitReserve {

        private final long limit;
        private long held;
        private long captured;
        private int holds;
        private int releases;
        private int captures;

        public FakeLimitReserve(long limit) {
            this.limit = limit;
        }

        /** Удержать сумму под операцию. */
        public void hold(String key, long amount) {
            holds++;
            if (amount > limit - held) {
                throw new IllegalArgumentException("лимит исчерпан");
            }
            held += amount;
        }

        /** Снять резерв: операция не состоится, лимит возвращается клиенту. */
        public void release(String key, long amount) {
            releases++;
            held -= amount;
        }

        /** Превратить резерв в списание: операция состоялась ровно на эту сумму. */
        public void capture(String key, long amount) {
            captures++;
            if (amount > held) {
                throw new IllegalArgumentException("резерв меньше списания");
            }
            held -= amount;
            captured += amount;
        }

        /** Сколько удержано прямо сейчас. */
        public long held() {
            return held;
        }

        /** Сколько всего списано. */
        public long captured() {
            return captured;
        }

        /** Сколько раз резерв удерживали. */
        public int holds() {
            return holds;
        }

        /** Сколько раз резерв снимали. */
        public int releases() {
            return releases;
        }

        /** Сколько раз резерв превращали в списание. */
        public int captures() {
            return captures;
        }
    }
}

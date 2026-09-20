import java.util.ArrayList;
import java.util.List;

/**
 * Фейк журнала: только append и read — update и delete намеренно отсутствуют.
 *
 * <p>Фейк считает то, что проверяет инвариант: сколько записей добавлено
 * ({@link FakeJournal#appends()}) и что сейчас лежит в журнале
 * ({@link FakeJournal#read()}). Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /**
     * Запись журнала: неизменяема по построению.
     *
     * @param reverses id исходной записи для компенсирующей; {@code null} — не компенсация
     */
    public record Entry(int id, String kind, long amount, Integer reverses) {
    }

    /** Журнал операций: записи только добавляются, id выдаётся по порядку. */
    public static final class FakeJournal {

        private final List<Entry> entries = new ArrayList<>();
        private int appends;

        /** Добавить запись; id растёт по порядку добавления. */
        public Entry append(String kind, long amount, Integer reverses) {
            appends++;
            Entry entry = new Entry(entries.size() + 1, kind, amount, reverses);
            entries.add(entry);
            return entry;
        }

        /**
         * Записи журнала в порядке добавления.
         *
         * <p>Фейк отдаёт внутренний список как есть — это осознанное упрощение:
         * неизменяемость журнала обеспечивает вызывающий, и именно её проверяет
         * инвариант.
         */
        public List<Entry> read() {
            return entries;
        }

        /** Сколько записей добавлено. */
        public int appends() {
            return appends;
        }
    }
}

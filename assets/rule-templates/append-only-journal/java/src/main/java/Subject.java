import java.util.List;

/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
 */
public class Subject {

    private final Fakes.FakeJournal journal;

    public Subject(Fakes.FakeJournal journal) {
        this.journal = journal;
    }

    /** Добавить запись об операции. */
    public Fakes.Entry post(long amount) {
        return journal.append("post", amount, null);
    }

    /**
     * Исправить запись компенсирующей записью.
     *
     * <p>Свойство: исходная запись неизменяема — исправление приходит отдельной
     * записью со ссылкой на неё.
     */
    public Fakes.Entry correct(int entryId, long amount) {
        requireEntry(entryId);
        return journal.append("correction", amount, entryId);
    }

    /** Записи журнала в порядке добавления. */
    public List<Fakes.Entry> entries() {
        return journal.read();
    }

    /** Проверить, что запись есть в журнале: править несуществующее нельзя. */
    private void requireEntry(int entryId) {
        for (Fakes.Entry entry : journal.read()) {
            if (entry.id() == entryId) {
                return;
            }
        }
        throw new IllegalArgumentException("записи " + entryId + " в журнале нет");
    }
}

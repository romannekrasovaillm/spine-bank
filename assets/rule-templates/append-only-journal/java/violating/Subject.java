import java.util.List;

/**
 * Нарушающая реализация: одно засеянное нарушение — исправление правит запись на месте.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * {@code correct} переписывает существующую запись вместо того, чтобы добавить
 * компенсирующую, — журнал перестаёт быть append-only, и история операции
 * теряется.
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

    /** Исправить запись, переписав её на месте. */
    public Fakes.Entry correct(int entryId, long amount) {
        int index = indexOf(entryId);
        Fakes.Entry original = journal.read().get(index);
        // Единственное засеянное нарушение: исправление меняет существующую
        // запись вместо компенсирующей — исходное значение теряется.
        Fakes.Entry edited = new Fakes.Entry(original.id(), original.kind(), amount, original.reverses());
        journal.read().set(index, edited);
        return edited;
    }

    /** Записи журнала в порядке добавления. */
    public List<Fakes.Entry> entries() {
        return journal.read();
    }

    /** Найти запись по id; отсутствие записи — ошибка. */
    private int indexOf(int entryId) {
        List<Fakes.Entry> entries = journal.read();
        for (int i = 0; i < entries.size(); i++) {
            if (entries.get(i).id() == entryId) {
                return i;
            }
        }
        throw new IllegalArgumentException("записи " + entryId + " в журнале нет");
    }
}

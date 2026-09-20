import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.List;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства журнала только на дозапись — читаются как спецификация инварианта.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Журнал только на дозапись")
class AppendOnlyJournalTest {

    @Test
    @DisplayName("запись нельзя изменить: исправление не переписывает исходную")
    void entryContentCannotBeChanged() {
        Fakes.FakeJournal journal = new Fakes.FakeJournal();
        Subject ledger = new Subject(journal);

        Fakes.Entry original = ledger.post(100);
        ledger.correct(original.id(), 40);

        Fakes.Entry first = ledger.entries().get(0);

        assertEquals(100L, first.amount(),
                "исправление переписало существующую запись — журнал перестал быть append-only");
        assertEquals("post", first.kind(),
                "тип исходной записи изменился: запись правят на месте, а не дополняют");
    }

    @Test
    @DisplayName("исправление — компенсирующая запись со ссылкой на исходную")
    void correctionIsACompensatingEntryLinkingTheOriginal() {
        Fakes.FakeJournal journal = new Fakes.FakeJournal();
        Subject ledger = new Subject(journal);

        Fakes.Entry original = ledger.post(100);
        ledger.correct(original.id(), 40);

        List<Fakes.Entry> entries = ledger.entries();
        Fakes.Entry last = entries.get(entries.size() - 1);

        assertEquals("correction", last.kind(),
                "исправление не породило компенсирующей записи — история операции потеряна");
        assertEquals(Integer.valueOf(original.id()), last.reverses(),
                "компенсирующая запись не ссылается на исходную — связь исправления потеряна");
        assertEquals(2, entries.size(),
                "журнал не вырос на одну запись: исправление обязано быть отдельной записью");
    }

    @Test
    @DisplayName("у журнала нет операции правки или удаления записи")
    void journalHasNoUpdateOrDeleteOperation() {
        List<String> forbidden = List.of("update", "delete", "remove", "rewrite");
        List<String> found = new ArrayList<>();

        for (Method method : Fakes.FakeJournal.class.getDeclaredMethods()) {
            if (forbidden.contains(method.getName())) {
                found.add(method.getName());
            }
        }

        assertTrue(found.isEmpty(),
                "у журнала появилась операция " + found + " — запись можно изменить или удалить");
    }

    @Test
    @DisplayName("порядок записей сохраняется")
    void orderOfRecordsIsPreserved() {
        Fakes.FakeJournal journal = new Fakes.FakeJournal();
        Subject ledger = new Subject(journal);

        Fakes.Entry first = ledger.post(100);
        ledger.post(50);
        ledger.correct(first.id(), 40);
        ledger.post(70);

        List<Fakes.Entry> entries = ledger.entries();
        List<Integer> ids = new ArrayList<>();
        List<String> kinds = new ArrayList<>();
        for (Fakes.Entry entry : entries) {
            ids.add(entry.id());
            kinds.add(entry.kind());
        }

        assertEquals(List.of(1, 2, 3, 4), ids,
                "порядок записей нарушен: история операций не восстанавливается по журналу");
        assertEquals(List.of("post", "post", "correction", "post"), kinds,
                "последовательность записей изменилась: компенсирующая запись потерялась или встала не в конец");
    }
}

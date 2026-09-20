import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства единственного источника истины — читаются как спецификация инварианта.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Единственный источник истины")
class SingleSourceOfTruthTest {

    @Test
    @DisplayName("локальное значение не меняется само по себе")
    void localValueDoesNotChangeByItself() {
        Fakes.FakeSource source = new Fakes.FakeSource();
        Subject projection = new Subject(source);

        projection.submit(100);
        int pullsBefore = source.pulls();

        assertEquals(100, projection.read().value(),
                "локальное значение изменилось само по себе — проекция додумала состояние");
        assertEquals(100, projection.read().value(),
                "локальное значение изменилось само по себе — проекция додумала состояние");
        assertEquals(pullsBefore, source.pulls(),
                "чтение проекции сходило в источник: показанное значение взято не из ответа");
    }

    @Test
    @DisplayName("показанное значение равно последнему ответу источника")
    void shownValueEqualsLastSourceAnswer() {
        Fakes.FakeSource source = new Fakes.FakeSource();
        Subject projection = new Subject(source);

        source.write(50);
        Fakes.Snapshot first = projection.refresh();

        assertEquals(50, first.value(),
                "проекция показала не то значение, которое вернул источник истины");
        assertEquals(source.currentVersion(), first.version(),
                "версия проекции разошлась с версией источника истины — это рассинхрон");

        source.write(70);
        Fakes.Snapshot second = projection.refresh();

        assertEquals(70, second.value(),
                "после нового ответа источника проекция показала старое значение");
        assertEquals(source.valueAt(second.version()), second.value(),
                "показанное значение не совпадает с записанным в источнике для этой версии");
    }

    @Test
    @DisplayName("устаревший ответ не выдаётся за актуальный")
    void outdatedAnswerIsNotPresentedAsActual() {
        Fakes.FakeSource source = new Fakes.FakeSource();
        Subject projection = new Subject(source);

        source.write(10);
        source.write(20);
        source.delayAnswer(1);

        Fakes.Snapshot shown = projection.refresh();

        assertEquals(1, shown.version(),
                "версия устаревшего ответа потеряна — проекция выдала старые данные за текущие");
        assertTrue(shown.stale(),
                "устаревшая проекция выдана как актуальная: у значения нет признака устаревания");
    }

    @Test
    @DisplayName("свежий ответ снимает признак устаревания")
    void freshAnswerClearsTheStaleMark() {
        Fakes.FakeSource source = new Fakes.FakeSource();
        Subject projection = new Subject(source);

        source.write(10);
        source.write(20);
        source.delayAnswer(1);
        projection.refresh();

        source.delayAnswer(0);
        Fakes.Snapshot shown = projection.refresh();

        assertFalse(shown.stale(),
                "проекция осталась помеченной устаревшей после свежего ответа источника");
        assertEquals(20, shown.value(),
                "после снятия задержки проекция показала не текущее значение источника");
    }
}

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства неопределённого исхода — читаются как спецификация инварианта.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Неопределённый исход: повторной отправки нет")
class UnknownOutcomeNoResendTest {

    @Test
    @DisplayName("таймаут оставляет операцию в состоянии UNKNOWN")
    void timeoutPutsOperationIntoUnknownState() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        platform.setTimeout();
        Subject service = new Subject(platform);

        String state = service.submit("op-1", 100);

        assertEquals(Subject.UNKNOWN, state,
                "таймаут внешней системы должен оставлять операцию в состоянии UNKNOWN");
        assertNotEquals(Subject.SUCCESS, state,
                "операция объявлена успешной, хотя ответа от внешней системы не было");
        assertNotEquals(Subject.FAILED, state,
                "операция объявлена неуспешной, хотя исход неизвестен: поручение могло дойти");
        assertEquals(Subject.UNKNOWN, service.stateOf("op-1"),
                "состояние операции не сохранилось — вызывающий не может отличить UNKNOWN от успеха");
        assertEquals(1, platform.sends(), "отправка при таймауте должна быть ровно одна");
    }

    @Test
    @DisplayName("при неопределённом исходе повторной отправки нет")
    void unknownOutcomeDoesNotResend() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        platform.setTimeout();
        Subject service = new Subject(platform);

        service.submit("op-2", 250);
        service.submit("op-2", 250);

        assertEquals(1, platform.sends(),
                "при неопределённом исходе отправка повторилась — возможен второй эффект "
                        + "у внешней системы");
        assertEquals(Subject.UNKNOWN, service.stateOf("op-2"),
                "повторный вызов перевёл операцию из UNKNOWN, хотя исход не выяснен");
    }

    @Test
    @DisplayName("после UNKNOWN во внешнюю систему уходит только запрос статуса")
    void afterUnknownOnlyStatusQueryReachesTheSystem() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        platform.setTimeout();
        Subject service = new Subject(platform);

        service.submit("op-3", 500);
        service.queryStatus("op-3");

        assertEquals(1, platform.statusQueries(),
                "после неопределённого исхода запрос статуса не дошёл до внешней системы");
        assertEquals(1, platform.sends(),
                "после неопределённого исхода ушла повторная отправка вместо запроса статуса");
    }

    @Test
    @DisplayName("терминальный статус закрывает операцию")
    void terminalStatusClosesTheOperation() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        platform.setTimeout();
        Subject service = new Subject(platform);

        service.submit("op-4", 100);
        assertEquals(Subject.UNKNOWN, service.stateOf("op-4"),
                "перед сверкой операция должна быть в UNKNOWN");

        platform.setStatus("done"); // внешняя система досчитала операцию
        assertEquals(Subject.SUCCESS, service.queryStatus("op-4"),
                "терминальный статус не закрыл операцию успехом");
        assertNotEquals(Subject.UNKNOWN, service.stateOf("op-4"),
                "операция осталась в UNKNOWN после терминального статуса — сверка не закрывает исход");
    }
}

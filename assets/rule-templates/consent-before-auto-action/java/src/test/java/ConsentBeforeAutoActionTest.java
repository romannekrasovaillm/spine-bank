import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства «согласие до автодействия» — читаются как спецификация инварианта.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Согласие до автодействия")
class ConsentBeforeAutoActionTest {

    private static final String CLIENT = "client-17";
    private static final String SCOPE = "autopayments";

    private final Fakes.ConsentRegistry consents = new Fakes.ConsentRegistry();
    private final Fakes.FakeActionLog actions = new Fakes.FakeActionLog();
    private final Fakes.FakeOperations operations = new Fakes.FakeOperations();

    private Subject service() {
        return new Subject(consents, actions, operations);
    }

    @Test
    @DisplayName("без записи о согласии автодействие не создаётся")
    void actionWithoutConsentIsNotCreated() {
        Fakes.Action result = service().trigger(CLIENT, SCOPE, "autopayment", 5000L);

        assertNull(result, "без записи о согласии автодействие всё равно создано");
        assertEquals(0, actions.count(),
                "без записи о согласии счётчик созданных автодействий вырос — "
                        + "автодействие обязано быть остановлено до создания");
    }

    @Test
    @DisplayName("отзыв согласия останавливает следующее срабатывание")
    void revokedConsentStopsTheNextTrigger() {
        consents.grant(CLIENT, SCOPE, "consent-1");
        Subject service = service();

        service.trigger(CLIENT, SCOPE, "autopayment", 5000L);
        consents.revoke(CLIENT, SCOPE);
        Fakes.Action second = service.trigger(CLIENT, SCOPE, "autopayment", 5000L);

        assertNull(second, "после отзыва согласия автодействие создано повторно");
        assertEquals(1, actions.count(),
                "после отзыва согласия создано ещё одно автодействие (всего "
                        + actions.count() + ") — отзыв обязан останавливать следующее срабатывание");
    }

    @Test
    @DisplayName("при незавершённой операции новое автодействие не стартует")
    void openOperationBlocksNewAction() {
        consents.grant(CLIENT, SCOPE, "consent-2");
        operations.open(CLIENT, "operation-1");

        Fakes.Action result = service().trigger(CLIENT, SCOPE, "autopayment", 5000L);

        assertNull(result, "при незавершённой операции автодействие всё равно стартовало");
        assertEquals(0, actions.count(),
                "при незавершённой операции клиента счётчик автодействий вырос — "
                        + "новое автодействие не должно стартовать поверх незакрытой операции");
    }

    @Test
    @DisplayName("действующее согласие и завершённая операция дают ровно одно автодействие")
    void activeConsentAndClosedOperationCreateExactlyOneAction() {
        consents.grant(CLIENT, SCOPE, "consent-3");
        operations.open(CLIENT, "operation-2");
        operations.close(CLIENT);

        Fakes.Action result = service().trigger(CLIENT, SCOPE, "autopayment", 5000L);

        assertNotNull(result,
                "при действующем согласии и завершённой операции автодействие не создано — "
                        + "проверка согласия не должна глушить законные срабатывания");
        assertEquals(1, actions.count(),
                "ожидалось ровно одно автодействие, создано " + actions.count());
    }
}

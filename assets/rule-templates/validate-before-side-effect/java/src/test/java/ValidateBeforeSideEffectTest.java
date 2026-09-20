import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.util.List;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства «проверка раньше побочного эффекта» — читаются как спецификация.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Проверка раньше побочного эффекта")
class ValidateBeforeSideEffectTest {

    private static final String BLOCKED = "recipient-blocked";
    private static final String CLEAN = "recipient-clean";

    private final Fakes.FakeStopList stopList = new Fakes.FakeStopList();
    private final Fakes.FakeLimit limit = new Fakes.FakeLimit(10_000L);
    private final Fakes.FakeExternalSystem external = new Fakes.FakeExternalSystem();

    private Subject service() {
        stopList.block(BLOCKED);
        return new Subject(stopList, limit, external);
    }

    @Test
    @DisplayName("получатель из стоп-листа: обращений к внешней системе ноль")
    void stopListRecipientMakesNoCall() {
        Subject service = service();

        assertThrows(IllegalArgumentException.class, () -> service.transfer(BLOCKED, 1000L));

        assertEquals(0, external.calls(),
                "при получателе из стоп-листа внешняя система получила обращение — "
                        + "проверка обязана пройти до побочного эффекта");
        assertEquals(List.of(), external.moves(),
                "внешняя система применила перевод по получателю из стоп-листа: " + external.moves());
        assertEquals(10_000L, limit.available(),
                "отклонённый перевод израсходовал лимит — отказ не должен стоить клиенту лимита");
    }

    @Test
    @DisplayName("сумма сверх лимита: обращений к внешней системе ноль")
    void overLimitTransferMakesNoCall() {
        Fakes.FakeLimit smallLimit = new Fakes.FakeLimit(500L);
        Fakes.FakeExternalSystem externalSystem = new Fakes.FakeExternalSystem();
        Fakes.FakeStopList stops = new Fakes.FakeStopList();
        Subject service = new Subject(stops, smallLimit, externalSystem);

        assertThrows(IllegalArgumentException.class, () -> service.transfer(CLEAN, 1500L));

        assertEquals(0, externalSystem.calls(),
                "при превышении лимита внешняя система получила обращение — "
                        + "лимит обязан проверяться до списания и до обращения");
        assertEquals(500L, smallLimit.available(),
                "отклонённый по лимиту перевод всё равно израсходовал лимит");
    }

    @Test
    @DisplayName("успешный перевод: ровно одно обращение и израсходованный лимит")
    void successfulTransferMakesExactlyOneCall() {
        Subject service = service();

        Fakes.Answer answer = service.transfer(CLEAN, 1500L);

        assertEquals(1, external.calls(),
                "успешный перевод обратился к внешней системе " + external.calls()
                        + " раз(а), ожидалось одно");
        assertEquals(List.of(new Fakes.Move(CLEAN, 1500L)), external.moves(),
                "внешняя система применила не тот перевод: " + external.moves());
        assertEquals(8_500L, limit.available(),
                "после успешного перевода лимит равен " + limit.available()
                        + ", ожидалось 8500 — лимит расходуется ровно на сумму перевода");
        assertEquals("done", answer.status(), "ответ внешней системы не проброшен вызывающему");
    }

    @Test
    @DisplayName("оба нарушения сразу: порядок проверок не меняет исход — обращений ноль")
    void bothViolationsTogetherMakeNoCall() {
        Fakes.FakeLimit smallLimit = new Fakes.FakeLimit(500L);
        Fakes.FakeExternalSystem externalSystem = new Fakes.FakeExternalSystem();
        Fakes.FakeStopList stops = new Fakes.FakeStopList();
        stops.block(BLOCKED);
        Subject service = new Subject(stops, smallLimit, externalSystem);

        assertThrows(IllegalArgumentException.class, () -> service.transfer(BLOCKED, 1500L));

        assertEquals(0, externalSystem.calls(),
                "получатель в стоп-листе и сумма сверх лимита, но обращение к внешней системе "
                        + "всё равно ушло — проверки не должны зависеть от порядка и от того, "
                        + "какая из них сработала первой");
        assertEquals(500L, smallLimit.available(), "отклонённый перевод израсходовал лимит");
    }
}

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства компенсации резерва — читаются как спецификация инварианта.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Компенсация резерва в саге")
class SagaReserveCompensationTest {

    @Test
    @DisplayName("отказ внешней системы снимает резерв и не даёт списания")
    void rejectReleasesTheReserveWithoutCapture() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        platform.setReject();
        Fakes.FakeLimitReserve reserve = new Fakes.FakeLimitReserve(1000L);
        Subject saga = new Subject(platform, reserve);

        String state = saga.pay("op-1", 300L);

        assertEquals(Subject.FAILED, state,
                "отказ внешней системы должен завершать операцию отказом");
        assertEquals(1, reserve.releases(),
                "при отказе внешней системы компенсация не выполнена — резерв не снят");
        assertEquals(0L, reserve.held(),
                "при отказе внешней системы резерв остался удержанным — лимит клиента утёк");
        assertEquals(0, reserve.captures(), "при отказе внешней системы прошло списание");
        assertEquals(0L, reserve.captured(), "при отказе внешней системы списана сумма");
    }

    @Test
    @DisplayName("неопределённый исход удерживает резерв")
    void unknownOutcomeKeepsTheReserveHeld() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        platform.setTimeout();
        Fakes.FakeLimitReserve reserve = new Fakes.FakeLimitReserve(1000L);
        Subject saga = new Subject(platform, reserve);

        String state = saga.pay("op-2", 300L);

        assertEquals(Subject.UNKNOWN, state,
                "таймаут внешней системы должен давать неопределённый исход");
        assertEquals(300L, reserve.held(),
                "при неопределённом исходе резерв снят — исход ещё не решён, сумма должна удерживаться");
        assertEquals(0, reserve.releases(),
                "при неопределённом исходе резерв снят — исход ещё не решён");
        assertEquals(0, reserve.captures(),
                "при неопределённом исходе резерв превращён в списание — исход ещё не решён");
    }

    @Test
    @DisplayName("успех превращает резерв в списание ровно один раз")
    void successCapturesTheReserveExactlyOnce() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        Fakes.FakeLimitReserve reserve = new Fakes.FakeLimitReserve(1000L);
        Subject saga = new Subject(platform, reserve);

        String state = saga.pay("op-3", 300L);

        assertEquals(Subject.SUCCESS, state,
                "принятое внешней системой поручение должно завершаться успехом");
        assertEquals(300L, reserve.captured(), "при успехе резерв не превращён в списание");
        assertEquals(1, reserve.captures(),
                "списание прошло не ровно один раз — резерв превращён в списание повторно");
        assertEquals(0L, reserve.held(),
                "после списания резерв остался удержанным — двойное удержание");
    }

    @Test
    @DisplayName("снятый при отказе резерв возвращает лимит следующей операции")
    void releasedReserveIsAvailableForTheNextOperation() {
        Fakes.FakeExternalSystem platform = new Fakes.FakeExternalSystem();
        Fakes.FakeLimitReserve reserve = new Fakes.FakeLimitReserve(1000L);
        Subject saga = new Subject(platform, reserve);

        platform.setReject();
        saga.pay("op-4", 300L);

        platform.setOk();
        String state = saga.pay("op-5", 300L);

        assertEquals(Subject.SUCCESS, state,
                "следующая операция должна пройти после снятия резерва");
        assertEquals(300L, reserve.captured(),
                "списание прошло не по одной операции — сумма списана дважды");
        assertEquals(1, reserve.captures(), "списание прошло не ровно один раз");
        assertEquals(0L, reserve.held(),
                "лимит не освобождён после отказа — резерв утёк, следующая операция удержала лишнее");
    }
}

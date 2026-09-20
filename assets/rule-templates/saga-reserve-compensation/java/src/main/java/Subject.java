/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
 */
public class Subject {

    public static final String UNKNOWN = "UNKNOWN";
    public static final String SUCCESS = "SUCCESS";
    public static final String FAILED = "FAILED";

    private final Fakes.FakeExternalSystem platform;
    private final Fakes.FakeLimitReserve reserve;

    public Subject(Fakes.FakeExternalSystem platform, Fakes.FakeLimitReserve reserve) {
        this.platform = platform;
        this.reserve = reserve;
    }

    /**
     * Провести платёж через сагу: резерв, отправка, списание либо снятие.
     *
     * <p>Свойство: отказ снимает резерв и не даёт списания, неопределённый
     * исход резерв удерживает, успех превращает резерв в списание ровно раз.
     */
    public String pay(String key, long amount) {
        reserve.hold(key, amount);
        try {
            platform.send(key, amount);
        } catch (IllegalStateException timeout) {
            // исход неизвестен: резерв удерживаем, списания нет — решает сверка
            return UNKNOWN;
        } catch (IllegalArgumentException rejected) {
            reserve.release(key, amount);
            return FAILED;
        }
        reserve.capture(key, amount);
        return SUCCESS;
    }
}

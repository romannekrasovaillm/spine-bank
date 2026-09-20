/**
 * Нарушающая реализация: одно засеянное нарушение — резерв не снимается.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * при отказе внешней системы компенсация не выполняется — резерв остаётся
 * удержанным, лимит клиента утекает.
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

    /** Провести платёж; при отказе резерв остаётся удержанным. */
    public String pay(String key, long amount) {
        reserve.hold(key, amount);
        try {
            platform.send(key, amount);
        } catch (IllegalStateException timeout) {
            return UNKNOWN;
        } catch (IllegalArgumentException rejected) {
            return FAILED;
        }
        reserve.capture(key, amount);
        return SUCCESS;
    }
}

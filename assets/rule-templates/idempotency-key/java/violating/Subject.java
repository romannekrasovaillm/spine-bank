/**
 * Нарушающая реализация: одно засеянное нарушение — дедупликации нет.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * повторная доставка идёт во внешнюю систему ещё раз (второй эффект).
 */
public class Subject {

    private final Fakes.FakePlatform platform;

    public Subject(Fakes.FakePlatform platform) {
        this.platform = platform;
    }

    /** Обработать поручение; повторная доставка создаёт второй эффект. */
    public Fakes.Answer process(String key, long amount) {
        return platform.send(key, amount);
    }
}

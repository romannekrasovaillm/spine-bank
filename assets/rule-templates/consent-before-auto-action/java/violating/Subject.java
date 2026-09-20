/**
 * Нарушающая реализация: одно засеянное нарушение — согласие не проверяется.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * автодействие создаётся без записи о согласии (и после его отзыва тоже).
 */
public class Subject {

    private final Fakes.ConsentRegistry consents;
    private final Fakes.FakeActionLog actions;
    private final Fakes.FakeOperations operations;

    public Subject(Fakes.ConsentRegistry consents,
                   Fakes.FakeActionLog actions,
                   Fakes.FakeOperations operations) {
        this.consents = consents;
        this.actions = actions;
        this.operations = operations;
    }

    /** Создать автодействие, не заглядывая в реестр согласий. */
    public Fakes.Action trigger(String clientId, String scope, String kind, long amount) {
        if (operations.hasOpen(clientId)) {
            return null;
        }
        return actions.create(clientId, kind, amount);
    }
}

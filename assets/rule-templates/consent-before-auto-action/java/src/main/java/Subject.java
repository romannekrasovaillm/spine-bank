/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
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

    /**
     * Создать автодействие, если это разрешено; иначе вернуть {@code null}.
     *
     * <p>Свойства: без записи о согласии автодействие не создаётся; отзыв
     * согласия останавливает следующее срабатывание; при незавершённой
     * операции клиента новое автодействие не стартует.
     */
    public Fakes.Action trigger(String clientId, String scope, String kind, long amount) {
        if (!consents.isActive(clientId, scope)) {
            return null;
        }
        if (operations.hasOpen(clientId)) {
            return null;
        }
        return actions.create(clientId, kind, amount);
    }
}

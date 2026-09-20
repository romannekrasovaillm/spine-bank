/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
 */
public class Subject {

    private final Fakes.FakeStopList stopList;
    private final Fakes.FakeLimit limit;
    private final Fakes.FakeExternalSystem external;

    public Subject(Fakes.FakeStopList stopList,
                   Fakes.FakeLimit limit,
                   Fakes.FakeExternalSystem external) {
        this.stopList = stopList;
        this.limit = limit;
        this.external = external;
    }

    /**
     * Проверить получателя и лимит, затем ровно один раз обратиться к системе.
     *
     * <p>Свойства: при провале проверки (получатель в стоп-листе, сумма сверх
     * лимита) обращений к внешней системе ноль; при успешной проверке —
     * ровно одно обращение; лимит расходуется только после исполнения.
     */
    public Fakes.Answer transfer(String recipient, long amount) {
        if (stopList.contains(recipient)) {
            throw new IllegalArgumentException("получатель в стоп-листе");
        }
        if (!limit.allows(amount)) {
            throw new IllegalArgumentException("перевод превышает доступный лимит");
        }
        Fakes.Answer answer = external.transfer(recipient, amount);
        limit.spend(amount);
        return answer;
    }
}

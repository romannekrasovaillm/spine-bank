/**
 * Нарушающая реализация: одно засеянное нарушение — проверки после эффекта.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * перевод уходит во внешнюю систему ДО проверки стоп-листа и лимита,
 * поэтому провал проверки оставляет за собой исполненное обращение.
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

    /** Перевести получателю, проверяя стоп-лист и лимит уже после обращения. */
    public Fakes.Answer transfer(String recipient, long amount) {
        Fakes.Answer answer = external.transfer(recipient, amount);
        if (stopList.contains(recipient)) {
            throw new IllegalArgumentException("получатель в стоп-листе");
        }
        if (!limit.allows(amount)) {
            throw new IllegalArgumentException("перевод превышает доступный лимит");
        }
        limit.spend(amount);
        return answer;
    }
}

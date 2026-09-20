import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Фейки внешней системы, стоп-листа и лимита.
 *
 * <p>Фейки считают то, что проверяет инвариант: сколько раз обратились
 * к внешней системе ({@link FakeExternalSystem#calls()}) и какие переводы
 * она применила ({@link FakeExternalSystem#moves()}), кого держит стоп-лист
 * ({@link FakeStopList#contains}) и сколько клиенту ещё можно перевести
 * ({@link FakeLimit#available()}). Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Применённый перевод. */
    public record Move(String recipient, long amount) {
    }

    /** Ответ внешней системы на принятый перевод. */
    public record Answer(String recipient, long amount, String status) {
    }

    /** Внешняя система: считает обращения и применённые переводы. */
    public static final class FakeExternalSystem {

        private final List<Move> moves = new ArrayList<>();
        private int calls;

        /** Принять перевод к исполнению. */
        public Answer transfer(String recipient, long amount) {
            calls++;
            moves.add(new Move(recipient, amount));
            return new Answer(recipient, amount, "done");
        }

        /** Число обращений к системе. */
        public int calls() {
            return calls;
        }

        /** Применённые переводы, в порядке применения. */
        public List<Move> moves() {
            return List.copyOf(moves);
        }
    }

    /** Стоп-лист получателей. */
    public static final class FakeStopList {

        private final Set<String> blocked = new HashSet<>();

        /** Добавить получателя в стоп-лист. */
        public void block(String recipient) {
            blocked.add(recipient);
        }

        /** Есть ли получатель в стоп-листе. */
        public boolean contains(String recipient) {
            return blocked.contains(recipient);
        }
    }

    /** Доступный лимит клиента: сколько ещё можно перевести. */
    public static final class FakeLimit {

        private long available;

        public FakeLimit(long available) {
            this.available = available;
        }

        /** Укладывается ли сумма в доступный лимит. */
        public boolean allows(long amount) {
            return amount <= available;
        }

        /** Израсходовать лимит после успешного перевода. */
        public void spend(long amount) {
            available -= amount;
        }

        /** Доступный остаток лимита. */
        public long available() {
            return available;
        }
    }
}

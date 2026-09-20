import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Фейки реестра согласий, незавершённых операций и счётчика автодействий.
 *
 * <p>Фейки считают то, что проверяет инвариант: есть ли действующая запись
 * о согласии ({@link ConsentRegistry#isActive}), открыта ли операция клиента
 * ({@link FakeOperations#hasOpen}) и сколько автодействий создано
 * ({@link FakeActionLog#count()}). Ни сети, ни сна.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Созданное автодействие. */
    public record Action(String clientId, String kind, long amount, String status) {
    }

    /** Запись о согласии. */
    public record Consent(String consentId, String status) {

        /** Действует ли согласие. */
        public boolean active() {
            return "active".equals(status);
        }
    }

    /**
     * Реестр согласий: записи о согласии клиента на автодействие.
     *
     * <p>Запись хранит статус, а не только факт наличия: отозванное согласие
     * остаётся в реестре, но действующим не считается.
     */
    public static final class ConsentRegistry {

        private final Map<String, Consent> records = new HashMap<>();

        private static String key(String clientId, String scope) {
            return clientId + "|" + scope;
        }

        /** Записать согласие клиента в объёме {@code scope}. */
        public void grant(String clientId, String scope, String consentId) {
            records.put(key(clientId, scope), new Consent(consentId, "active"));
        }

        /** Отозвать согласие; запись остаётся со статусом {@code revoked}. */
        public void revoke(String clientId, String scope) {
            Consent record = records.get(key(clientId, scope));
            if (record != null) {
                records.put(key(clientId, scope), new Consent(record.consentId(), "revoked"));
            }
        }

        /** Есть ли действующая запись о согласии. */
        public boolean isActive(String clientId, String scope) {
            Consent record = records.get(key(clientId, scope));
            return record != null && record.active();
        }

        /** Запись о согласии или {@code null}. */
        public Consent recordFor(String clientId, String scope) {
            return records.get(key(clientId, scope));
        }
    }

    /**
     * Незавершённые операции клиента.
     *
     * <p>Пока операция клиента открыта, новое автодействие по нему не стартует.
     */
    public static final class FakeOperations {

        private final Set<String> open = new HashSet<>();

        /** Открыть операцию клиента. */
        public void open(String clientId, String operationId) {
            open.add(clientId);
        }

        /** Закрыть операцию клиента. */
        public void close(String clientId) {
            open.remove(clientId);
        }

        /** Открыта ли у клиента незавершённая операция. */
        public boolean hasOpen(String clientId) {
            return open.contains(clientId);
        }
    }

    /** Счётчик созданных автодействий. */
    public static final class FakeActionLog {

        private final List<Action> actions = new ArrayList<>();

        /** Создать автодействие и записать его в счётчик. */
        public Action create(String clientId, String kind, long amount) {
            Action action = new Action(clientId, kind, amount, "created");
            actions.add(action);
            return action;
        }

        /** Сколько автодействий создано. */
        public int count() {
            return actions.size();
        }

        /** Созданные автодействия, в порядке создания. */
        public List<Action> actions() {
            return List.copyOf(actions);
        }
    }
}

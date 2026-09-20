/**
 * Нарушающая реализация: одно засеянное нарушение — в журнал уходит номер карты.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * строка журнала пишет номер карты клиента вместо его идентификатора.
 */
public class Subject {

    private final Fakes.LogSink sink;

    public Subject(Fakes.LogSink sink) {
        this.sink = sink;
    }

    /** Провести перевод; строка журнала содержит номер карты. */
    public Fakes.TransferResult run(Fakes.Client client, long amount) {
        sink.log("transfer started client=" + client.clientId() + " card=" + client.card());
        sink.metric("transfer_total", "client=\"" + client.clientId() + "\",result=\"ok\"");
        sink.log("transfer done operation=op-" + client.clientId());
        return new Fakes.TransferResult(client.clientId(), amount, "done");
    }
}

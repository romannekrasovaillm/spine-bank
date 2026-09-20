/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
 */
public class Subject {

    private final Fakes.LogSink sink;

    public Subject(Fakes.LogSink sink) {
        this.sink = sink;
    }

    /**
     * Провести перевод и записать телеметрию по идентификаторам.
     *
     * <p>Свойство: ни в строке журнала, ни в метке метрики нет формата
     * персональных данных — телефона, номера карты, ФИО.
     */
    public Fakes.TransferResult run(Fakes.Client client, long amount) {
        sink.log("transfer started client=" + client.clientId() + " amount=" + amount);
        sink.metric("transfer_total", "client=\"" + client.clientId() + "\",result=\"ok\"");
        sink.log("transfer done operation=op-" + client.clientId());
        return new Fakes.TransferResult(client.clientId(), amount, "done");
    }
}

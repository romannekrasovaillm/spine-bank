import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства «в телеметрии нет персональных данных» — читаются как спецификация.
 *
 * <p>Тест прогоняет сценарий и сканирует ЗАХВАЧЕННЫЕ строки журнала и метки
 * метрик регулярными выражениями форматов. Тест читает реализацию из
 * {@code Subject.java}. Проверка зубов ({@code arch-be rules template verify})
 * подменяет этот файл нарушающей реализацией — тест обязан упасть.
 */
@DisplayName("Персональные данные не попадают в журналы и метрики")
class NoPiiInLogsTest {

    private static final Fakes.Client CLIENT = new Fakes.Client(
            "user-17", "+7 916 123-45-67", "4276 3800 1234 5678", "Иванов Иван Иванович");

    private Fakes.LogSink runScenario() {
        Fakes.LogSink sink = new Fakes.LogSink();
        new Subject(sink).run(CLIENT, 1500L);
        return sink;
    }

    @Test
    @DisplayName("в телеметрии нет формата номера карты")
    void cardNumberFormatNotInTelemetry() {
        Fakes.LogSink sink = runScenario();

        List<String> found = Fakes.PiiFormats.findCard(String.join("\n", sink.captured()));
        assertEquals(List.of(), found,
                "в захваченных журналах и метках метрик найден формат номера карты: " + found
                        + " — персональные данные не должны покидать сценарий");
    }

    @Test
    @DisplayName("в телеметрии нет телефонного формата")
    void phoneFormatNotInTelemetry() {
        Fakes.LogSink sink = runScenario();

        List<String> found = Fakes.PiiFormats.findPhone(String.join("\n", sink.captured()));
        assertEquals(List.of(), found,
                "в захваченных журналах и метках метрик найден телефонный формат: " + found
                        + " — телефон клиента в телеметрию не пишется");
    }

    @Test
    @DisplayName("в телеметрии нет формата ФИО")
    void fullNameFormatNotInTelemetry() {
        Fakes.LogSink sink = runScenario();

        List<String> found = Fakes.PiiFormats.findFullName(String.join("\n", sink.captured()));
        assertEquals(List.of(), found,
                "в захваченных журналах и метках метрик найден формат ФИО: " + found
                        + " — имя клиента в телеметрию не пишется");
    }

    @Test
    @DisplayName("телеметрия не пуста и содержит идентификаторы")
    void telemetryContainsIdentifiers() {
        Fakes.LogSink sink = runScenario();

        assertFalse(sink.lines().isEmpty(),
                "сценарий не записал ни одной строки журнала — проверка на отсутствие "
                        + "персональных данных прошла бы вхолостую");
        assertTrue(sink.captured().stream().anyMatch(line -> line.contains(CLIENT.clientId())),
                "в телеметрии нет идентификатора клиента — журнал должен оставаться "
                        + "пригодным для разбора, а не становиться пустым");
    }
}

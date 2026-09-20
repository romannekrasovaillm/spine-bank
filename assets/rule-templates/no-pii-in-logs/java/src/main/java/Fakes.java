import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Фейк сборщиков телеметрии — то, что проверяет инвариант.
 *
 * <p>Фейк собирает то, что уходит наружу: строки журнала
 * ({@link LogSink#lines()}) и метки метрик ({@link LogSink#metricLabels()}).
 * Ни сети, ни сна. Проверка сканирует именно эти строки после прогона
 * сценария: персональные данные в них попадать не должны.
 */
public final class Fakes {

    private Fakes() {
    }

    /** Клиент с персональными данными: сценарий обязан писать только id. */
    public record Client(String clientId, String phone, String card, String fullName) {
    }

    /** Итог прогона перевода. */
    public record TransferResult(String clientId, long amount, String status) {
    }

    /** Сборщик журнальных строк и меток метрик. */
    public static final class LogSink {

        private final List<String> lines = new ArrayList<>();
        private final List<String> metricLabels = new ArrayList<>();

        /** Записать строку журнала. */
        public void log(String message) {
            lines.add(message);
        }

        /** Записать метку метрики в виде «имя{метка}». */
        public void metric(String name, String label) {
            metricLabels.add(name + "{" + label + "}");
        }

        /** Строки журнала, в порядке записи. */
        public List<String> lines() {
            return List.copyOf(lines);
        }

        /** Метки метрик, в порядке записи. */
        public List<String> metricLabels() {
            return List.copyOf(metricLabels);
        }

        /** Всё захваченное: строки журнала и метки метрик. */
        public List<String> captured() {
            List<String> all = new ArrayList<>(lines);
            all.addAll(metricLabels);
            return List.copyOf(all);
        }
    }

    /**
     * Регулярные выражения ФОРМАТОВ персональных данных.
     *
     * <p>Проверяются форматы — последовательность цифр карты, телефонный
     * шаблон, шаблон ФИО, — а не слова «карта»/«ФИО»: слово в журнале
     * о персональных данных ничего не говорит, а формат говорит.
     */
    public static final class PiiFormats {

        public static final Pattern CARD =
                Pattern.compile("(?<!\\d)\\d{4}[ -]?\\d{4}[ -]?\\d{4}[ -]?\\d{4}(?!\\d)");

        public static final Pattern PHONE = Pattern.compile(
                "(?<!\\d)(?:\\+7|8)[\\s\\-()]*\\d{3}[\\s\\-()]*\\d{3}[\\s\\-]*\\d{2}[\\s\\-]*\\d{2}(?!\\d)");

        public static final Pattern FULL_NAME = Pattern.compile(
                "(?<![А-ЯЁа-яё])[А-ЯЁ][а-яё]+(?:\\s+[А-ЯЁ][а-яё]+){2}(?![а-яё])");

        private PiiFormats() {
        }

        /** Совпадения формата номера карты. */
        public static List<String> findCard(String text) {
            return fragments(CARD, text);
        }

        /** Совпадения телефонного формата. */
        public static List<String> findPhone(String text) {
            return fragments(PHONE, text);
        }

        /** Совпадения формата ФИО. */
        public static List<String> findFullName(String text) {
            return fragments(FULL_NAME, text);
        }

        /** Все найденные форматы одной строкой: «ФОРМАТ: фрагмент». */
        public static List<String> scan(String text) {
            List<String> found = new ArrayList<>();
            for (String fragment : findCard(text)) {
                found.add("card: " + fragment);
            }
            for (String fragment : findPhone(text)) {
                found.add("phone: " + fragment);
            }
            for (String fragment : findFullName(text)) {
                found.add("full_name: " + fragment);
            }
            return List.copyOf(found);
        }

        private static List<String> fragments(Pattern pattern, String text) {
            List<String> found = new ArrayList<>();
            Matcher matcher = pattern.matcher(text);
            while (matcher.find()) {
                found.add(matcher.group());
            }
            return List.copyOf(found);
        }
    }
}

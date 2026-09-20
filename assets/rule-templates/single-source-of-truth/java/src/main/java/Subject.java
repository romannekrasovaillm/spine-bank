/**
 * Эталонная реализация: инвариант соблюдён — тесты зелёные из коробки.
 *
 * <p>Класс назван так же, как в нарушающей реализации
 * ({@code violating/Subject.java}): проверка зубов подменяет файл целиком,
 * поэтому имена совпадают.
 */
public class Subject {

    private final Fakes.FakeSource source;
    private int value;
    private int version;
    private boolean stale;

    public Subject(Fakes.FakeSource source) {
        this.source = source;
    }

    /** Показать значение вместе с его версией и признаком устаревания. */
    public Fakes.Snapshot read() {
        return new Fakes.Snapshot(value, version, stale);
    }

    /**
     * Принять ответ источника как есть: со своей версией и признаком свежести.
     *
     * <p>Свойство: устаревший ответ помечается устаревшим, а не выдаётся за текущий.
     */
    public Fakes.Snapshot refresh() {
        Fakes.Snapshot answer = source.pull();
        this.value = answer.value();
        this.version = answer.version();
        this.stale = answer.stale();
        return read();
    }

    /**
     * Отправить значение в источник.
     *
     * <p>Свойство: локальное значение меняется только вместе с подтверждённым
     * ответом источника — проекция не «додумывает» состояние.
     */
    public Fakes.Snapshot submit(int newValue) {
        Fakes.Ack ack = source.write(newValue);
        this.value = ack.value();
        this.version = ack.version();
        this.stale = false;
        return read();
    }
}

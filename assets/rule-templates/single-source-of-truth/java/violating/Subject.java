/**
 * Нарушающая реализация: одно засеянное нарушение — устаревший ответ выдан за актуальный.
 *
 * <p>Проверка зубов обязана упасть на этом файле. Нарушение ровно одно:
 * {@code refresh} не переносит в проекцию признак устаревания, поэтому
 * задержавшийся ответ источника молча выдаётся за текущее состояние.
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

    /** Принять любой ответ источника как текущий. */
    public Fakes.Snapshot refresh() {
        Fakes.Snapshot answer = source.pull();
        this.value = answer.value();
        this.version = answer.version();
        // Единственное засеянное нарушение: признак устаревания не переносится —
        // устаревший ответ выдаётся за актуальный.
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

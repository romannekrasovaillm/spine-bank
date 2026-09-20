import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.List;

import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/**
 * Свойства идемпотентности по ключу — читаются как спецификация инварианта.
 *
 * <p>Тест читает реализацию из {@code Subject.java}. Проверка зубов
 * ({@code arch-be rules template verify}) подменяет этот файл нарушающей
 * реализацией — тест обязан упасть.
 */
@DisplayName("Идемпотентность по ключу")
class IdempotencyKeyTest {

    @Test
    @DisplayName("повторная доставка с тем же ключом даёт один эффект")
    void twoDeliveriesWithSameKeyMakeOneEffect() {
        Fakes.FakePlatform platform = new Fakes.FakePlatform();
        Subject service = new Subject(platform);

        service.process("key-1", 100);
        service.process("key-1", 100);

        assertEquals(1, platform.calls(),
                "повторная доставка обратилась к внешней системе второй раз");
        assertEquals(List.of("key-1"), platform.effectKeys(),
                "повторная доставка с тем же ключом создала второй эффект у внешней системы");
    }

    @Test
    @DisplayName("ответ на повторную доставку равен первому ответу")
    void repeatDeliveryReturnsTheFirstAnswer() {
        Fakes.FakePlatform platform = new Fakes.FakePlatform();
        Subject service = new Subject(platform);

        Fakes.Answer first = service.process("key-2", 250);
        Fakes.Answer second = service.process("key-2", 250);

        assertEquals(first, second,
                "ответ на повторную доставку отличается от первого — вызывающий не может "
                        + "отличить уже применённую операцию от новой");
    }

    @Test
    @DisplayName("разные ключи дают два независимых эффекта")
    void differentKeysMakeTwoEffects() {
        Fakes.FakePlatform platform = new Fakes.FakePlatform();
        Subject service = new Subject(platform);

        service.process("key-3", 100);
        service.process("key-4", 100);

        assertEquals(List.of("key-3", "key-4"), platform.effectKeys(),
                "разные ключи должны давать два независимых эффекта");
    }
}

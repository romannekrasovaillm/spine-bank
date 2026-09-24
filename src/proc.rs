//! Запуск внешних команд с таймаутом: процессные группы и читатели вывода
//! с дедлайном (A1).
//!
//! Прежняя идиома `spawn bash -c … → child.kill() по таймауту` убивала только
//! интерпретатор: внуки (`pytest` с воркерами, `./gradlew`, `server &`)
//! оставались сиротами и, держа унаследованные stdout/stderr, подвешивали
//! потоки-читатели на EOF — гейт ждал завершения внуков (бесконечно, если те
//! не выходят). Здесь единая точка механики: shell стартует в собственной
//! процессной группе, таймаут завершает группу целиком (TERM → grace → KILL),
//! а ожидание читателей ограничено дедлайном.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::error::{HarnessError, Result};

/// Шаг опроса `try_wait` в синхронном ожидании (как в точках-источниках).
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Сколько ждём завершения процессной группы после SIGTERM, прежде чем
/// добить SIGKILL (синхронный путь; у async-пути своё окно — 10 × 300 мс).
const TERM_GRACE: Duration = Duration::from_secs(1);

/// Дедлайн ожидания читателей вывода после завершения/убийства процесса:
/// внук, переживший убийство группы (был отсоединён от неё самой командой),
/// держит pipe открытым — без дедлайна ожидание EOF не кончилось бы никогда.
pub(crate) const READER_JOIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Сколько байт вывода храним для отчёта об упавшей команде (хвост).
pub(crate) const MAX_CAPTURE_BYTES: usize = 16 * 1024;

/// Собирает `Command` для `<shell> -c <command>` в СОБСТВЕННОЙ процессной
/// группе (unix): при таймауте убивается группа целиком, и потомки команды
/// не остаются сиротами, держащими pipe (см. шапку модуля). Каталог, stdio и
/// env выставляет вызывающий. Вне unix групп нет — убивается только сам
/// процесс (fallback в [`kill_process_group_sync`]).
pub(crate) fn shell_command(shell: &str, command: &str) -> Command {
    let mut cmd = Command::new(shell);
    cmd.arg("-c").arg(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd
}

/// Tokio-вариант [`shell_command`] для async-точек (eval, прогон сьютов);
/// потребители живут только в сборке `harness`.
#[cfg(feature = "harness")]
pub(crate) fn shell_command_tokio(shell: &str, command: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg("-c").arg(command);
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

/// Посылает сигнал `sig` процессной группе `pgid` через bash-builtin kill.
///
/// Почему не внешний `/bin/kill`: slim-образы контейнеров
/// (`rust:1.85-slim-bookworm`) НЕ содержат procps — `Command::new("kill")`
/// падает `NotFound`, а проглоченная ошибка (`let _ =`) оставляла группу
/// живой (красная hermetic-джоба CI 2026-09-24: внуки переживали таймаут).
/// bash — уже жёсткая зависимость исполнителя ([`shell_command`]), его
/// builtin kill есть всегда; unsafe/libc запрещены линтом проекта.
///
/// ВАЖНО: разделитель `--` обязателен — procps `/bin/kill -TERM -PGID` без
/// него молча (rc=0!) трактует отрицательное число как опцию и никого не
/// сигналит (проверено опытом). builtin работал бы и так, но форма держится
/// единой и безопасной для обеих реализаций.
///
/// Исход намеренно не возвращается: ESRCH (группа завершилась сама раньше)
/// — штатная ситуация на пути убийства.
fn signal_group(pgid: u32, sig: &str) {
    let _ = Command::new("bash")
        .args(["-c", &format!("kill -{sig} -- -{pgid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Жив ли процесс `pid` — по `/proc/<pid>/stat`: файл отсутствует или
/// состояние `Z`/`X` (зомби/мёртв) — считается НЕЖИВЫМ. Точнее `kill -0`:
/// зомби отвечает на сигнал 0 успехом, а в контейнерах, где PID 1 не
/// reaper (GitHub Actions container jobs), убитая сирота может висеть
/// зомби вечно — проверка сирот через `kill -0` там флачит.
#[cfg(test)]
fn pid_alive(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false; // процесса нет
    };
    // Поле state — после ПОСЛЕДНЕГО ") " (comm в скобках может содержать
    // пробелы и скобки). Нет поля — считаем мёртвым.
    let Some((_, rest)) = stat.rsplit_once(") ") else {
        return false;
    };
    !matches!(rest.chars().next(), Some('Z' | 'X'))
}

/// Мягко, затем жёстко завершает процессную группу `pid`
/// (TERM → [`TERM_GRACE`] → KILL) и забирает зомби. Убивает и потомков
/// команды — сирот после таймаута не остаётся. Вне unix (где группа не
/// ставилась) завершает только сам процесс — дерева там не создавалось.
pub(crate) fn kill_process_group_sync(pid: u32, child: &mut Child) {
    if pid > 0 && cfg!(unix) {
        signal_group(pid, "TERM");
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        signal_group(pid, "KILL");
    } else {
        // pgid неизвестен (теоретический случай) или ОС без процессных групп —
        // хотя бы сам процесс.
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    // Забираем зомби, чтобы try_wait наверняка отдал статус.
    let _ = child.wait();
}

/// Мягко, затем жёстко завершает процессную группу `pid` (TERM → 3 с → KILL).
/// Убивает и дочерние процессы харнесса — сирот после таймаута не остаётся.
/// Вне unix (где `process_group` не ставился) завершает только сам процесс —
/// дерево процессов там не создавалось. Потребители (прогон харнессов и
/// eval-сьютов) живут только в сборке `harness`.
#[cfg(feature = "harness")]
pub(crate) async fn kill_process_group(pid: u32, child: &mut tokio::process::Child) {
    if pid > 0 && cfg!(unix) {
        signal_group(pid, "TERM");
        for _ in 0..10 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        signal_group(pid, "KILL");
    } else {
        // pgid неизвестен (теоретический случай) или ОС без процессных групп —
        // хотя бы самого ребёнка.
        let _ = child.kill().await;
        return;
    }
    // Забираем zombie, чтобы try_wait наверняка отдал статус.
    let _ = tokio::time::timeout(Duration::from_secs(3), child.wait()).await;
}

/// Читает pipe до конца, храня только хвост в [`MAX_CAPTURE_BYTES`]
/// (вывод упавшей команды может быть мегабайтным — в отчёт нужен конец).
pub(crate) fn drain_tail(mut pipe: impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_CAPTURE_BYTES * 2 {
                    let keep = buf.split_off(buf.len() - MAX_CAPTURE_BYTES);
                    buf = keep;
                }
            }
        }
    }
    if buf.len() > MAX_CAPTURE_BYTES {
        buf.split_off(buf.len() - MAX_CAPTURE_BYTES)
    } else {
        buf
    }
}

/// Поток-читатель pipe с дедлайном ожидания результата. `JoinHandle` таймаута
/// не имеет, поэтому результат приходит каналом: читатель шлёт прочитанное,
/// принимающая сторона ждёт `recv_timeout`.
pub(crate) struct PipeReader {
    rx: mpsc::Receiver<Vec<u8>>,
}

impl PipeReader {
    /// Запускает поток, читающий pipe до EOF с хвостом [`MAX_CAPTURE_BYTES`].
    pub(crate) fn spawn(pipe: impl Read + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // Ошибка отправки игнорируется: принимающая сторона могла уйти по
            // дедлайну — тогда прочитанное просто не нужно.
            let _ = tx.send(drain_tail(pipe));
        });
        Self { rx }
    }

    /// Забирает прочитанное, ожидая не дольше [`READER_JOIN_TIMEOUT`]:
    /// отсоединённый внук, держащий pipe, не подвешивает вызывающего — при
    /// дедлайне (или гибели потока-читателя) отчитываемся тем, что успели
    /// прочитать к этому моменту (пустым буфером).
    pub(crate) fn join_bounded(self) -> Vec<u8> {
        self.rx
            .recv_timeout(READER_JOIN_TIMEOUT)
            .unwrap_or_default()
    }
}

/// Исход синхронного прогона shell-команды: статус и хвосты вывода.
pub(crate) struct ShellOutcome {
    /// `Some`, если команда завершилась сама (иначе — убита по таймауту).
    pub(crate) status: Option<ExitStatus>,
    /// Последние байты stdout (обрезаны до [`MAX_CAPTURE_BYTES`]).
    pub(crate) stdout: Vec<u8>,
    /// Последние байты stderr (обрезаны до [`MAX_CAPTURE_BYTES`]).
    pub(crate) stderr: Vec<u8>,
}

/// Запускает `<shell> -c <command>` в `dir` с ручным таймаутом: spawn в
/// собственной процессной группе + опрос `try_wait` каждые
/// [`POLL_INTERVAL`] + по истечении убийство ВСЕЙ группы
/// ([`kill_process_group_sync`]). `Ok` с `status: None` — команда превысила
/// таймаут и была убита вместе с потомками.
///
/// stdout/stderr читаются потоками [`PipeReader`] (иначе полный pipe
/// заблокирует дочерний процесс задолго до таймаута); после убийства
/// читатели ждутся не дольше [`READER_JOIN_TIMEOUT`].
///
/// # Errors
/// Процесс не запустился; сбой `try_wait`.
pub(crate) fn run_shell(
    dir: &Path,
    shell: &str,
    command: &str,
    timeout: Duration,
) -> Result<ShellOutcome> {
    let mut cmd = shell_command(shell, command);
    cmd.current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| HarnessError::Control(format!("не удалось запустить {shell}: {e}")))?;
    let pid = child.id();
    let out_reader = child.stdout.take().map(PipeReader::spawn);
    let err_reader = child.stderr.take().map(PipeReader::spawn);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    kill_process_group_sync(pid, &mut child);
                    break None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => {
                kill_process_group_sync(pid, &mut child);
                return Err(HarnessError::Control(format!(
                    "ошибка ожидания команды '{command}': {e}"
                )));
            }
        }
    };
    let stdout = out_reader.map_or_else(Vec::new, PipeReader::join_bounded);
    let stderr = err_reader.map_or_else(Vec::new, PipeReader::join_bounded);
    Ok(ShellOutcome {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Команда дольше таймаута убивается вместе с потомком за считаные
    /// секунды, а не за время её собственной жизни (регрессия A1:
    /// `child.kill()` убивал только bash, `sleep`-внук держал pipe, и
    /// читатели ждали EOF до его завершения).
    #[test]
    fn timeout_kills_sleeping_command_within_grace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        let outcome = run_shell(tmp.path(), "bash", "sleep 15; true", Duration::from_secs(1))
            .expect("прогон команды");
        assert!(outcome.status.is_none(), "команда убита по таймауту");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "таймаут 1s, а заняло {:?} — внук держал pipe",
            started.elapsed()
        );
    }

    /// Кейс бесконечного висения: фоновой внук наследует stdout, и без
    /// убийства группы + дедлайна читателей прогон не завершился бы никогда.
    #[test]
    fn timeout_kills_background_grandchildren_within_grace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let started = Instant::now();
        let outcome = run_shell(
            tmp.path(),
            "bash",
            "(sleep 30 &); sleep 30",
            Duration::from_secs(1),
        )
        .expect("прогон команды");
        assert!(outcome.status.is_none(), "команда убита по таймауту");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "таймаут 1s, а заняло {:?} — фоновой внук держал pipe",
            started.elapsed()
        );
    }

    /// Фоновой потомок не переживает таймаут: после убийства группы
    /// `kill -0 <pid>` возвращает ошибку — сироты нет.
    #[test]
    fn timeout_leaves_no_orphan_processes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pidfile = tmp.path().join("child.pid");
        let cmd = format!("sleep 60 & echo $! > {}; sleep 60", pidfile.display());
        let outcome =
            run_shell(tmp.path(), "bash", &cmd, Duration::from_secs(1)).expect("прогон команды");
        assert!(outcome.status.is_none(), "команда убита по таймауту");
        let pid = std::fs::read_to_string(&pidfile).expect("child.pid записан");
        let pid: u32 = pid.trim().parse().expect("$! — числовой pid");
        // Опрос до 2 с: убитый потомок может умирать не мгновенно; зомби
        // при этом считается мёртвым (pid_alive читает /proc — см. выше),
        // поэтому поведение не зависит от reaper'а контейнера.
        let mut alive = true;
        for _ in 0..40 {
            if !pid_alive(pid) {
                alive = false;
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(!alive, "фоновой потомок {pid} пережил таймаут — сирота");
    }

    /// Хвост вывода захватывается и после миграции: маркеры из обоих
    /// потоков на месте, код выхода сохраняется.
    #[test]
    fn captures_output_tails_and_exit_code() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outcome = run_shell(
            tmp.path(),
            "bash",
            "echo marker-stdout; echo marker-stderr >&2; exit 3",
            Duration::from_secs(10),
        )
        .expect("прогон команды");
        assert_eq!(outcome.status.and_then(|s| s.code()), Some(3));
        let stdout = String::from_utf8_lossy(&outcome.stdout);
        let stderr = String::from_utf8_lossy(&outcome.stderr);
        assert!(stdout.contains("marker-stdout"), "stdout: {stdout}");
        assert!(stderr.contains("marker-stderr"), "stderr: {stderr}");
    }

    /// Успешная команда завершается сама — убийства группы нет, статус Some.
    #[test]
    fn successful_command_completes_normally() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let outcome =
            run_shell(tmp.path(), "sh", "true", Duration::from_secs(10)).expect("прогон команды");
        assert!(outcome.status.is_some_and(|s| s.success()));
    }

    #[test]
    fn drain_tail_bounds_memory_to_capture_limit() {
        let big = vec![b'x'; MAX_CAPTURE_BYTES * 3];
        let tail = drain_tail(&big[..]);
        assert_eq!(tail.len(), MAX_CAPTURE_BYTES);
    }
}

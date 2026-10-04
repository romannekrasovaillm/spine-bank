//! Конформность ACP-фикстуры `tests/fixtures/acp_agent.py`: два свойства,
//! проваливших TCK v1 (`docs/experiments/acp-tck-2026-10-04.md`).
//!
//! - ACP-EXT-001 / ACP-ERROR-001: запрос с неизвестным методом получает
//!   JSON-RPC error `-32601` с непустым однострочным `message` (фикстура
//!   больше не молчит);
//! - ACP-SESSION-002: каждый `session/new` получает свежий уникальный
//!   `sessionId`, последний выданный — активный.
//!
//! Тест говорит с фикстурой сырым newline-delimited JSON-RPC — так же, как
//! это делает TCK, — и не зависит от ACP-клиента харнесса.

#![cfg(feature = "harness")]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use serde_json::{Value, json};

/// Потолок ожидания одного кадра: фикстура отвечает мгновенно, таймаут —
/// страховка от зависшего процесса (иначе тест висел бы вечно).
const READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Абсолютный путь к python3, если он есть (whitelist может не иметь PATH).
fn python() -> String {
    for candidate in ["/usr/bin/python3", "/usr/local/bin/python3"] {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    String::from("python3")
}

/// Есть ли `python3` в PATH: без него фикстуру не запустить, тесты
/// скипаются (инвариант A2 — герметичный прогон без внешних зависимостей).
fn python3_available() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("python3").is_file()))
}

fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/acp_agent.py")
}

/// Запущенная фикстура: stdin для кадров, канал распарсенных кадров stdout.
struct Fixture {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
}

impl Fixture {
    fn start(mode: &str) -> Self {
        let mut child = Command::new(python())
            .arg(fixture_path())
            .arg(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("запуск ACP-фикстуры");
        let stdin = child.stdin.take().expect("stdin фикстуры");
        let stdout = child.stdout.take().expect("stdout фикстуры");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if tx.send(value).is_err() {
                    break;
                }
            }
        });
        Self { child, stdin, rx }
    }

    fn send(&mut self, value: &Value) {
        writeln!(self.stdin, "{value}").expect("запись кадра");
        self.stdin.flush().expect("flush кадра");
    }

    fn recv(&self) -> Value {
        self.rx
            .recv_timeout(READ_TIMEOUT)
            .expect("фикстура обязана ответить в срок")
    }

    /// Ответ (`result`/`error`) на запрос с данным id; уведомления-апдейты
    /// (`session/update` без id) пропускаются.
    fn recv_response(&self, id: u64) -> Value {
        loop {
            let msg = self.recv();
            if msg.get("id").and_then(Value::as_u64) == Some(id) && msg.get("method").is_none() {
                return msg;
            }
        }
    }

    fn initialize(&mut self) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}
        }));
        let resp = self.recv_response(1);
        assert!(resp.get("result").is_some(), "initialize: {resp}");
    }

    fn open_session(&mut self, id: u64) -> String {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/new",
            "params": {"cwd": "/tmp", "mcpServers": []}
        }));
        self.recv_response(id)["result"]["sessionId"]
            .as_str()
            .expect("session/new вернул sessionId")
            .to_string()
    }

    fn prompt(&mut self, id: u64, session_id: &str) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "session/prompt",
            "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": "задача"}]}
        }));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// ACP-EXT-001 / ACP-ERROR-001: неизвестный (в т.ч. `_`-кастомный) метод
/// получает `-32601` с непустым однострочным message, а сессия остаётся
/// рабочей — прогон доходит до `end_turn`.
#[test]
fn unknown_method_gets_method_not_found_error() {
    if !python3_available() {
        eprintln!("skipped: no python3");
        return;
    }
    let mut fx = Fixture::start("ok");
    fx.initialize();
    let session = fx.open_session(2);
    assert_ne!(session, "", "session/new вернул пустой sessionId");

    for (id, method) in [(3u64, "_custom/hello"), (4, "vendor/mystery")] {
        fx.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {}
        }));
        let resp = fx.recv_response(id);
        let error = resp.get("error").expect("ожидался JSON-RPC error");
        assert_eq!(error["code"], json!(-32601), "код: {resp}");
        let message = error["message"].as_str().expect("message — строка");
        assert!(!message.is_empty(), "message обязан быть непустым");
        assert!(
            !message.contains('\n') && !message.contains('\r'),
            "message одной строкой: {message:?}"
        );
        assert!(
            message.contains(method),
            "message называет метод: {message:?}"
        );
    }

    // Фикстура не «сломалась» на ошибках: обычный промпт доходит до конца.
    fx.prompt(5, &session);
    let done = fx.recv_response(5);
    assert_eq!(done["result"]["stopReason"], json!("end_turn"), "{done}");
}

/// ACP-SESSION-002: второй `session/new` на том же соединении отвечает и
/// получает ДРУГОЙ `sessionId`; активной становится последняя сессия.
#[test]
fn every_session_new_gets_unique_id() {
    if !python3_available() {
        eprintln!("skipped: no python3");
        return;
    }
    let mut fx = Fixture::start("ok");
    fx.initialize();

    let first = fx.open_session(2);
    // Лишний (неизвестный) метод между сессиями не мешает второй.
    fx.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "_probe", "params": {}}));
    assert_eq!(fx.recv_response(3)["error"]["code"], json!(-32601));

    let second = fx.open_session(4);
    assert!(!first.is_empty() && !second.is_empty());
    assert_ne!(first, second, "sessionId уникален на каждый session/new");

    // Активна последняя сессия: session/update несёт именно её id.
    fx.prompt(5, &second);
    loop {
        let msg = fx.recv();
        if msg.get("method") == Some(&json!("session/update")) {
            assert_eq!(
                msg["params"]["sessionId"],
                json!(second),
                "активной обязана быть последняя сессия"
            );
            break;
        }
    }
    let done = fx.recv_response(5);
    assert_eq!(done["result"]["stopReason"], json!("end_turn"), "{done}");
}

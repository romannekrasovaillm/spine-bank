//! Локальный egress-шлюз (vpn-egress): проверка порта и мягкий автозапуск.
//!
//! Провайдер с полем `proxy` в `[models.*]` может указывать на локальный
//! шлюз (напр. sing-box на `127.0.0.1:12080` — вывод трафика из РФ для
//! регионально заблокированных API). Харнесс при построении LLM-клиента и при
//! переключении модели проверяет порт и, если он недоступен, пытается поднять
//! systemd user-юнит `vpn-egress`. Инварианты платформы: управление ТОЛЬКО
//! через `systemctl --user ... vpn-egress` (ручной запуск sing-box параллельно
//! с юнитом запрещён — конфликт порта), порт шлюза — только loopback.
//! Любой сбой здесь — мягкая ошибка (предупреждение в лог/статус): запрос
//! всё равно идёт через настроенный прокси, паники нет.

use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::doctor::{ProbeResult, ProbeRow};
use crate::error::{HarnessError, Result};

/// Таймаут TCP-пробы порта шлюза, миллисекунды.
const EGRESS_PROBE_TIMEOUT_MS: u64 = 300;

/// Суммарное ожидание подъёма шлюза после `systemctl --user start`, секунды.
const MAX_EGRESS_WAIT_SECS: u64 = 5;

/// Пауза между пробами порта в ожидании подъёма, миллисекунды.
const EGRESS_POLL_STEP_MS: u64 = 200;

/// systemd user-юнит локального egress-шлюза vpn-платформы.
const EGRESS_UNIT: &str = "vpn-egress";

/// Исход проверки локального egress-шлюза.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressStatus {
    /// Порт уже слушался — ничего запускать не потребовалось.
    AlreadyRunning,
    /// Порт был недоступен; юнит стартован и порт поднялся.
    Started,
}

/// Разбирает URL прокси (`http://host:port`, `socks5://host:port`,
/// опционально с userinfo) в пару (хост, порт). Порт по умолчанию — по схеме:
/// `http` — 80, `https` — 443, `socks5`/`socks5h` — 1080 (как у reqwest).
///
/// # Errors
/// Нет схемы или хоста, нечисловой порт, схема без известного порта по
/// умолчанию (порт тогда обязателен в URL).
pub fn proxy_endpoint(url: &str) -> Result<(String, u16)> {
    let bad = |reason: &str| {
        HarnessError::Config(format!(
            "прокси '{url}': {reason} (ожидается scheme://host:port)"
        ))
    };
    let (scheme, rest) = url
        .trim()
        .split_once("://")
        .ok_or_else(|| bad("нет схемы"))?;
    // Отрезаем путь/квери и userinfo — нужен только authority host:port.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if let Some((h, p)) = authority.rsplit_once(':') {
        let port: u16 = p
            .parse()
            .map_err(|_| bad(&format!("нечисловой порт '{p}'")))?;
        (h.to_string(), port)
    } else {
        let port = match scheme {
            "http" => 80,
            "https" => 443,
            "socks5" | "socks5h" => 1080,
            other => {
                return Err(bad(&format!(
                    "схема '{other}' без известного порта по умолчанию"
                )));
            }
        };
        (authority.to_string(), port)
    };
    if host.is_empty() {
        return Err(bad("пустой хост"));
    }
    Ok((host, port))
}

/// Loopback-хост (`127.0.0.0/8`, `::1`, `localhost`): автозапуск локального
/// юнита применим только к таким прокси; внешний прокси — не наш юнит.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// TCP-проба порта с коротким таймаутом (любой из разрешённых адресов).
fn port_open(host: &str, port: u16) -> bool {
    let probe = || {
        (host, port).to_socket_addrs().map(|mut addrs| {
            addrs.any(|addr| {
                TcpStream::connect_timeout(&addr, Duration::from_millis(EGRESS_PROBE_TIMEOUT_MS))
                    .is_ok()
            })
        })
    };
    probe().unwrap_or(false)
}

/// Гарантирует доступность локального egress-шлюза из URL прокси.
///
/// - прокси не loopback → `Ok(None)`: внешний прокси, автозапуск неприменим;
/// - порт доступен → `Ok(Some(`[`EgressStatus::AlreadyRunning`]`))`;
/// - порт недоступен → `systemctl --user start vpn-egress`, затем поллинг
///   порта до [`MAX_EGRESS_WAIT_SECS`] секунд; поднялся →
///   `Ok(Some(`[`EgressStatus::Started`]`))`, иначе — мягкая ошибка
///   (вызывающий показывает предупреждение, запрос не блокируется).
///
/// # Errors
/// Битый URL прокси, сбой/отказ запуска `systemctl`, шлюз не поднялся за
/// отведённое время.
pub fn ensure_local_egress(proxy_url: &str) -> Result<Option<EgressStatus>> {
    let (host, port) = proxy_endpoint(proxy_url)?;
    if !is_loopback_host(&host) {
        return Ok(None);
    }
    if port_open(&host, port) {
        return Ok(Some(EgressStatus::AlreadyRunning));
    }
    let status = Command::new("systemctl")
        .args(["--user", "start", EGRESS_UNIT])
        .status()
        .map_err(|e| {
            HarnessError::Config(format!(
                "egress-шлюз {host}:{port} недоступен и systemctl не запустился: {e}"
            ))
        })?;
    if !status.success() {
        return Err(HarnessError::Config(format!(
            "egress-шлюз {host}:{port} недоступен; \
             systemctl --user start {EGRESS_UNIT} завершился: {status}"
        )));
    }
    let deadline = Instant::now() + Duration::from_secs(MAX_EGRESS_WAIT_SECS);
    loop {
        if port_open(&host, port) {
            return Ok(Some(EgressStatus::Started));
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(EGRESS_POLL_STEP_MS));
    }
    Err(HarnessError::Config(format!(
        "egress-шлюз {host}:{port} не поднялся за {MAX_EGRESS_WAIT_SECS} с \
         после запуска {EGRESS_UNIT} (диагностика: \
         systemctl --user status {EGRESS_UNIT})"
    )))
}

// ---------------------------------------------------------------------
// Сетевая проба endpoint'ов моделей (doctor «models-net»). Живёт здесь, а
// не в `doctor.rs`: core-редакция свободна от сетевых крейтов (правило
// `core_free_of_network_and_tui_crates`), reqwest — только harness-фича.
// Типы результата — чистые данные и лежат в `crate::doctor`.
// ---------------------------------------------------------------------

/// Таймаут одной пробы endpoint'а (секунды). Пробы идут параллельно, поэтому
/// это же и верхняя граница всей «models-net»: DPI-окна и мёртвые хосты не
/// растягивают doctor дольше.
const MODEL_PROBE_TIMEOUT_SECS: u64 = 5;

/// Реальная параллельная проба всех http-моделей конфига.
pub(crate) fn probe_models(cfg: &crate::config::Config) -> Vec<ProbeRow> {
    let mut rows: Vec<ProbeRow> = Vec::new();
    let mut targets: Vec<(String, String, crate::config::ModelConfig)> = Vec::new();
    for (name, mc) in &cfg.models {
        if mc.kind.as_deref() == Some("cli") {
            rows.push(ProbeRow {
                name: name.clone(),
                url: String::new(),
                result: ProbeResult::Skipped("kind = \"cli\""),
            });
            continue;
        }
        let configured = mc.base_url.trim();
        let base = if configured.is_empty() {
            // Пустой base_url — рантайм применяет пресет вендора; проба
            // обязана целиться туда же (матч — `llm::preset_base_url`).
            if let Some(u) = crate::llm::preset_base_url(name) {
                u
            } else {
                rows.push(ProbeRow {
                    name: name.clone(),
                    url: String::new(),
                    result: ProbeResult::Skipped("base_url не задан"),
                });
                continue;
            }
        } else {
            configured
        };
        targets.push((
            name.clone(),
            base.trim_end_matches('/').to_string(),
            mc.clone(),
        ));
    }
    // Отдельный поток со своим рантаймом: run_checks — синхронный контракт и
    // вызывается и из tokio-контекста (CLI/TUI), и из чистых тестов; вложенный
    // block_on в потоке чужого рантайма tokio запрещён.
    let probed = std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return Vec::new();
        };
        rt.block_on(async move {
            let mut set = tokio::task::JoinSet::new();
            for (name, url, mc) in targets {
                set.spawn(async move { probe_one(name, url, mc).await });
            }
            let mut out = Vec::new();
            while let Some(joined) = set.join_next().await {
                // Паника пробы теряет строку, а не весь doctor.
                if let Ok(row) = joined {
                    out.push(row);
                }
            }
            out
        })
    })
    .join()
    .unwrap_or_default();
    rows.extend(probed);
    rows
}

/// Одна проба: `GET <base>/models` без ключа, короткий таймаут, клиент с
/// учётом proxy/CA/mTLS модели (автозапуск egress отключён — doctor не
/// мутирует машину).
async fn probe_one(name: String, base: String, mc: crate::config::ModelConfig) -> ProbeRow {
    let client = match crate::llm::openai_compat::build_client_with_egress(&name, &mc, false) {
        Ok(c) => c,
        Err(e) => {
            return ProbeRow {
                name,
                url: base,
                result: ProbeResult::Down {
                    reason: format!("сборка клиента: {e}"),
                    latency_ms: 0,
                },
            };
        }
    };
    let started = std::time::Instant::now();
    let res = client
        .get(format!("{base}/models"))
        .timeout(std::time::Duration::from_secs(MODEL_PROBE_TIMEOUT_SECS))
        .send()
        .await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match res {
        Ok(resp) => ProbeRow {
            name,
            url: base,
            result: ProbeResult::Alive {
                status: resp.status().as_u16(),
                latency_ms,
            },
        },
        Err(e) => {
            let reason = probe_failure_reason(&e);
            ProbeRow {
                name,
                url: base,
                result: ProbeResult::Down { reason, latency_ms },
            }
        }
    }
}

/// Короткая причина транспортного отказа пробы (без URL и цепочек reqwest):
/// именно она отличает «модель умерла» от «сеть/DPI».
fn probe_failure_reason(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        return format!("таймаут >{MODEL_PROBE_TIMEOUT_SECS}с (модель мертва или сеть/DPI)");
    }
    let chain = crate::llm::openai_compat::error_chain(e).to_lowercase();
    if chain.contains("connection refused") {
        "отказ в соединении (сервис не слушает порт)".to_string()
    } else if chain.contains("connection reset") || chain.contains("broken pipe") {
        "соединение сброшено (сеть/DPI?)".to_string()
    } else if chain.contains("dns") || chain.contains("resolve") || chain.contains("no such host") {
        "DNS не разрешил хост".to_string()
    } else if chain.contains("certificate") || chain.contains("tls") {
        "TLS-ошибка (сертификат/прокси)".to_string()
    } else {
        let short: String = chain.trim().chars().take(120).collect();
        format!("сетевой сбой ({short})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_endpoint_parses_explicit_port() {
        assert_eq!(
            proxy_endpoint("http://127.0.0.1:12080").expect("parse"),
            ("127.0.0.1".to_string(), 12080)
        );
        assert_eq!(
            proxy_endpoint("socks5://localhost:1080").expect("parse"),
            ("localhost".to_string(), 1080)
        );
        // Userinfo и путь не влияют на endpoint.
        assert_eq!(
            proxy_endpoint("http://user:pass@10.0.0.2:3128/proxy").expect("parse"),
            ("10.0.0.2".to_string(), 3128)
        );
    }

    #[test]
    fn proxy_endpoint_applies_scheme_default_ports() {
        assert_eq!(
            proxy_endpoint("http://proxy.internal").expect("parse"),
            ("proxy.internal".to_string(), 80)
        );
        assert_eq!(
            proxy_endpoint("https://proxy.internal").expect("parse"),
            ("proxy.internal".to_string(), 443)
        );
        assert_eq!(
            proxy_endpoint("socks5h://proxy.internal").expect("parse"),
            ("proxy.internal".to_string(), 1080)
        );
    }

    #[test]
    fn proxy_endpoint_rejects_garbage() {
        for url in [
            "",
            "127.0.0.1:12080",        // нет схемы
            "http://",                // пустой хост
            "http://127.0.0.1:abc",   // нечисловой порт
            "http://127.0.0.1:99999", // порт вне u16
            "gopher://host",          // неизвестная схема без порта
        ] {
            assert!(proxy_endpoint(url).is_err(), "должен быть отказ: '{url}'");
        }
    }

    #[test]
    fn loopback_detection_covers_ip_and_localhost() {
        for host in ["127.0.0.1", "127.0.0.2", "::1", "localhost", "LOCALHOST"] {
            assert!(is_loopback_host(host), "{host} — loopback");
        }
        for host in ["10.0.0.1", "192.168.1.1", "proxy.internal", "example.com"] {
            assert!(!is_loopback_host(host), "{host} — не loopback");
        }
    }

    #[test]
    fn ensure_skips_non_loopback_proxy_without_side_effects() {
        // Внешний прокси: ни TCP-пробы, ни systemctl — детерминированный None.
        assert_eq!(
            ensure_local_egress("http://proxy.internal:3128").expect("ok"),
            None
        );
    }
}

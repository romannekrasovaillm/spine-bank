//! Тонкая точка входа: парсинг аргументов → вызов lib → код возврата.

mod cli;

#[tokio::main]
async fn main() {
    if let Err(err) = cli::run().await {
        report_error(&err);
        std::process::exit(1);
    }
}

/// Край CLI печатает пользовательскую ошибку сам (B9): `Error: …` и цепочку
/// `Caused by: …` — **без стектрейса**, независимо от `RUST_BACKTRACE`
/// (в CI он обычно равен `1`, и дефолтный `anyhow`-Debug сыпал в лицо
/// пользователю стеком `<unknown>`-кадров). Полный стек — только по явному
/// `ARCH_BE_DEBUG=1`.
///
/// Коды возврата остальных веток (`run` использует `process::exit(0..3)`)
/// не затрагиваются: здесь обрабатывается лишь ошибка, проброшенная до края.
fn report_error(err: &anyhow::Error) {
    if std::env::var_os("ARCH_BE_DEBUG").is_some() {
        eprintln!("Error: {err:?}");
        return;
    }
    eprintln!("Error: {err}");
    for cause in err.chain().skip(1) {
        eprintln!("Caused by: {cause}");
    }
}

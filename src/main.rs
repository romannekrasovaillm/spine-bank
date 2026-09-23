//! Тонкая точка входа: парсинг аргументов → вызов lib → код возврата.

mod cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run().await
}

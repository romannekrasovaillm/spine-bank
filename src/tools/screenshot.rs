//! Инструменты работы с изображениями (нативная мультимодальность
//! `deepseek-flash`, формат `OpenAI` `image_url` data-URL):
//! - [`screenshot`] — снять скриншот экрана и вернуть изображение модели;
//! - [`read_image`] — прочитать файл изображения и вернуть его модели.
//!
//! Изображение возвращается в [`ToolOutput::images`]; агентный цикл
//! прикрепляет его к user-сообщению (см. `agent.rs`), после чего модель
//! распознаёт/извлекает данные по скриншоту. Это те же примитивы, что дают
//! браузерно-компьютерное управление у Kimi Code + Kimi K3.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{HarnessError, Result};
use crate::llm::ToolSpec;
use crate::tool::{Tool, ToolContext, ToolOutput};

/// Кандидаты команд скриншота (первый успешный — рабочий).
/// `{path}` — целевой файл, `{display}` — DISPLAY (для `ffmpeg` x11grab).
/// `ffmpeg` — надёжный fallback (уже стоит в ML-контурах); `scrot`/`import` —
/// легче, но требуют установки.
const SCREENSHOT_CANDIDATES: &[&[&str]] = &[
    &["scrot", "{path}"],
    &["import", "-window", "root", "{path}"],
    &["gnome-screenshot", "-f", "{path}"],
    &[
        "ffmpeg",
        "-y",
        "-f",
        "x11grab",
        "-i",
        "{display}",
        "-frames:v",
        "1",
        "{path}",
    ],
];

/// Инструмент `screenshot`: снять скриншот и вернуть его модели.
struct ScreenshotTool;

#[derive(Deserialize)]
struct ScreenshotArgs {
    /// Куда сохранить файл (без него — `state/screenshots/shot-<ts>.png`).
    #[serde(default)]
    path: Option<String>,
}

/// Инструмент `read_image`: прочитать изображение и вернуть его модели.
struct ReadImageTool;

#[derive(Deserialize)]
struct ReadImageArgs {
    /// Путь к файлу изображения (png/jpg/webp/gif).
    path: String,
}

/// Инструменты домена изображений (регистрируются в ядре).
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![Arc::new(ScreenshotTool), Arc::new(ReadImageTool)]
}

#[async_trait]
impl Tool for ScreenshotTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "screenshot".into(),
            description: "Снять скриншот экрана и передать изображение модели для распознавания/извлечения данных. Используй для наблюдения за интерфейсом и ввода данных по скриншоту.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Куда сохранить файл (необязательно; по умолчанию во временный каталог скриншотов)"}
                }
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: ScreenshotArgs = serde_json::from_value(args)
            .map_err(|e| HarnessError::Tool(format!("screenshot: невалидные аргументы: {e}")))?;
        let path = if let Some(p) = args.path {
            ctx.resolve(p)
        } else {
            let dir = ctx.config.paths.state_dir.join("screenshots");
            std::fs::create_dir_all(&dir).map_err(|e| {
                HarnessError::Tool(format!("screenshot: каталог {}: {e}", dir.display()))
            })?;
            dir.join(format!("shot-{}.png", chrono::Utc::now().timestamp()))
        };
        capture_screenshot(&path)?;
        let image = image_data_url(&path)?;
        Ok(
            ToolOutput::ok(format!("Скриншот сохранён: {}", path.display()))
                .with_images(vec![image]),
        )
    }
}

#[async_trait]
impl Tool for ReadImageTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_image".into(),
            description: "Прочитать файл изображения (png/jpg/webp/gif) и передать его модели для распознавания текста/структуры/данных.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Путь к файлу изображения"}
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: ReadImageArgs = serde_json::from_value(args)
            .map_err(|e| HarnessError::Tool(format!("read_image: невалидные аргументы: {e}")))?;
        let path = ctx.resolve(&args.path);
        let image = image_data_url(&path)?;
        Ok(
            ToolOutput::ok(format!("Изображение прочитано: {}", path.display()))
                .with_images(vec![image]),
        )
    }
}

/// Снять скриншот первой доступной утилитой.
fn capture_screenshot(path: &Path) -> Result<()> {
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
    let mut last = String::new();
    for args in SCREENSHOT_CANDIDATES {
        let bin = args[0];
        let rest: Vec<String> = args[1..]
            .iter()
            .map(|a| {
                a.replace("{path}", &path.to_string_lossy())
                    .replace("{display}", &display)
            })
            .collect();
        match std::process::Command::new(bin).args(&rest).output() {
            Ok(out) if out.status.success() => return Ok(()),
            Ok(out) => {
                last = format!("{bin}: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Err(e) => {
                last = format!("{bin}: {e}");
            }
        }
    }
    Err(HarnessError::Tool(format!(
        "не удалось снять скриншот — нужна утилита (scrot/import/gnome-screenshot/ffmpeg). Последняя ошибка: {last}"
    )))
}

/// Прочитать изображение в base64 data-URL.
fn image_data_url(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| HarnessError::Tool(format!("чтение изображения {}: {e}", path.display())))?;
    let mime = mime_from_ext(path);
    Ok(format!(
        "data:{mime};base64,{}",
        crate::clipboard::base64_encode(&bytes)
    ))
}

/// MIME-тип по расширению файла (`DeepSeek` детектирует по содержимому, MIME —
/// лишь префикс data-URL).
fn mime_from_ext(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Минимальный PNG (1×1 пиксель, валидная сигнатура) — для теста чтения.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // сигнатура
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, // bit depth
        0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, // IDAT
        0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, // данные
        0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, // IEND
        0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn mime_from_extension() {
        assert_eq!(mime_from_ext(Path::new("a.png")), "image/png");
        assert_eq!(mime_from_ext(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(mime_from_ext(Path::new("a.webp")), "image/webp");
        assert_eq!(mime_from_ext(Path::new("a.bin")), "image/png");
    }

    #[test]
    fn image_data_url_embeds_base64() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = tmp.path().join("x.png");
        std::fs::write(&p, TINY_PNG).expect("write");
        let url = image_data_url(&p).expect("encode");
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        assert!(url.len() > "data:image/png;base64,".len());
    }

    #[test]
    #[ignore = "live: требует DISPLAY + утилиту скриншота (ffmpeg/scrot)"]
    fn live_capture_produces_png_data_url() {
        let tmp = tempfile::tempdir().expect("tmp");
        let p = tmp.path().join("live.png");
        capture_screenshot(&p).expect("скриншот");
        let bytes = std::fs::read(&p).expect("read");
        assert!(
            bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]),
            "PNG-сигнатура (первые байты: {:02X?})",
            &bytes[..8.min(bytes.len())]
        );
        let url = image_data_url(&p).expect("data-url");
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }
}

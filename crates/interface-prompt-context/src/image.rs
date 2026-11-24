use std::io::Cursor;
use std::time::SystemTime;

use ::image::{DynamicImage, ImageFormat, ImageReader, Limits, imageops::FilterType};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use interface_contracts::prompt_images::{
    PROMPT_IMAGE_LIMITS, is_prompt_image_preview_url, prompt_image_thumbnail_size,
};
use uuid::Uuid;

use crate::service::{PromptContextClock, PromptImageNameMinter};

/// Decode-and-downscale boundary used by paste and thumbnail reads.
pub trait PromptImageThumbnailer: Send + Sync + 'static {
    fn thumbnail(&self, bytes: &[u8], max_edge: u32) -> Option<String>;
}

/// Rust-native equivalent of Electron's `nativeImage` thumbnail path.
#[derive(Clone, Copy, Debug, Default)]
pub struct RustImageThumbnailer;

impl PromptImageThumbnailer for RustImageThumbnailer {
    fn thumbnail(&self, bytes: &[u8], max_edge: u32) -> Option<String> {
        let mut reader = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = Limits::default();
        limits.max_image_width = Some(32_768);
        limits.max_image_height = Some(32_768);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader.decode().ok()?;
        let size = prompt_image_thumbnail_size(
            f64::from(decoded.width()),
            f64::from(decoded.height()),
            max_edge,
        );
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let thumbnail = if size.width == decoded.width() && size.height == decoded.height() {
            decoded
        } else {
            // Resize the native buffer (no full-size RGBA copy outside the
            // decoder's allocation limit), then convert only the small result
            // to 8-bit RGBA so the preview stays inside max_preview_chars.
            DynamicImage::ImageRgba8(
                decoded
                    .resize_exact(size.width, size.height, FilterType::Triangle)
                    .into_rgba8(),
            )
        };
        let mut encoded = Cursor::new(Vec::new());
        thumbnail.write_to(&mut encoded, ImageFormat::Png).ok()?;
        let url = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(encoded.into_inner())
        );
        (url.len() <= PROMPT_IMAGE_LIMITS.max_preview_chars && is_prompt_image_preview_url(&url))
            .then_some(url)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPromptContextClock;

impl PromptContextClock for SystemPromptContextClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemPromptImageNameMinter;

impl PromptImageNameMinter for SystemPromptImageNameMinter {
    fn mint(&self) -> String {
        Uuid::new_v4().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = DynamicImage::new_rgba8(width, height);
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode fixture");
        encoded.into_inner()
    }

    #[test]
    fn decoder_returns_a_bounded_png_data_url() {
        let url = RustImageThumbnailer
            .thumbnail(&png(640, 320), PROMPT_IMAGE_LIMITS.thumbnail_max_edge)
            .expect("thumbnail");
        assert!(url.starts_with("data:image/png;base64,"));
        assert!(is_prompt_image_preview_url(&url));
    }

    #[test]
    fn a_downscaled_16_bit_image_becomes_an_8_bit_rgba_preview() {
        let image = DynamicImage::new_rgb16(1200, 600);
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode fixture");
        let url = RustImageThumbnailer
            .thumbnail(&encoded.into_inner(), 320)
            .expect("thumbnail");
        let payload = url.strip_prefix("data:image/png;base64,").expect("png url");
        let preview = ::image::load_from_memory(&STANDARD.decode(payload).expect("base64"))
            .expect("decode preview");
        assert_eq!((preview.width(), preview.height()), (320, 160));
        assert_eq!(preview.color(), ::image::ColorType::Rgba8);
    }

    #[test]
    fn decoder_refuses_a_magic_header_in_front_of_garbage() {
        assert_eq!(
            RustImageThumbnailer.thumbnail(b"\x89PNG\r\n\x1a\ngarbage", 320),
            None
        );
    }
}

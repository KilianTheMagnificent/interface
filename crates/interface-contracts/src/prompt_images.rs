use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PromptImageLimits {
    pub max_source_bytes: usize,
    pub thumbnail_max_edge: u32,
    pub max_preview_chars: usize,
    pub max_turn_images: usize,
}

pub const PROMPT_IMAGE_LIMITS: PromptImageLimits = PromptImageLimits {
    max_source_bytes: 16 * 1024 * 1024,
    thumbnail_max_edge: 320,
    max_preview_chars: 512_000,
    max_turn_images: 8,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PromptTurnImage {
    pub src: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptImageReference {
    pub text_line: u64,
    pub text_line_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[must_use]
pub fn sniff_prompt_image_format(bytes: &[u8]) -> Option<PromptImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(PromptImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(PromptImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(PromptImageFormat::Gif)
    } else if bytes.starts_with(b"BM") {
        Some(PromptImageFormat::Bmp)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(PromptImageFormat::Webp)
    } else {
        None
    }
}

#[must_use]
pub const fn prompt_image_extension(format: PromptImageFormat) -> &'static str {
    match format {
        PromptImageFormat::Png => ".png",
        PromptImageFormat::Jpeg => ".jpg",
        PromptImageFormat::Gif => ".gif",
        PromptImageFormat::Webp => ".webp",
        PromptImageFormat::Bmp => ".bmp",
    }
}

#[must_use]
pub const fn prompt_image_size_accepted(byte_length: usize) -> bool {
    byte_length > 0 && byte_length <= PROMPT_IMAGE_LIMITS.max_source_bytes
}

#[must_use]
pub fn is_prompt_image_preview_url(value: &str) -> bool {
    static PREVIEW: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^data:image/(?:png|jpeg|webp);base64,[A-Za-z0-9+/]+={0,2}$")
            .expect("preview URL regex")
    });
    value.len() <= PROMPT_IMAGE_LIMITS.max_preview_chars && PREVIEW.is_match(value)
}

#[must_use]
pub fn prompt_image_basename(value: Option<&str>) -> Option<String> {
    static IMAGE_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)\.(?:avif|bmp|gif|heic|jpe?g|png|tiff?|webp)$")
            .expect("image filename regex")
    });
    static URI_SCHEME: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").expect("URI scheme regex"));
    static WINDOWS_PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z]:[\\/]").expect("Windows path regex"));

    let collapsed = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty()
        || collapsed.chars().any(char::is_control)
        || (URI_SCHEME.is_match(&collapsed) && !WINDOWS_PATH.is_match(&collapsed))
    {
        return None;
    }
    let basename = collapsed
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    if basename.contains(['?', '#'])
        || basename.to_ascii_lowercase().contains(";base64,")
        || !IMAGE_FILENAME.is_match(basename)
    {
        return None;
    }
    let characters = basename.chars().collect::<Vec<_>>();
    if characters.len() <= 256 {
        return Some(basename.to_owned());
    }
    let side = (256 - 1) / 2;
    Some(
        characters[..side]
            .iter()
            .chain(std::iter::once(&'…'))
            .chain(characters[characters.len() - side..].iter())
            .collect(),
    )
}

#[must_use]
pub fn inline_prompt_image_url(media_type: &str, base64_data: &str) -> Option<String> {
    let url = format!("data:{media_type};base64,{base64_data}");
    is_prompt_image_preview_url(&url).then_some(url)
}

#[must_use]
pub fn replayed_turn_images(urls: &[String]) -> Vec<PromptTurnImage> {
    urls.iter()
        .take(PROMPT_IMAGE_LIMITS.max_turn_images)
        .enumerate()
        .map(|(index, src)| PromptTurnImage {
            src: src.clone(),
            label: (index + 1).to_string(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThumbnailSize {
    pub width: u32,
    pub height: u32,
}

#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn prompt_image_thumbnail_size(width: f64, height: f64, max_edge: u32) -> ThumbnailSize {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return ThumbnailSize {
            width: 0,
            height: 0,
        };
    }
    let longest = width.max(height);
    if longest <= f64::from(max_edge) {
        return ThumbnailSize {
            width: width.round() as u32,
            height: height.round() as u32,
        };
    }
    let scale = f64::from(max_edge) / longest;
    ThumbnailSize {
        width: (width * scale).round().max(1.0) as u32,
        height: (height * scale).round().max(1.0) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_only_the_admitted_raster_formats() {
        assert_eq!(
            sniff_prompt_image_format(b"\x89PNG\r\n\x1a\n"),
            Some(PromptImageFormat::Png)
        );
        assert_eq!(
            sniff_prompt_image_format(b"RIFF1234WEBP"),
            Some(PromptImageFormat::Webp)
        );
        assert_eq!(sniff_prompt_image_format(b"RIFF1234WAVE"), None);
        assert_eq!(sniff_prompt_image_format(b"<svg>"), None);
    }

    #[test]
    fn preview_boundary_rejects_svg_remote_and_bad_base64() {
        assert!(is_prompt_image_preview_url("data:image/png;base64,YQ=="));
        assert!(!is_prompt_image_preview_url(
            "data:image/svg+xml;base64,YQ=="
        ));
        assert!(!is_prompt_image_preview_url("https://example.com/a.png"));
        assert!(!is_prompt_image_preview_url(
            "data:image/png;base64,not base64"
        ));
    }

    #[test]
    fn reference_golden_json_preserves_zero_based_field_names() {
        let reference = PromptImageReference {
            text_line: 3,
            text_line_count: 1,
            label: Some("screen.png".into()),
        };
        assert_eq!(
            serde_json::to_value(reference).unwrap(),
            json!({
                "textLine": 3, "textLineCount": 1, "label": "screen.png"
            })
        );
    }

    #[test]
    fn basename_is_local_bounded_and_image_shaped() {
        assert_eq!(
            prompt_image_basename(Some(" C:\\shots\\screen.png ")),
            Some("screen.png".into())
        );
        assert_eq!(
            prompt_image_basename(Some("https://example.com/a.png")),
            None
        );
        assert_eq!(prompt_image_basename(Some("notes.txt")), None);
    }

    #[test]
    fn thumbnail_never_enlarges_and_caps_the_long_edge() {
        assert_eq!(
            prompt_image_thumbnail_size(640.0, 320.0, 320),
            ThumbnailSize {
                width: 320,
                height: 160
            }
        );
        assert_eq!(
            prompt_image_thumbnail_size(40.0, 40.0, 320),
            ThumbnailSize {
                width: 40,
                height: 40
            }
        );
    }
}

//! JSONC comments and trailing commas only. Strings stay byte-for-byte intact.
pub(super) fn parse(text: &str) -> Result<serde_json::Value, &'static str> {
    let mut bytes = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .as_bytes()
        .to_vec();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        if quoted {
            if bytes[index] == b'\\' {
                index += 2;
                continue;
            }
            if bytes[index] == b'"' {
                quoted = false;
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                bytes[index] = b' ';
                index += 1;
            }
            continue;
        } else if bytes[index..].starts_with(b"/*") {
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            while index < bytes.len() && !bytes[index..].starts_with(b"*/") {
                if bytes[index] != b'\n' {
                    bytes[index] = b' ';
                }
                index += 1;
            }
            if index + 1 >= bytes.len() {
                return Err("Unterminated JSONC comment.");
            }
            bytes[index] = b' ';
            bytes[index + 1] = b' ';
            index += 2;
            continue;
        }
        index += 1;
    }
    index = 0;
    quoted = false;
    while index < bytes.len() {
        if quoted {
            if bytes[index] == b'\\' {
                index += 2;
                continue;
            }
            if bytes[index] == b'"' {
                quoted = false;
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes[index] == b','
            && bytes[index + 1..]
                .iter()
                .find(|byte| !byte.is_ascii_whitespace())
                .is_some_and(|byte| matches!(byte, b']' | b'}'))
        {
            bytes[index] = b' ';
        }
        index += 1;
    }
    serde_json::from_slice(&bytes).map_err(|_| "Could not be parsed as JSONC.")
}

#[cfg(test)]
mod tests {
    use super::parse;
    #[test]
    fn jsonc_keeps_comment_shapes_inside_strings_and_removes_only_comments_and_trailing_commas() {
        let value = parse(
            r#"{/* comment */"url":"https://example.com/a//b/*c*/", "args":["a,}",], // comment
        }"#,
        )
        .unwrap();
        assert_eq!(value["url"], "https://example.com/a//b/*c*/");
        assert_eq!(value["args"][0], "a,}");
        assert!(parse("{/* never closed").is_err());
        assert!(parse("{unquoted: 1}").is_err());
    }
}

const ELLIPSIS: &str = "…";
const MIN_KEPT: usize = 2;

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn cap(max: f64) -> usize {
    if max.is_nan() || max <= 0.0 {
        0
    } else if max.is_infinite() {
        usize::MAX
    } else {
        max.floor() as usize
    }
}

fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

fn utf16_prefix(value: &str, units: usize) -> String {
    String::from_utf16_lossy(&value.encode_utf16().take(units).collect::<Vec<_>>())
}

fn utf16_suffix(value: &str, units: usize) -> String {
    if units == 0 {
        return String::new();
    }
    let mut budget = units;
    let mut start = value.len();
    for (idx, ch) in value.char_indices().rev() {
        let cost = ch.len_utf16();
        if budget > cost {
            budget -= cost;
            start = idx;
            continue;
        }
        if budget == cost {
            return value[idx..].to_owned();
        }
        let mut clipped = String::from('\u{FFFD}');
        clipped.push_str(&value[start..]);
        return clipped;
    }
    value.to_owned()
}

#[must_use]
pub fn clip_label(text: &str, max: f64) -> String {
    let cap = cap(max);
    if text.len() <= cap || utf16_len(text) <= cap {
        return text.to_owned();
    }
    if cap == 0 {
        return String::new();
    }
    format!("{}{ELLIPSIS}", utf16_prefix(text, cap - 1))
}

fn omission_marker(omitted: usize) -> String {
    let digits = omitted.to_string();
    let grouped = digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(",");
    format!("\n… {grouped} characters omitted …\n")
}

#[must_use]
pub fn clip_output(text: &str, max: f64) -> String {
    let cap = cap(max);
    if text.len() <= cap {
        return text.to_owned();
    }
    let length = utf16_len(text);
    if length <= cap {
        return text.to_owned();
    }
    let mut marker = omission_marker(length);
    for _ in 0..4 {
        let marker_len = utf16_len(&marker);
        let next = omission_marker(length.saturating_sub(cap.saturating_sub(marker_len)));
        let settled = utf16_len(&next) == marker_len;
        marker = next;
        if settled {
            break;
        }
    }
    let kept = cap.saturating_sub(utf16_len(&marker));
    if kept < MIN_KEPT {
        #[allow(clippy::cast_precision_loss)]
        let label_cap = cap as f64;
        return clip_label(text, label_cap);
    }
    let tail = kept.div_ceil(2);
    format!(
        "{}{}{}",
        utf16_prefix(text, kept - tail),
        marker,
        utf16_suffix(text, tail)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_suffix(value: &str, units: usize) -> String {
        let encoded = value.encode_utf16().collect::<Vec<_>>();
        let start = encoded.len().saturating_sub(units);
        String::from_utf16_lossy(&encoded[start..])
    }

    #[test]
    fn label_marks_the_cut_inside_the_budget() {
        assert_eq!(clip_label("abcdefgh", 5.0), "abcd…");
        assert_eq!(clip_label("abc", 3.9), "abc");
        assert_eq!(clip_label("abc", -1.0), "");
    }

    #[test]
    fn output_keeps_both_ends_and_truthful_omission_count() {
        let clipped = clip_output(&"a".repeat(2_000), 60.0);
        assert!(clipped.starts_with('a'));
        assert!(clipped.ends_with('a'));
        assert!(clipped.contains("1,9"));
        assert!(utf16_len(&clipped) <= 60);
    }

    #[test]
    fn tiny_output_budget_degrades_to_marked_head() {
        assert_eq!(clip_output("abcdef", 3.0), "ab…");
    }

    #[test]
    fn utf16_suffix_preserves_lossy_surrogate_splits() {
        for (value, units) in [
            ("😀", 1),
            ("😀b", 2),
            ("a😀", 2),
            ("a😀b", 1),
            ("a😀b", 3),
            ("abc", 2),
            ("", 4),
        ] {
            assert_eq!(utf16_suffix(value, units), legacy_suffix(value, units));
        }
        assert_eq!(utf16_suffix("😀", 1), "\u{FFFD}");
        assert_eq!(utf16_suffix("😀b", 2), "\u{FFFD}b");
    }

    #[test]
    fn clip_helpers_short_circuit_when_bytes_fit() {
        assert_eq!(clip_label("abc", 3.0), "abc");
        assert_eq!(clip_output("abc", 3.0), "abc");
    }
}

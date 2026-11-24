use std::{fs, os::unix::fs::PermissionsExt};

use super::{
    MAX_THREAD_VISUAL_BYTES, MAX_THREAD_VISUALS, MAX_VISUAL_BYTES, ThreadVisuals, VISUALS_DIR,
    VisualLimits, VisualRefusal, check_visual, looks_like_html, read_visual, retention, visual_id,
};
use crate::thread_journal::thread_hash;

const KEY: &str = "claude#11111111-1111-4111-8111-111111111111";
const OTHER: &str = "codex#22222222-2222-4222-8222-222222222222";
const PAGE: &str = "<!doctype html><html><body><svg viewBox=\"0 0 4 4\"><rect width=\"2\" height=\"3\"/></svg></body></html>";

fn private_temp() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    temp
}

fn id(at: u64) -> String {
    visual_id(at, at.to_string().as_bytes())
}

#[test]
fn a_page_is_refused_for_its_title_its_size_or_not_being_html() {
    assert_eq!(check_visual("Signups by week", PAGE), Ok(()));
    assert_eq!(check_visual("  Chart  ", "<svg></svg>"), Ok(()));
    assert_eq!(check_visual("x", "\u{feff}\n  <div>hi</div>"), Ok(()));
    for title in ["", "   ", "two\nlines", &"t".repeat(121)] {
        assert_eq!(
            check_visual(title, PAGE),
            Err(VisualRefusal::InvalidTitle),
            "{title:?}"
        );
    }
    assert_eq!(check_visual(&"t".repeat(120), PAGE), Ok(()));
    let mut large = String::from("<!doctype html><p>");
    large.push_str(&"a".repeat(MAX_VISUAL_BYTES - large.len()));
    assert_eq!(large.len(), MAX_VISUAL_BYTES);
    assert_eq!(check_visual("Big", &large), Ok(()));
    large.push('é');
    assert_eq!(
        check_visual("Big", &large),
        Err(VisualRefusal::TooLarge {
            bytes: MAX_VISUAL_BYTES + 2
        })
    );
    for text in [
        "",
        "Here is the chart: <svg></svg>",
        "{\"data\": [1, 2]}",
        "< 3 is less",
        "<>",
        "<p>nul\0</p>",
    ] {
        assert!(!looks_like_html(text), "{text:?}");
        assert_eq!(check_visual("x", text), Err(VisualRefusal::NotHtml));
    }
    let codes = [
        VisualRefusal::TooLarge { bytes: 1 }.code(),
        VisualRefusal::NotHtml.code(),
        VisualRefusal::InvalidTitle.code(),
    ];
    assert_eq!(
        codes,
        ["visual-too-large", "visual-not-html", "invalid-input"]
    );
    assert!(
        VisualRefusal::TooLarge { bytes: 600_000 }
            .message()
            .contains("600000 bytes")
    );
}

#[test]
fn ids_sort_by_when_the_visual_was_shown() {
    let early = visual_id(1_731_630_000_500, b"a");
    let late = visual_id(1_731_630_000_501, b"a");
    assert!(interface_contracts::thread_journal::is_visual_id(&early));
    assert!(early < late);
    assert_ne!(visual_id(5, b"a"), visual_id(5, b"b"));
    assert_eq!(visual_id(5, b"a"), visual_id(5, b"a"));
}

#[test]
fn retention_removes_the_oldest_until_the_new_page_fits() {
    let limits = VisualLimits {
        count: 3,
        bytes: 100,
    };
    let pages = |sizes: &[u64]| {
        sizes
            .iter()
            .enumerate()
            .map(|(at, size)| (id(at as u64 + 1), *size))
            .collect::<Vec<_>>()
    };
    assert!(retention(&pages(&[10, 10]), 10, limits).is_empty());
    // A fourth page: the oldest goes, in any listing order.
    let mut three = pages(&[10, 10, 10]);
    three.reverse();
    assert_eq!(retention(&three, 10, limits), [id(1)]);
    // Bytes: the oldest go until the new one fits.
    assert_eq!(retention(&pages(&[40, 40]), 50, limits), [id(1)]);
    assert_eq!(retention(&pages(&[40, 40]), 90, limits), [id(1), id(2)]);
    assert_eq!(retention(&pages(&[60]), 40, limits), Vec::<String>::new());
    let defaults = VisualLimits::default();
    assert_eq!(defaults.count, MAX_THREAD_VISUALS);
    assert_eq!(defaults.bytes, MAX_THREAD_VISUAL_BYTES);
}

#[test]
fn a_saved_page_reads_back_private_and_the_oldest_go_past_the_bounds() {
    let temp = private_temp();
    let mut store = ThreadVisuals::open_with(
        temp.path(),
        VisualLimits {
            count: 2,
            bytes: 1_000_000,
        },
    )
    .unwrap();
    let saved = store.save(KEY, &id(1), PAGE).unwrap();
    assert_eq!(saved.bytes, PAGE.len() as u64);
    assert!(saved.removed.is_empty());
    assert_eq!(read_visual(temp.path(), KEY, &id(1)).as_deref(), Some(PAGE));
    // Another thread's key does not reach it.
    assert_eq!(read_visual(temp.path(), OTHER, &id(1)), None);
    let dir = temp.path().join(VISUALS_DIR).join(thread_hash(KEY));
    let file = dir.join(format!("{}.html", id(1)));
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // The same id twice is refused; the page stays.
    assert!(store.save(KEY, &id(1), "<p>again</p>").is_err());
    assert_eq!(read_visual(temp.path(), KEY, &id(1)).as_deref(), Some(PAGE));
    store.save(KEY, &id(2), "<p>two</p>").unwrap();
    let third = store.save(KEY, &id(3), "<p>three</p>").unwrap();
    assert_eq!(third.removed, [id(1)]);
    assert_eq!(read_visual(temp.path(), KEY, &id(1)), None);
    assert_eq!(
        store
            .pages(KEY)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        [id(2), id(3)]
    );
    // The store never keeps what the rules refuse, whatever its caller did.
    assert!(store.save(KEY, &id(4), "not html").is_err());
    assert!(store.save(KEY, "../escape", PAGE).is_err());
    assert_eq!(store.pages(KEY).unwrap().len(), 2);
}

#[test]
fn a_page_goes_with_its_thread_and_reads_refuse_links_and_bad_ids() {
    let temp = private_temp();
    let mut store = ThreadVisuals::open(temp.path()).unwrap();
    store.save(KEY, &id(1), PAGE).unwrap();
    store.save(OTHER, &id(1), PAGE).unwrap();
    store.remove_thread(KEY).unwrap();
    assert_eq!(read_visual(temp.path(), KEY, &id(1)), None);
    assert!(store.pages(KEY).unwrap().is_empty());
    // Removing it again, or a thread with none, is fine.
    store.remove_thread(KEY).unwrap();
    // Other threads keep theirs.
    assert_eq!(
        read_visual(temp.path(), OTHER, &id(1)).as_deref(),
        Some(PAGE)
    );
    assert_eq!(read_visual(temp.path(), OTHER, "../../x"), None);

    // A link in place of a page is never followed.
    let dir = temp.path().join(VISUALS_DIR).join(thread_hash(OTHER));
    let outside = temp.path().join("outside.html");
    fs::write(&outside, PAGE).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join(format!("{}.html", id(2)))).unwrap();
    assert_eq!(read_visual(temp.path(), OTHER, &id(2)), None);
    // A leftover temporary file of an interrupted save is cleared.
    fs::write(dir.join(format!(".{}.tmp", id(3))), "half").unwrap();
    assert_eq!(store.pages(OTHER).unwrap(), [(id(1), PAGE.len() as u64)]);
    assert!(!dir.join(format!(".{}.tmp", id(3))).exists());
    // One writer: a second store on the same directory does not open.
    assert!(ThreadVisuals::open(temp.path()).is_err());
}

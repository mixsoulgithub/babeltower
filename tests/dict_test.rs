use std::path::Path;

use babeltower::dict;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn builds_and_queries_both_sources() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dict.sqlite");
    let info = dict::build(
        Some(&fixture("ecdict.csv")),
        Some(&fixture("cedict.txt")),
        Some(&out),
    )
    .unwrap();
    assert_eq!(info.en, 2);
    assert_eq!(info.zh, 4);

    let box_entry = dict::lookup_in(&out, "box", 20).unwrap();
    assert_eq!(box_entry.entries.len(), 1);
    let senses = dict::split_senses(&box_entry.entries[0].translation);
    assert!(senses.iter().any(|sense| sense.body.contains("盒子")));
    assert!(senses.iter().any(|sense| sense.body.contains("拳击")));
    let translation = dict::entry_translation(&box_entry.entries[0]);
    assert!(translation.contains("n. 盒子"));
    assert!(translation.contains("vi. 拳击"));

    let plural = dict::lookup_in(&out, "boxes", 20).unwrap();
    assert!(plural.stemmed);
    assert_eq!(plural.entries[0].head, "box");

    let simplified = dict::lookup_in(&out, "银行", 20).unwrap();
    assert_eq!(simplified.entries.len(), 1);
    let traditional = dict::lookup_in(&out, "銀行", 20).unwrap();
    assert!(!traditional.entries.is_empty());

    let multi = dict::lookup_in(&out, "行", 20).unwrap();
    assert!(multi.entries.len() >= 2);
    let pinyins: Vec<&str> = multi
        .entries
        .iter()
        .map(|entry| entry.pinyin.as_str())
        .collect();
    assert!(pinyins.contains(&"xing2"));
    assert!(pinyins.contains(&"hang2"));

    let missing = dict::lookup_in(&out, "notaword", 20).unwrap();
    assert!(missing.entries.is_empty());
}

#[test]
fn missing_database_is_reported() {
    let result = dict::lookup_in(Path::new("/nonexistent/dict.sqlite"), "box", 20).unwrap();
    assert!(result.dict_missing);
    assert!(result.entries.is_empty());
}

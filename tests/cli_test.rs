use std::path::Path;
use std::process::Command;

use babeltower::dict;

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("words.csv");
    let dict = dir.path().join("dict.sqlite");
    std::fs::write(
        &csv,
        "id,word,translation,hit,last\n1,hello,你好,4,08-16\n2,具体的,Specific,1,08-17\n",
    )
    .unwrap();
    dict::build(
        Some(&fixture("ecdict.csv")),
        Some(&fixture("cedict.txt")),
        Some(&dict),
    )
    .unwrap();
    (dir, csv, dict)
}

fn run(dir: &Path, csv: &Path, dict: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_bbt"))
        .args(args)
        .env("BBT_CSV", csv)
        .env("BBT_DICT", dict)
        .env("BBT_CONFIG_DIR", dir.join("config"))
        .output()
        .unwrap()
}

#[test]
fn lists_migrates_and_records() {
    let (dir, csv, dict) = setup();
    let list = run(dir.path(), &csv, &dict, &["-l"]);
    assert!(list.status.success());
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.starts_with("hit,word,translation,last"));
    assert!(text.contains("hello,你好"));

    let recorded = run(dir.path(), &csv, &dict, &["-r", "box"]);
    assert!(recorded.status.success(), "{:?}", recorded);
    let list = run(dir.path(), &csv, &dict, &["-l"]);
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("box"));
    assert!(text.contains("盒子"));
    assert!(text.contains("拳击"));
    assert!(std::fs::read_to_string(&csv).unwrap().starts_with("uuid,"));
}

#[test]
fn formats_and_unknown_words() {
    let (dir, csv, dict) = setup();
    let output = run(dir.path(), &csv, &dict, &["-f", "box"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("box"));
    assert!(text.contains("盒子"));

    let missing = run(dir.path(), &csv, &dict, &["notaword"]);
    assert!(!missing.status.success());
    let error = String::from_utf8(missing.stderr).unwrap();
    assert!(error.contains("not found"));
}

#[test]
fn merges_and_imports_v1() {
    let (dir, csv, dict) = setup();
    let other = dir.path().join("other.csv");
    std::fs::write(
        &other,
        "id,word,translation,hit,last\n9,mergeword,合并词,3,08-20\n",
    )
    .unwrap();
    let output = run(
        dir.path(),
        &csv,
        &dict,
        &["merge", other.to_str().unwrap(), "--dry-run"],
    );
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("added 1")
    );
    let list = run(dir.path(), &csv, &dict, &["-l"]);
    assert!(
        !String::from_utf8(list.stdout)
            .unwrap()
            .contains("mergeword")
    );

    let output = run(
        dir.path(),
        &csv,
        &dict,
        &["import", other.to_str().unwrap()],
    );
    assert!(output.status.success());
    let list = run(dir.path(), &csv, &dict, &["-l"]);
    assert!(
        String::from_utf8(list.stdout)
            .unwrap()
            .contains("mergeword")
    );
}

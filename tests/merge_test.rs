use std::path::Path;

use babeltower::merge;
use babeltower::store::{HEADER, Store, WordRow};

const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
const C: &str = "33333333-3333-4333-8333-333333333333";

fn row(uuid: &str, word: &str, updated: i64, origin: &str) -> WordRow {
    WordRow {
        uuid: uuid.to_string(),
        word: word.to_string(),
        translation: format!("{word}-tr"),
        phonetic: String::new(),
        tags: String::new(),
        hit: 1,
        last: String::new(),
        box_: 1,
        due: String::new(),
        reviews: 0,
        reviewed: String::new(),
        updated,
        origin: origin.to_string(),
        deleted: 0,
    }
}

fn write_rows(path: &Path, rows: &[WordRow]) {
    let mut writer = csv::WriterBuilder::new().from_path(path).unwrap();
    writer.write_record(HEADER).unwrap();
    for row in rows {
        writer
            .write_record([
                row.uuid.as_str(),
                row.word.as_str(),
                row.translation.as_str(),
                row.phonetic.as_str(),
                row.tags.as_str(),
                &row.hit.to_string(),
                row.last.as_str(),
                &row.box_.to_string(),
                row.due.as_str(),
                &row.reviews.to_string(),
                row.reviewed.as_str(),
                &row.updated.to_string(),
                row.origin.as_str(),
                &row.deleted.to_string(),
            ])
            .unwrap();
    }
    writer.flush().unwrap();
}

fn setup(local: &[WordRow], remote: &[WordRow]) -> (tempfile::TempDir, Store, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let local_path = dir.path().join("local.csv");
    let remote_path = dir.path().join("remote.csv");
    write_rows(&local_path, local);
    write_rows(&remote_path, remote);
    let store = Store::open(Some(local_path)).unwrap();
    (dir, store, remote_path)
}

#[test]
fn merges_union_and_keeps_counters() {
    let local = vec![row(A, "alpha", 10, "l"), row(B, "beta", 10, "l")];
    let mut remote = vec![row(A, "alpha", 20, "r"), row(C, "gamma", 10, "r")];
    remote[0].hit = 9;
    remote[0].translation = "alpha-new".to_string();
    let (_dir, store, remote_path) = setup(&local, &remote);
    let stats = merge::merge(&store, &remote_path, false, None).unwrap();
    assert_eq!(stats.added, 1);
    assert_eq!(stats.updated, 1);
    let rows = store.all_rows().unwrap();
    assert_eq!(rows.len(), 3);
    let alpha = rows.iter().find(|row| row.uuid == A).unwrap();
    assert_eq!(alpha.translation, "alpha-new");
    assert_eq!(alpha.hit, 9);
    assert_eq!(alpha.updated, 20);
}

#[test]
fn tombstone_wins_older_edit_and_new_edit_revives() {
    let mut local = vec![row(A, "alpha", 100, "l")];
    local[0].deleted = 1;
    let remote = vec![row(A, "alpha", 50, "r")];
    let (_dir, store, remote_path) = setup(&local, &remote);
    merge::merge(&store, &remote_path, false, None).unwrap();
    assert_eq!(store.all_rows().unwrap()[0].deleted, 1);

    let mut remote_new = vec![row(A, "alpha", 200, "r")];
    remote_new[0].deleted = 0;
    remote_new[0].translation = "revived".to_string();
    write_rows(&remote_path, &remote_new);
    merge::merge(&store, &remote_path, false, None).unwrap();
    let rows = store.all_rows().unwrap();
    assert_eq!(rows[0].deleted, 0);
    assert_eq!(rows[0].translation, "revived");
}

#[test]
fn merge_is_idempotent() {
    let local = vec![row(A, "alpha", 10, "l")];
    let remote = vec![row(A, "alpha", 20, "r"), row(B, "beta", 10, "r")];
    let (_dir, store, remote_path) = setup(&local, &remote);
    merge::merge(&store, &remote_path, false, None).unwrap();
    let stats = merge::merge(&store, &remote_path, false, None).unwrap();
    assert_eq!(stats.added, 0);
    assert_eq!(stats.updated, 0);
    assert_eq!(stats.conflicts, 0);
    assert_eq!(stats.unchanged, 2);
}

#[test]
fn dry_run_does_not_write() {
    let local = vec![row(A, "alpha", 10, "l")];
    let remote = vec![row(B, "beta", 10, "r")];
    let (_dir, store, remote_path) = setup(&local, &remote);
    let before = std::fs::read_to_string(&store.path).unwrap();
    let stats = merge::merge(&store, &remote_path, true, None).unwrap();
    let after = std::fs::read_to_string(&store.path).unwrap();
    assert_eq!(stats.added, 1);
    assert_eq!(before, after);
}

#[test]
fn v1_remote_file_can_be_merged() {
    let local = vec![row(A, "alpha", 10, "l")];
    let dir = tempfile::tempdir().unwrap();
    let local_path = dir.path().join("local.csv");
    let remote_path = dir.path().join("v1.csv");
    write_rows(&local_path, &local);
    std::fs::write(
        &remote_path,
        "id,word,translation,hit,last\n1,beta,beta-tr,2,08-17\n",
    )
    .unwrap();
    let store = Store::open(Some(local_path)).unwrap();
    let stats = merge::merge(&store, &remote_path, false, None).unwrap();
    assert_eq!(stats.added, 1);
    let rows = store.all_rows().unwrap();
    assert!(rows.iter().any(|row| row.word == "beta"));
}

#[test]
fn duplicate_words_mark_loser_deleted() {
    let local = vec![row(A, "same", 10, "l")];
    let remote = vec![row(B, "same", 20, "r")];
    let (_dir, store, remote_path) = setup(&local, &remote);
    let stats = merge::merge(&store, &remote_path, false, None).unwrap();
    assert_eq!(stats.conflicts, 1);
    let rows = store.all_rows().unwrap();
    let deleted = rows.iter().filter(|row| row.deleted != 0).count();
    assert_eq!(deleted, 1);
    assert_eq!(rows.iter().find(|row| row.deleted == 0).unwrap().uuid, B);
}

#[test]
fn gc_only_removes_old_tombstones_with_force() {
    let mut old = row(A, "old", babeltower::store::now_unix() - 200 * 86_400, "l");
    old.deleted = 1;
    let live = row(B, "live", 10, "l");
    let (dir, store, _remote) = setup(&[old, live], &[]);
    let count = merge::gc(&store, 90, false).unwrap();
    assert_eq!(count, 1);
    assert_eq!(store.all_rows().unwrap().len(), 2);
    let removed = merge::gc(&store, 90, true).unwrap();
    assert_eq!(removed, 1);
    let rows = store.all_rows().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].word, "live");
    let _ = dir;
}

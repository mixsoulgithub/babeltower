use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use csv::WriterBuilder;

use crate::store::{self, Store, WordRow};

static QUIET: AtomicBool = AtomicBool::new(false);

pub fn set_quiet(value: bool) {
    QUIET.store(value, Ordering::Relaxed);
}

fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

#[derive(Clone, Debug, Default)]
pub struct MergeStats {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub conflicts: usize,
}

#[derive(Clone, Debug)]
struct Conflict {
    uuid: String,
    reason: String,
    local_updated: i64,
    remote_updated: i64,
}

pub fn merge(
    store: &Store,
    other_path: &Path,
    dry_run: bool,
    report: Option<&Path>,
) -> Result<MergeStats> {
    let other =
        store::read_rows(other_path).with_context(|| format!("read {}", other_path.display()))?;
    let (stats, conflicts) = if dry_run {
        let local = store.all_rows()?;
        let (_, stats, conflicts) = merge_rows(&local, &other.rows);
        (stats, conflicts)
    } else {
        store.mutate(|rows| {
            let (merged, stats, conflicts) = merge_rows(rows, &other.rows);
            *rows = merged;
            Ok((stats, conflicts))
        })?
    };
    if let Some(path) = report {
        write_report(path, &conflicts)?;
    }
    if !quiet() {
        eprintln!(
            "merge: added {} updated {} unchanged {} conflicts {}",
            stats.added, stats.updated, stats.unchanged, stats.conflicts
        );
    }
    Ok(stats)
}

pub fn import(
    store: &Store,
    path: &Path,
    dry_run: bool,
    report: Option<&Path>,
) -> Result<MergeStats> {
    merge(store, path, dry_run, report)
}

pub fn gc(store: &Store, days: i64, force: bool) -> Result<usize> {
    let cutoff = store::now_unix() - days.max(0) * 86_400;
    if !force {
        let count = store
            .all_rows()?
            .iter()
            .filter(|row| row.deleted != 0 && row.updated < cutoff)
            .count();
        if !quiet() {
            eprintln!("gc: {count} tombstones older than {days} days (use --force)");
        }
        return Ok(count);
    }
    let mut removed = 0usize;
    store.mutate(|rows| {
        let before = rows.len();
        rows.retain(|row| !(row.deleted != 0 && row.updated < cutoff));
        removed = before - rows.len();
        Ok(())
    })?;
    if !quiet() {
        eprintln!("gc: removed {removed} tombstones");
    }
    Ok(removed)
}

fn merge_rows(local: &[WordRow], remote: &[WordRow]) -> (Vec<WordRow>, MergeStats, Vec<Conflict>) {
    let mut stats = MergeStats::default();
    let mut conflicts = Vec::new();
    let mut remote_by_uuid: HashMap<String, WordRow> = remote
        .iter()
        .cloned()
        .map(|row| (row.uuid.clone(), row))
        .collect();
    let mut merged: Vec<WordRow> = Vec::new();
    for row in local {
        if let Some(other) = remote_by_uuid.remove(&row.uuid) {
            let (result, conflict) = merge_pair(row, &other);
            if result != *row {
                stats.updated += 1;
            } else {
                stats.unchanged += 1;
            }
            if let Some(conflict) = conflict {
                conflicts.push(conflict);
            }
            merged.push(result);
        } else {
            stats.unchanged += 1;
            merged.push(row.clone());
        }
    }
    for (_, row) in remote_by_uuid {
        stats.added += 1;
        merged.push(row);
    }
    let duplicate_conflicts = enforce_word_unique(&mut merged);
    stats.conflicts = conflicts.len() + duplicate_conflicts.len();
    conflicts.extend(duplicate_conflicts);
    merged.sort_by(|a, b| a.uuid.cmp(&b.uuid));
    (merged, stats, conflicts)
}

fn merge_pair(local: &WordRow, remote: &WordRow) -> (WordRow, Option<Conflict>) {
    let local_key = (local.updated, local.origin.clone());
    let remote_key = (remote.updated, remote.origin.clone());
    let tie = local_key == remote_key;
    let content_diff = content_differs(local, remote);
    let mut conflict = None;
    if tie && content_diff {
        conflict = Some(Conflict {
            uuid: local.uuid.clone(),
            reason: if local.origin == remote.origin {
                "tie_content".to_string()
            } else {
                "tie_origin".to_string()
            },
            local_updated: local.updated,
            remote_updated: remote.updated,
        });
    }
    let winner = if remote_key > local_key {
        remote
    } else {
        local
    };
    if local_key < remote_key && content_diff {
        // 新版本覆盖旧版本是正常 LWW，不算冲突。
    }
    let mut result = winner.clone();
    result.hit = local.hit.max(remote.hit);
    result.reviews = local.reviews.max(remote.reviews);
    result.last = local.last.clone().max(remote.last.clone());
    result.reviewed = local.reviewed.clone().max(remote.reviewed.clone());
    (result, conflict)
}

fn content_differs(left: &WordRow, right: &WordRow) -> bool {
    left.word != right.word
        || left.translation != right.translation
        || left.phonetic != right.phonetic
        || left.tags != right.tags
        || left.box_ != right.box_
        || left.due != right.due
        || left.deleted != right.deleted
}

fn enforce_word_unique(rows: &mut [WordRow]) -> Vec<Conflict> {
    let mut conflicts = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for index in 0..rows.len() {
        if rows[index].deleted != 0 {
            continue;
        }
        let key = store::casefold(&rows[index].word);
        if let Some(previous) = seen.get(&key).copied() {
            let previous_key = (rows[previous].updated, rows[previous].origin.clone());
            let current_key = (rows[index].updated, rows[index].origin.clone());
            let winner = if current_key > previous_key {
                index
            } else {
                previous
            };
            let loser = if winner == index { previous } else { index };
            rows[loser].deleted = 1;
            rows[loser].updated = store::now_unix();
            conflicts.push(Conflict {
                uuid: rows[loser].uuid.clone(),
                reason: "duplicate_word".to_string(),
                local_updated: rows[loser].updated,
                remote_updated: rows[winner].updated,
            });
            seen.insert(key, winner);
        } else {
            seen.insert(key, index);
        }
    }
    conflicts
}

fn write_report(path: &Path, conflicts: &[Conflict]) -> Result<()> {
    let mut writer = WriterBuilder::new().from_path(path)?;
    writer.write_record(["uuid", "reason", "local_updated", "remote_updated"])?;
    for conflict in conflicts {
        let record = vec![
            conflict.uuid.clone(),
            conflict.reason.clone(),
            conflict.local_updated.to_string(),
            conflict.remote_updated.to_string(),
        ];
        writer.write_record(record.iter().map(String::as_str))?;
    }
    writer.flush()?;
    Ok(())
}

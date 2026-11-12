use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate};
use csv::{ReaderBuilder, WriterBuilder};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const HEADER: [&str; 14] = [
    "uuid",
    "word",
    "translation",
    "phonetic",
    "tags",
    "hit",
    "last",
    "box",
    "due",
    "reviews",
    "reviewed",
    "updated",
    "origin",
    "deleted",
];

pub const BOX_INTERVALS: [i64; 6] = [0, 1, 2, 4, 8, 16];
const PROJECT_NAMESPACE: &str = "d3a9b1f0-7c2e-4a6b-9c1d-2f8e5a4b6c7d";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordRow {
    pub uuid: String,
    pub word: String,
    pub translation: String,
    pub phonetic: String,
    pub tags: String,
    pub hit: u64,
    pub last: String,
    pub box_: u8,
    pub due: String,
    pub reviews: u64,
    pub reviewed: String,
    pub updated: i64,
    pub origin: String,
    pub deleted: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    Again,
    Good,
}

#[derive(Clone, Debug, Default)]
pub struct WordUpdate {
    pub word: Option<String>,
    pub translation: Option<String>,
    pub phonetic: Option<String>,
    pub tags: Option<String>,
    pub box_: Option<u8>,
    pub due: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub total: u64,
    pub new: u64,
    pub due: u64,
    pub reviewed_today: u64,
    pub by_box: [u64; 5],
    pub hits: u64,
    pub top: Vec<WordRow>,
}

#[derive(Clone, Debug)]
pub struct ReadOutcome {
    pub rows: Vec<WordRow>,
    pub dirty: bool,
    pub legacy: bool,
}

#[derive(Clone, Debug)]
pub struct Store {
    pub path: PathBuf,
    pub origin: String,
}

impl Store {
    pub fn open(path: Option<PathBuf>) -> Result<Self> {
        let path = path.unwrap_or_else(default_csv_path);
        let store = Self {
            path,
            origin: origin_id(),
        };
        store.ensure_parent()?;
        if !store.path.exists() {
            let lock = store.lock()?;
            store.write_rows_unlocked(&[])?;
            drop(lock);
            return Ok(store);
        }
        let outcome = read_rows(&store.path)?;
        if outcome.legacy && !backup_path(&store.path).exists() {
            fs::copy(&store.path, backup_path(&store.path))
                .with_context(|| format!("backup {}", store.path.display()))?;
        }
        if outcome.dirty {
            let lock = store.lock()?;
            store.write_rows_unlocked(&outcome.rows)?;
            drop(lock);
        }
        Ok(store)
    }

    fn ensure_parent(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<File> {
        let lock_path = PathBuf::from(format!("{}.lock", self.path.display()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open lock {}", lock_path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("lock {}", lock_path.display()))?;
        Ok(file)
    }

    pub fn load(&self) -> Result<Vec<WordRow>> {
        Ok(read_rows(&self.path)?.rows)
    }

    pub fn words(&self) -> Result<Vec<WordRow>> {
        Ok(self
            .load()?
            .into_iter()
            .filter(|row| row.deleted == 0)
            .collect())
    }

    pub fn upsert(
        &self,
        word: &str,
        translation: &str,
        phonetic: Option<&str>,
        tags: Option<&str>,
    ) -> Result<(WordRow, bool)> {
        let word = word.trim();
        if word.is_empty() {
            bail!("word required");
        }
        let translation = translation.trim();
        self.mutate(|rows| {
            let key = casefold(word);
            if let Some(index) = rows.iter().position(|row| casefold(&row.word) == key) {
                let row = &mut rows[index];
                if row.deleted != 0 {
                    row.deleted = 0;
                }
                if !translation.is_empty() {
                    row.translation = translation.to_string();
                }
                if let Some(value) = phonetic {
                    if !value.trim().is_empty() {
                        row.phonetic = value.trim().to_string();
                    }
                }
                if let Some(value) = tags {
                    if !value.trim().is_empty() {
                        row.tags = value.trim().to_string();
                    }
                }
                row.hit += 1;
                row.last = today();
                row.updated = now_unix();
                let result = row.clone();
                Ok((result, false))
            } else {
                let now = now_unix();
                let row = WordRow {
                    uuid: uuid_for(word),
                    word: word.to_string(),
                    translation: translation.to_string(),
                    phonetic: phonetic.unwrap_or("").trim().to_string(),
                    tags: tags.unwrap_or("").trim().to_string(),
                    hit: 1,
                    last: today(),
                    box_: 1,
                    due: String::new(),
                    reviews: 0,
                    reviewed: String::new(),
                    updated: now,
                    origin: self.origin.clone(),
                    deleted: 0,
                };
                rows.push(row.clone());
                Ok((row, true))
            }
        })
    }

    pub fn update(&self, uuid: &str, fields: WordUpdate) -> Result<WordRow> {
        self.mutate(|rows| {
            let index = rows
                .iter()
                .position(|row| row.uuid == uuid && row.deleted == 0)
                .with_context(|| "word not found")?;
            if let Some(word) = fields.word.as_ref() {
                let word = word.trim();
                if word.is_empty() {
                    bail!("word required");
                }
                let key = casefold(word);
                if rows
                    .iter()
                    .enumerate()
                    .any(|(i, row)| i != index && row.deleted == 0 && casefold(&row.word) == key)
                {
                    bail!("word already exists");
                }
                rows[index].word = word.to_string();
            }
            if let Some(value) = fields.translation {
                rows[index].translation = value.trim().to_string();
            }
            if let Some(value) = fields.phonetic {
                rows[index].phonetic = value.trim().to_string();
            }
            if let Some(value) = fields.tags {
                rows[index].tags = value.trim().to_string();
            }
            if let Some(value) = fields.box_ {
                rows[index].box_ = value.clamp(1, 5);
            }
            if let Some(value) = fields.due {
                rows[index].due = normalize_date(&value);
            }
            rows[index].updated = now_unix();
            Ok(rows[index].clone())
        })
    }

    pub fn delete(&self, uuid: &str) -> Result<()> {
        self.mutate(|rows| {
            let row = rows
                .iter_mut()
                .find(|row| row.uuid == uuid && row.deleted == 0)
                .with_context(|| "word not found")?;
            row.deleted = 1;
            row.updated = now_unix();
            Ok(())
        })
    }

    pub fn review(&self, uuid: &str, grade: Grade) -> Result<WordRow> {
        self.mutate(|rows| {
            let row = rows
                .iter_mut()
                .find(|row| row.uuid == uuid && row.deleted == 0)
                .with_context(|| "word not found")?;
            match grade {
                Grade::Again => {
                    row.box_ = 1;
                    row.due = today();
                }
                Grade::Good => {
                    let interval = BOX_INTERVALS.get(row.box_ as usize).copied().unwrap_or(16);
                    row.due = add_days(&today(), interval);
                    row.box_ = (row.box_ + 1).min(5);
                }
            }
            row.reviews += 1;
            row.reviewed = today();
            row.updated = now_unix();
            Ok(row.clone())
        })
    }

    pub fn queue(&self, limit: usize, new_limit: usize) -> Result<Vec<WordRow>> {
        let mut rows: Vec<WordRow> = self
            .load()?
            .into_iter()
            .filter(|row| row.deleted == 0)
            .collect();
        let current = today();
        let mut due: Vec<WordRow> = rows
            .iter()
            .filter(|row| !row.due.is_empty() && row.due <= current)
            .cloned()
            .collect();
        due.sort_by(|a, b| {
            (a.due.as_str(), a.word.as_str()).cmp(&(b.due.as_str(), b.word.as_str()))
        });
        let mut new: Vec<WordRow> = rows.drain(..).filter(|row| row.due.is_empty()).collect();
        new.sort_by(|a, b| (a.updated, a.word.as_str()).cmp(&(b.updated, b.word.as_str())));
        let mut selected = due;
        selected.truncate(limit);
        let room = limit.saturating_sub(selected.len());
        if room > 0 {
            selected.extend(new.into_iter().take(new_limit.min(room)));
        }
        Ok(selected)
    }

    pub fn stats(&self) -> Result<Stats> {
        let rows: Vec<WordRow> = self
            .load()?
            .into_iter()
            .filter(|row| row.deleted == 0)
            .collect();
        let current = today();
        let mut stats = Stats {
            total: rows.len() as u64,
            new: rows.iter().filter(|row| row.due.is_empty()).count() as u64,
            due: rows
                .iter()
                .filter(|row| !row.due.is_empty() && row.due <= current)
                .count() as u64,
            reviewed_today: rows.iter().filter(|row| row.reviewed == current).count() as u64,
            hits: rows.iter().map(|row| row.hit).sum(),
            by_box: [0; 5],
            top: Vec::new(),
        };
        for row in &rows {
            let index = (row.box_.clamp(1, 5) - 1) as usize;
            stats.by_box[index] += 1;
        }
        let mut top = rows;
        top.sort_by(|a, b| (b.hit, b.last.as_str()).cmp(&(a.hit, a.last.as_str())));
        top.truncate(5);
        stats.top = top;
        Ok(stats)
    }

    pub fn all_rows(&self) -> Result<Vec<WordRow>> {
        self.load()
    }

    pub fn mutate<T>(&self, f: impl FnOnce(&mut Vec<WordRow>) -> Result<T>) -> Result<T> {
        let lock = self.lock()?;
        let outcome = read_rows(&self.path)?;
        let mut rows = outcome.rows;
        let previous = rows.clone();
        let value = f(&mut rows)?;
        dedupe_all(&mut rows, outcome.legacy);
        if rows != previous {
            self.write_rows_unlocked(&rows)?;
        }
        drop(lock);
        Ok(value)
    }

    fn write_rows_unlocked(&self, rows: &[WordRow]) -> Result<()> {
        let mut rows = rows.to_vec();
        rows.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        let directory = self.path.parent().unwrap_or_else(|| Path::new("."));
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or(0);
        let tmp = directory.join(format!(".words.{}.{}.csv.tmp", std::process::id(), suffix));
        let result = (|| -> Result<()> {
            let mut writer = WriterBuilder::new().from_path(&tmp)?;
            writer.write_record(HEADER)?;
            for row in &rows {
                let record = vec![
                    row.uuid.clone(),
                    row.word.clone(),
                    row.translation.clone(),
                    row.phonetic.clone(),
                    row.tags.clone(),
                    row.hit.to_string(),
                    row.last.clone(),
                    row.box_.to_string(),
                    row.due.clone(),
                    row.reviews.to_string(),
                    row.reviewed.clone(),
                    row.updated.to_string(),
                    row.origin.clone(),
                    row.deleted.to_string(),
                ];
                writer.write_record(record.iter().map(String::as_str))?;
            }
            writer.flush()?;
            let file = writer.into_inner()?;
            file.sync_all()?;
            fs::rename(&tmp, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

pub fn read_rows(path: &Path) -> Result<ReadOutcome> {
    if !path.exists() {
        return Ok(ReadOutcome {
            rows: Vec::new(),
            dirty: true,
            legacy: false,
        });
    }
    let mut reader = ReaderBuilder::new().flexible(true).from_path(path)?;
    let headers = reader.headers()?.clone();
    let canonical = headers.iter().map(str::trim).eq(HEADER.iter().copied());
    let index: HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, name)| (name.trim().to_lowercase(), i))
        .collect();
    let legacy = !index.contains_key("uuid");
    let origin = origin_id();
    let now = now_unix();
    let mut dirty = legacy || !canonical;
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record?;
        let get = |name: &str| -> String {
            index
                .get(name)
                .and_then(|i| record.get(*i))
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let word = get("word");
        if word.is_empty() {
            dirty = true;
            continue;
        }
        let uuid_raw = get("uuid");
        let uuid = if uuid_raw.is_empty() || Uuid::parse_str(&uuid_raw).is_err() {
            dirty = true;
            uuid_for(&word)
        } else {
            uuid_raw
        };
        let last_raw = get("last");
        let due_raw = get("due");
        let reviewed_raw = get("reviewed");
        let last = normalize_date(&last_raw);
        let due = normalize_date(&due_raw);
        let reviewed = normalize_date(&reviewed_raw);
        if last != last_raw || due != due_raw || reviewed != reviewed_raw {
            dirty = true;
        }
        let box_raw = get("box");
        let box_ = box_raw.parse::<i64>().unwrap_or(1).clamp(1, 5) as u8;
        if box_raw.parse::<i64>().ok() != Some(box_ as i64) {
            dirty = true;
        }
        let hit_raw = get("hit");
        let hit = hit_raw.parse::<u64>().unwrap_or(0);
        if hit_raw.parse::<u64>().ok() != Some(hit) {
            dirty = true;
        }
        let reviews_raw = get("reviews");
        let reviews = reviews_raw.parse::<u64>().unwrap_or(0);
        if reviews_raw.parse::<u64>().ok() != Some(reviews) {
            dirty = true;
        }
        let updated_raw = get("updated");
        let updated = if legacy || updated_raw.is_empty() {
            now
        } else {
            updated_raw.parse::<i64>().unwrap_or_else(|_| {
                dirty = true;
                0
            })
        };
        let origin_raw = get("origin");
        let origin = if origin_raw.is_empty() {
            if !legacy {
                dirty = true;
            }
            origin.clone()
        } else {
            origin_raw
        };
        let deleted_raw = get("deleted");
        let deleted = deleted_raw.parse::<u8>().unwrap_or(0).min(1);
        if deleted_raw.parse::<u8>().ok() != Some(deleted) {
            dirty = true;
        }
        let row = WordRow {
            uuid,
            word,
            translation: get("translation"),
            phonetic: get("phonetic"),
            tags: get("tags"),
            hit,
            last,
            box_,
            due,
            reviews,
            reviewed,
            updated,
            origin,
            deleted,
        };
        rows.push(row);
    }
    let before = rows.len();
    dedupe_all(&mut rows, legacy);
    if rows.len() != before {
        dirty = true;
    }
    Ok(ReadOutcome {
        rows,
        dirty,
        legacy,
    })
}

fn dedupe_all(rows: &mut Vec<WordRow>, legacy: bool) {
    if legacy {
        dedupe_word(rows, true);
        dedupe_uuid(rows);
    } else {
        dedupe_uuid(rows);
        dedupe_word(rows, false);
    }
}

fn dedupe_uuid(rows: &mut Vec<WordRow>) -> bool {
    let mut map: HashMap<String, usize> = HashMap::new();
    let mut output: Vec<WordRow> = Vec::new();
    let mut changed = false;
    for row in rows.drain(..) {
        if let Some(index) = map.get(&row.uuid).copied() {
            changed = true;
            merge_same_uuid(&mut output[index], row);
        } else {
            map.insert(row.uuid.clone(), output.len());
            output.push(row);
        }
    }
    *rows = output;
    changed
}

fn merge_same_uuid(target: &mut WordRow, other: WordRow) {
    let target_key = (target.updated, target.origin.clone());
    let other_key = (other.updated, other.origin.clone());
    let hit = target.hit.max(other.hit);
    let reviews = target.reviews.max(other.reviews);
    let last = target.last.clone().max(other.last.clone());
    let reviewed = target.reviewed.clone().max(other.reviewed.clone());
    let winner = if other_key > target_key {
        other
    } else {
        target.clone()
    };
    *target = winner;
    target.hit = hit;
    target.reviews = reviews;
    target.last = last;
    target.reviewed = reviewed;
}

fn dedupe_word(rows: &mut Vec<WordRow>, legacy: bool) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut remove: HashSet<usize> = HashSet::new();
    for index in 0..rows.len() {
        if rows[index].deleted != 0 {
            continue;
        }
        let key = casefold(&rows[index].word);
        if let Some(previous) = seen.get(&key).copied() {
            if legacy {
                let previous_hit = rows[previous].hit;
                let current_hit = rows[index].hit;
                rows[previous].hit = previous_hit + current_hit;
                if current_hit > previous_hit {
                    let mut replacement = rows[index].clone();
                    replacement.hit = rows[previous].hit;
                    rows[previous] = replacement;
                }
                remove.insert(index);
            } else {
                let previous_key = (rows[previous].updated, rows[previous].origin.clone());
                let current_key = (rows[index].updated, rows[index].origin.clone());
                let winner = if current_key > previous_key {
                    index
                } else {
                    previous
                };
                let loser = if winner == index { previous } else { index };
                rows[loser].deleted = 1;
                rows[loser].updated = now_unix();
                seen.insert(key, winner);
            }
        } else {
            seen.insert(key, index);
        }
    }
    if !remove.is_empty() {
        let mut index = 0usize;
        rows.retain(|_| {
            let keep = !remove.contains(&index);
            index += 1;
            keep
        });
    }
}

pub fn default_csv_path() -> PathBuf {
    if let Ok(value) = std::env::var("BBT_CSV") {
        return PathBuf::from(value);
    }
    home_dir().join(".local/share/words/words.csv")
}

pub fn data_dir() -> PathBuf {
    if let Ok(value) = std::env::var("BBT_DATA_DIR") {
        return PathBuf::from(value);
    }
    home_dir().join(".local/share/babeltower")
}

pub fn config_dir() -> PathBuf {
    if let Ok(value) = std::env::var("BBT_CONFIG_DIR") {
        return PathBuf::from(value);
    }
    home_dir().join(".config/bbt")
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.v1.bak", path.display()))
}

pub fn today() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

pub fn add_days(date: &str, days: i64) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|date| {
            (date + ChronoDuration::days(days))
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_else(|_| today())
}

pub fn normalize_date(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    if NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() {
        return value.to_string();
    }
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 2 {
        return String::new();
    }
    let now = Local::now();
    let candidate_text = format!("{}-{}-{}", now.year(), parts[0], parts[1]);
    let Ok(candidate) = NaiveDate::parse_from_str(&candidate_text, "%Y-%m-%d") else {
        return String::new();
    };
    let today = now.date_naive();
    let candidate = if candidate > today {
        NaiveDate::from_ymd_opt(candidate.year() - 1, candidate.month(), candidate.day())
            .unwrap_or(candidate)
    } else {
        candidate
    };
    candidate.format("%Y-%m-%d").to_string()
}

pub fn casefold(word: &str) -> String {
    word.trim().to_lowercase()
}

pub fn uuid_for(word: &str) -> String {
    let namespace = Uuid::parse_str(PROJECT_NAMESPACE).expect("valid namespace");
    Uuid::new_v5(
        &namespace,
        format!("babeltower:{}", casefold(word)).as_bytes(),
    )
    .hyphenated()
    .to_string()
}

pub fn origin_id() -> String {
    if let Ok(value) = std::env::var("BBT_ORIGIN") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    let source = fs::read_to_string("/etc/machine-id")
        .or_else(|_| fs::read_to_string("/etc/hostname"))
        .unwrap_or_else(|_| "unknown".to_string());
    let digest = Sha256::digest(source.trim().as_bytes());
    format!("{digest:x}")[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, text: &str) {
        let mut file = File::create(path).unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn migrates_v1_duplicate_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("words.csv");
        write_file(
            &path,
            "id,word,translation,hit,last\n3,hello,你好,4,08-16\n3,hello,你好啊,2,08-17\n7,box,盒子,1,08-17\n",
        );
        let outcome = read_rows(&path).unwrap();
        assert!(outcome.legacy);
        assert_eq!(outcome.rows.len(), 2);
        let hello = outcome.rows.iter().find(|row| row.word == "hello").unwrap();
        assert_eq!(hello.hit, 6);
        assert_eq!(hello.translation, "你好");
        assert_eq!(hello.box_, 1);
        assert_eq!(hello.due, "");
        assert_eq!(hello.reviews, 0);
        assert!(hello.last.starts_with("20"));
    }

    #[test]
    fn roundtrips_v2_and_reviews() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("words.csv");
        write_file(&path, &format!("{}\n", HEADER.join(",")));
        let store = Store::open(Some(path.clone())).unwrap();
        let (row, created) = store
            .upsert("box", "n. 盒子", Some("/boks/"), Some("cet4"))
            .unwrap();
        assert!(created);
        assert_eq!(row.hit, 1);
        let (row, created) = store.upsert("box", "n. 盒子", None, None).unwrap();
        assert!(!created);
        assert_eq!(row.hit, 2);
        let good = store.review(&row.uuid, Grade::Good).unwrap();
        assert_eq!(good.box_, 2);
        assert_eq!(good.reviews, 1);
        assert_eq!(store.queue(20, 10).unwrap().len(), 0);
        let again = store.review(&row.uuid, Grade::Again).unwrap();
        assert_eq!(again.box_, 1);
        assert_eq!(store.queue(20, 10).unwrap().len(), 1);
        let stats = store.stats().unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.hits, 2);
        store.delete(&row.uuid).unwrap();
        assert!(store.words().unwrap().is_empty());
    }

    #[test]
    fn normalizes_month_day_across_year_boundary() {
        let now = Local::now().date_naive();
        let mut candidate = NaiveDate::from_ymd_opt(now.year(), 12, 31).unwrap();
        if candidate > now {
            candidate = NaiveDate::from_ymd_opt(now.year() - 1, 12, 31).unwrap();
        }
        assert_eq!(
            normalize_date("12-31"),
            candidate.format("%Y-%m-%d").to_string()
        );
        assert_eq!(normalize_date("2026-08-16"), "2026-08-16");
        assert_eq!(normalize_date("bad"), "");
    }

    #[test]
    fn open_writes_v1_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("words.csv");
        write_file(
            &path,
            "id,word,translation,hit,last\n1,hello,你好,1,08-16\n",
        );
        let store = Store::open(Some(path.clone())).unwrap();
        assert!(backup_path(&path).exists());
        assert_eq!(store.words().unwrap().len(), 1);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("uuid,word"));
    }

    #[test]
    fn loads_missing_columns_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("words.csv");
        write_file(&path, "uuid,word\nabc,hello\n");
        let outcome = read_rows(&path).unwrap();
        assert_eq!(outcome.rows.len(), 1);
        assert_eq!(outcome.rows[0].box_, 1);
        assert!(outcome.dirty);
    }
}

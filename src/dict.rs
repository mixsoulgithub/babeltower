use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use rusqlite::{Connection, OpenFlags, params};
use sha2::{Digest, Sha256};

use crate::store::data_dir;
use crate::text::{Lang, classify, strip_pos};

pub const ECDICT_URL: &str =
    "https://raw.githubusercontent.com/skywind3000/ECDICT/master/ecdict.csv";
pub const CEDICT_URL: &str =
    "https://www.mdbg.net/chinese/export/cedict/cedict_1_0_ts_utf-8_mdbg.txt.gz";

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub id: i64,
    pub head: String,
    pub trad: String,
    pub lang: String,
    pub phonetic: String,
    pub pinyin: String,
    pub pos: String,
    pub translation: String,
    pub definition: String,
    pub tags: String,
    pub collins: i64,
    pub oxford: i64,
    pub bnc: i64,
    pub frq: i64,
    pub exchange: String,
}

#[derive(Clone, Debug, Default)]
pub struct LookupResult {
    pub entries: Vec<Entry>,
    pub stemmed: bool,
    pub dict_missing: bool,
}

#[derive(Clone, Debug)]
pub struct Sense {
    pub pos: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug, Default)]
pub struct DictInfo {
    pub path: PathBuf,
    pub total: u64,
    pub en: u64,
    pub zh: u64,
}

pub fn default_ecdict_path() -> PathBuf {
    data_dir().join("ecdict.csv")
}

pub fn default_cedict_path() -> PathBuf {
    data_dir().join("cedict.txt")
}

pub fn dict_path() -> PathBuf {
    if let Ok(value) = std::env::var("BBT_DICT") {
        return PathBuf::from(value);
    }
    data_dir().join("dict.sqlite")
}

pub fn dict_available() -> bool {
    dict_path().is_file()
}

pub fn split_senses(text: &str) -> Vec<Sense> {
    let text = text.replace("\\n", "\n");
    let mut senses = Vec::new();
    for chunk in text.split(['\n', ';']) {
        let (pos, body) = strip_pos(chunk);
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        senses.push(Sense {
            pos,
            body: body.to_string(),
        });
    }
    senses
}

pub fn entry_translation(entry: &Entry) -> String {
    let senses = split_senses(&entry.translation);
    if senses.is_empty() {
        return entry.translation.trim().to_string();
    }
    senses
        .iter()
        .map(|sense| match &sense.pos {
            Some(pos) => format!("{pos} {}", sense.body),
            None => sense.body.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn open_readonly_path(path: &Path) -> Result<Connection> {
    if !path.is_file() {
        bail!(
            "dictionary missing at {}; run `bbt dict fetch` then `bbt dict build`",
            path.display()
        );
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).context("open dictionary")
}

fn open_readonly() -> Result<Connection> {
    open_readonly_path(&dict_path())
}

pub fn lookup_word(input: &str, limit: usize) -> Result<LookupResult> {
    lookup_in(&dict_path(), input, limit)
}

pub fn lookup_in(path: &Path, input: &str, limit: usize) -> Result<LookupResult> {
    let mut result = LookupResult::default();
    if !path.is_file() {
        result.dict_missing = true;
        return Ok(result);
    }
    let conn = open_readonly_path(path)?;
    match classify(input) {
        crate::text::Classified::Word { lang: Lang::En } => {
            let head = input.trim().to_lowercase();
            let mut entries = query_en(&conn, &head, limit)?;
            if entries.is_empty() {
                for candidate in stem_candidates(&head) {
                    entries = query_en(&conn, &candidate, limit)?;
                    if !entries.is_empty() {
                        result.stemmed = true;
                        break;
                    }
                }
            }
            result.entries = entries;
        }
        crate::text::Classified::Word { lang: Lang::Zh } => {
            let head = input.trim();
            result.entries = query_zh(&conn, head, limit)?;
        }
        crate::text::Classified::Sentence => {}
    }
    Ok(result)
}

fn query_en(conn: &Connection, head: &str, limit: usize) -> Result<Vec<Entry>> {
    let mut statement = conn.prepare(
        "SELECT id, head, trad, lang, phonetic, pinyin, pos, translation, definition, tags,
                collins, oxford, bnc, frq, exchange
         FROM entry WHERE lang = 'en' AND head = ?1
         ORDER BY collins DESC,
                  CASE WHEN frq = 0 THEN 999999999 ELSE frq END ASC,
                  id ASC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![head, limit as i64], row_to_entry)?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

fn query_zh(conn: &Connection, head: &str, limit: usize) -> Result<Vec<Entry>> {
    let mut statement = conn.prepare(
        "SELECT id, head, trad, lang, phonetic, pinyin, pos, translation, definition, tags,
                collins, oxford, bnc, frq, exchange
         FROM entry WHERE lang = 'zh' AND (head = ?1 OR trad = ?1)
         ORDER BY CASE WHEN head = ?1 THEN 0 ELSE 1 END ASC,
                  pinyin ASC, id ASC
         LIMIT ?2",
    )?;
    let rows = statement.query_map(params![head, limit as i64], row_to_entry)?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let entry = row?;
        if seen.insert(entry.id) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: row.get(0)?,
        head: row.get(1)?,
        trad: row.get(2)?,
        lang: row.get(3)?,
        phonetic: row.get(4)?,
        pinyin: row.get(5)?,
        pos: row.get(6)?,
        translation: row.get(7)?,
        definition: row.get(8)?,
        tags: row.get(9)?,
        collins: row.get(10)?,
        oxford: row.get(11)?,
        bnc: row.get(12)?,
        frq: row.get(13)?,
        exchange: row.get(14)?,
    })
}

fn stem_candidates(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    for suffix in ["s", "es", "ed", "ing", "ly"] {
        if let Some(stem) = word.strip_suffix(suffix) {
            if !stem.is_empty() {
                out.push(stem.to_string());
                if (suffix == "ing" || suffix == "ed") && !stem.ends_with('e') {
                    out.push(format!("{stem}e"));
                }
                let mut chars = stem.chars();
                let last = chars.next_back();
                let rest: String = chars.collect();
                if let Some(last) = last {
                    if rest.ends_with(last) {
                        out.push(rest);
                    }
                }
            }
        }
    }
    let mut seen = HashSet::new();
    out.retain(|value| seen.insert(value.clone()));
    out
}

pub fn fetch(ecdict: bool, cedict: bool, force: bool) -> Result<()> {
    let (ecdict, cedict) = if !ecdict && !cedict {
        (true, true)
    } else {
        (ecdict, cedict)
    };
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    if ecdict {
        download(&client, ECDICT_URL, &default_ecdict_path(), force, false)?;
    }
    if cedict {
        download(&client, CEDICT_URL, &default_cedict_path(), force, true)?;
    }
    Ok(())
}

fn download(client: &Client, url: &str, path: &Path, force: bool, gzip: bool) -> Result<()> {
    if path.exists() && !force {
        bail!("{} exists; use --force", path.display());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("download {url}"))?
        .error_for_status()
        .with_context(|| format!("download {url}"))?;
    let tmp = PathBuf::from(format!("{}.part", path.display()));
    if gzip {
        let gz_path = PathBuf::from(format!("{}.gz.part", path.display()));
        {
            let mut file = File::create(&gz_path)?;
            stream_copy(&mut response, &mut file, path)?;
            file.sync_all()?;
        }
        let gz = File::open(&gz_path)?;
        let mut decoder = GzDecoder::new(gz);
        let mut file = File::create(&tmp)?;
        std::io::copy(&mut decoder, &mut file)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::remove_file(gz_path)?;
    } else {
        let mut file = File::create(&tmp)?;
        stream_copy(&mut response, &mut file, path)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
    }
    eprintln!("\nwrote {}", path.display());
    Ok(())
}

fn stream_copy(reader: &mut impl Read, writer: &mut impl Write, path: &Path) -> Result<()> {
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    let mut last_report = 0u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        total += read as u64;
        if total - last_report >= 1024 * 1024 {
            eprint!(
                "\r{} {:.1} MiB",
                path.file_name().unwrap_or_default().to_string_lossy(),
                total as f64 / 1_048_576.0
            );
            last_report = total;
        }
    }
    Ok(())
}

pub fn build(
    ecdict_path: Option<&Path>,
    cedict_path: Option<&Path>,
    out_path: Option<&Path>,
) -> Result<DictInfo> {
    let out = out_path.map(Path::to_path_buf).unwrap_or_else(dict_path);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = PathBuf::from(format!("{}.tmp.{}", out.display(), std::process::id()));
    let _ = fs::remove_file(&tmp);
    let mut conn = Connection::open(&tmp)?;
    conn.execute_batch(
        "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
         CREATE TABLE entry (
             id INTEGER PRIMARY KEY,
             head TEXT NOT NULL,
             trad TEXT,
             lang TEXT NOT NULL,
             phonetic TEXT,
             pinyin TEXT,
             pos TEXT,
             translation TEXT NOT NULL,
             definition TEXT,
             tags TEXT,
             collins INTEGER DEFAULT 0,
             oxford INTEGER DEFAULT 0,
             bnc INTEGER DEFAULT 0,
             frq INTEGER DEFAULT 0,
             exchange TEXT
         );
         CREATE INDEX entry_lang_head ON entry(lang, head);",
    )?;
    let mut seen = HashSet::new();
    let mut en_count = 0u64;
    let mut zh_count = 0u64;
    let ecdict = ecdict_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_ecdict_path);
    let cedict = cedict_path
        .map(Path::to_path_buf)
        .unwrap_or_else(default_cedict_path);
    if !ecdict.exists() && !cedict.exists() {
        bail!(
            "no dictionary sources found at {} or {}; run `bbt dict fetch`",
            ecdict.display(),
            cedict.display()
        );
    }
    let tx = conn.transaction()?;
    if ecdict.exists() {
        en_count = import_ecdict(&tx, &ecdict, &mut seen)?;
    }
    if cedict.exists() {
        zh_count = import_cedict(&tx, &cedict, &mut seen)?;
    }
    tx.commit()?;
    conn.execute_batch("ANALYZE;")?;
    drop(conn);
    fs::rename(&tmp, &out)?;
    eprintln!(
        "dict: en {en_count}, zh {zh_count}, total {} -> {}",
        en_count + zh_count,
        out.display()
    );
    Ok(DictInfo {
        path: out,
        total: en_count + zh_count,
        en: en_count,
        zh: zh_count,
    })
}

fn import_ecdict(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    seen: &mut HashSet<u64>,
) -> Result<u64> {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_path(path)?;
    let headers = reader.headers()?.clone();
    let index: std::collections::HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, name)| (name.trim().trim_start_matches('\u{feff}').to_lowercase(), i))
        .collect();
    let mut count = 0u64;
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
        let mut translation = get("translation");
        if word.is_empty() || translation.is_empty() {
            continue;
        }
        translation = translation.replace("\\n", "\n");
        let definition = get("definition").replace("\\n", "\n");
        let head = word.to_lowercase();
        let pinyin = String::new();
        let key = hash_key(&["en", &head, &pinyin, &translation]);
        if !seen.insert(key) {
            continue;
        }
        tx.execute(
            "INSERT INTO entry
             (head, trad, lang, phonetic, pinyin, pos, translation, definition, tags,
              collins, oxford, bnc, frq, exchange)
             VALUES (?1, '', 'en', ?2, '', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                head,
                get("phonetic"),
                get("pos"),
                translation,
                definition,
                get("tag"),
                parse_int(&get("collins")),
                parse_int(&get("oxford")),
                parse_int(&get("bnc")),
                parse_int(&get("frq")),
                get("exchange"),
            ],
        )?;
        count += 1;
    }
    Ok(count)
}

fn import_cedict(
    tx: &rusqlite::Transaction<'_>,
    path: &Path,
    seen: &mut HashSet<u64>,
) -> Result<u64> {
    let reader: Box<dyn BufRead> =
        if path.extension().and_then(|value| value.to_str()) == Some("gz") {
            Box::new(BufReader::new(GzDecoder::new(File::open(path)?)))
        } else {
            Box::new(BufReader::new(File::open(path)?))
        };
    let mut count = 0u64;
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((trad, rest)) = line.split_once(' ') else {
            continue;
        };
        let Some((simp, rest)) = rest.split_once(' ') else {
            continue;
        };
        let Some(open) = rest.find('[') else {
            continue;
        };
        let Some(close_rel) = rest[open..].find(']') else {
            continue;
        };
        let close = open + close_rel;
        let pinyin = rest[open + 1..close].trim().to_string();
        let defs_text = rest[close + 1..].trim();
        if !defs_text.starts_with('/') {
            continue;
        }
        let defs: Vec<&str> = defs_text
            .split('/')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        if defs.is_empty() {
            continue;
        }
        let translation = defs.join("; ");
        for head in std::iter::once(simp).chain(std::iter::once(trad).filter(|t| *t != simp)) {
            let key = hash_key(&["zh", head, &pinyin, &translation]);
            if !seen.insert(key) {
                continue;
            }
            tx.execute(
                "INSERT INTO entry
                 (head, trad, lang, phonetic, pinyin, pos, translation, definition, tags,
                  collins, oxford, bnc, frq, exchange)
                 VALUES (?1, ?2, 'zh', '', ?3, '', ?4, '', '', 0, 0, 0, 0, '')",
                params![head, trad, &pinyin, &translation],
            )?;
            count += 1;
        }
    }
    Ok(count)
}

pub fn info() -> Result<DictInfo> {
    let path = dict_path();
    if !path.is_file() {
        bail!("dictionary missing at {}", path.display());
    }
    let conn = open_readonly()?;
    let total: u64 = conn.query_row("SELECT COUNT(*) FROM entry", [], |row| row.get(0))?;
    let en: u64 = conn.query_row("SELECT COUNT(*) FROM entry WHERE lang='en'", [], |row| {
        row.get(0)
    })?;
    let zh: u64 = conn.query_row("SELECT COUNT(*) FROM entry WHERE lang='zh'", [], |row| {
        row.get(0)
    })?;
    Ok(DictInfo {
        path,
        total,
        en,
        zh,
    })
}

fn parse_int(value: &str) -> i64 {
    value.trim().parse().unwrap_or(0)
}

fn hash_key(parts: &[&str]) -> u64 {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest = hasher.finalize();
    u64::from_be_bytes(digest[..8].try_into().expect("digest length"))
}

pub fn cli_lookup_text(result: &LookupResult) -> String {
    if result.entries.is_empty() {
        return String::new();
    }
    let mut values = Vec::new();
    for entry in &result.entries {
        let value = entry_translation(entry);
        if !value.is_empty() && !values.contains(&value) {
            values.push(value);
        }
    }
    values.join("; ")
}

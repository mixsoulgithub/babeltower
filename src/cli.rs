use std::io::{self, Read, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use csv::WriterBuilder;

use crate::dict;
use crate::merge;
use crate::store::Store;
use crate::text::{Classified, classify};
use crate::translate::Translator;

#[derive(Parser, Debug)]
#[command(
    name = "aitr",
    version,
    about = "Offline dictionary and Leitner review for the terminal"
)]
struct Cli {
    #[arg(short = 'f', long)]
    format: bool,
    #[arg(short = 'r', long)]
    record: bool,
    #[arg(short = 'l', long)]
    list: bool,
    #[arg(long)]
    ai: bool,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(value_name = "TEXT")]
    text: Vec<String>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Merge {
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        report: Option<PathBuf>,
    },
    Import {
        path: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        report: Option<PathBuf>,
    },
    Gc {
        #[arg(long, default_value_t = 90)]
        days: i64,
        #[arg(long)]
        force: bool,
    },
    Dict {
        #[command(subcommand)]
        command: DictCommand,
    },
}

#[derive(Subcommand, Debug)]
enum DictCommand {
    Fetch {
        #[arg(long)]
        ecdict: bool,
        #[arg(long)]
        cedict: bool,
        #[arg(long)]
        force: bool,
    },
    Build {
        #[arg(long)]
        ecdict: Option<PathBuf>,
        #[arg(long)]
        cedict: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    Info,
}

pub fn main() -> i32 {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("aitr: {error:#}");
            1
        }
    }
}

fn run(cli: Cli) -> Result<i32> {
    if let Some(command) = cli.command {
        return run_command(command);
    }
    if cli.list {
        return list_words();
    }
    let text = if cli.text.is_empty() {
        if stdin_is_tty() {
            let store = Store::open(None)?;
            crate::tui::run(store)?;
            return Ok(0);
        }
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        text.trim().to_string()
    } else {
        cli.text.join(" ")
    };
    if text.trim().is_empty() {
        eprintln!("aitr: no input");
        return Ok(1);
    }
    lookup_and_print(&text, cli.format, cli.record, cli.ai)
}

fn lookup_and_print(text: &str, format: bool, record: bool, force_ai: bool) -> Result<i32> {
    let classified = classify(text);
    let use_ai = force_ai || matches!(classified, Classified::Sentence);
    let mut save_word = text.trim().to_string();
    let mut save_phonetic: Option<String> = None;
    let mut save_tags: Option<String> = None;
    let translation = if use_ai {
        let translator = Translator::new()?;
        translator.translate(text)?.translation
    } else {
        let lookup = dict::lookup_word(text, 20)?;
        if lookup.dict_missing {
            eprintln!("aitr: dictionary missing; run `aitr dict fetch` then `aitr dict build`");
            return Ok(1);
        }
        if lookup.entries.is_empty() {
            eprintln!("aitr: not found in offline dict");
            eprintln!("aitr: use a sentence or `--ai` to force AI");
            return Ok(1);
        }
        let value = dict::cli_lookup_text(&lookup);
        if lookup.stemmed {
            if let Some(first) = lookup.entries.first() {
                save_word = first.head.clone();
            }
        }
        if let Some(first) = lookup.entries.first() {
            if !first.phonetic.is_empty() {
                save_phonetic = Some(first.phonetic.clone());
            }
            if !first.tags.is_empty() {
                save_tags = Some(first.tags.clone());
            }
        }
        value
    };
    let save_translation = translation.clone();
    if record {
        let store = Store::open(None)?;
        store.upsert(
            &save_word,
            &save_translation,
            save_phonetic.as_deref(),
            save_tags.as_deref(),
        )?;
    }
    let output = if format {
        if text.as_bytes().len() > 80 {
            format!("{}\n\n{}\n", text.trim(), translation)
        } else {
            format!("{}    {}\n", text.trim(), translation)
        }
    } else {
        format!("{translation}\n")
    };
    match io::stdout().write_all(output.as_bytes()) {
        Ok(()) => Ok(0),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn list_words() -> Result<i32> {
    let store = Store::open(None)?;
    let mut rows = store.words()?;
    rows.sort_by(|a, b| (b.hit, b.last.as_str()).cmp(&(a.hit, a.last.as_str())));
    let mut buffer = Vec::new();
    {
        let mut writer = WriterBuilder::new().from_writer(&mut buffer);
        writer.write_record(["hit", "word", "translation", "last"])?;
        for row in rows {
            let record = vec![row.hit.to_string(), row.word, row.translation, row.last];
            writer.write_record(record.iter().map(String::as_str))?;
        }
        writer.flush()?;
    }
    write_stdout(&buffer)
}

fn write_stdout(bytes: &[u8]) -> Result<i32> {
    match io::stdout().write_all(bytes) {
        Ok(()) => Ok(0),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn run_command(command: Command) -> Result<i32> {
    match command {
        Command::Merge {
            path,
            dry_run,
            report,
        } => {
            let store = Store::open(None)?;
            merge::merge(&store, &path, dry_run, report.as_deref())?;
        }
        Command::Import {
            path,
            dry_run,
            report,
        } => {
            let store = Store::open(None)?;
            merge::import(&store, &path, dry_run, report.as_deref())?;
        }
        Command::Gc { days, force } => {
            let store = Store::open(None)?;
            merge::gc(&store, days, force)?;
        }
        Command::Dict { command } => match command {
            DictCommand::Fetch {
                ecdict,
                cedict,
                force,
            } => dict::fetch(ecdict, cedict, force)?,
            DictCommand::Build {
                ecdict,
                cedict,
                out,
            } => {
                dict::build(ecdict.as_deref(), cedict.as_deref(), out.as_deref())?;
            }
            DictCommand::Info => {
                let info = dict::info().context("dict info")?;
                println!("path: {}", info.path.display());
                println!("total: {}", info.total);
                println!("en: {}", info.en);
                println!("zh: {}", info.zh);
            }
        },
    }
    Ok(0)
}

fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

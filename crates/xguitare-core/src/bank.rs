use std::fs;
use std::path::{Path, PathBuf};

use crate::preset::Preset;

#[derive(Debug, Clone, PartialEq)]
pub struct Bank {
    pub dir: PathBuf,
    pub entries: Vec<BankEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BankEntry {
    Valid { file: PathBuf, preset: Preset },
    Invalid { file: PathBuf, error: String },
}

impl Bank {
    pub fn load_dir(dir: &Path) -> Result<Bank, std::io::Error> {
        let mut files = Vec::new();
        for dir_entry in fs::read_dir(dir)? {
            let path = dir_entry?.path();
            if path.extension().is_some_and(|ext| ext == "toml") {
                files.push(path);
            }
        }
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        let entries = files.into_iter().map(load_entry).collect();
        Ok(Bank {
            dir: dir.to_path_buf(),
            entries,
        })
    }

    pub fn position(&self, file: &Path) -> Option<usize> {
        self.entries.iter().position(|entry| entry.file() == file)
    }

    pub fn by_name_or_number(&self, key: &str) -> Option<(usize, &BankEntry)> {
        if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) {
            let number: usize = key.parse().ok()?;
            return self.entries.get(number).map(|entry| (number, entry));
        }
        self.entries.iter().enumerate().find(
            |(_, entry)| matches!(entry, BankEntry::Valid { preset, .. } if preset.name == key),
        )
    }
}

impl BankEntry {
    pub fn file(&self) -> &Path {
        match self {
            BankEntry::Valid { file, .. } | BankEntry::Invalid { file, .. } => file,
        }
    }
}

fn load_entry(file: PathBuf) -> BankEntry {
    let parsed = fs::read_to_string(&file)
        .map_err(|error| error.to_string())
        .and_then(|text| Preset::parse(&text).map_err(|error| error.to_string()));
    match parsed {
        Ok(preset) => BankEntry::Valid { file, preset },
        Err(error) => BankEntry::Invalid { file, error },
    }
}

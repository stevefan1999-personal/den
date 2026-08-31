use std::{
    borrow::Cow,
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
};

use rustyline::{
    Config, Result,
    config::HistoryDuplicates,
    error::ReadlineError,
    history::{History, SearchDirection, SearchResult},
};
use surrealkv::{Durability, LSMIterator as _, Mode, Transaction, Tree, TreeBuilder};
use tokio::runtime::Handle;

const ENTRY_PREFIX: &[u8] = b"entry/";
const ENTRY_END: &[u8] = b"entry0";

struct Entry {
    id:   u64,
    text: String,
}

/// Rustyline navigation backed by one SurrealKV tree.
///
/// Navigation stays in memory. Mutations use Tokio's supported
/// `block_in_place` bridge because [`History`] is synchronous while SurrealKV
/// commits are async.
pub struct Surreal {
    tree:         Option<Tree>,
    handle:       Handle,
    path:         PathBuf,
    entries:      VecDeque<Entry>,
    next_id:      u64,
    max_len:      usize,
    ignore_space: bool,
    ignore_dups:  bool,
}

impl Surreal {
    pub(crate) fn open(config: &Config, path: PathBuf) -> Result<Self> {
        let tree = TreeBuilder::new()
            .with_path(path.clone())
            .with_block_size(4 * 1024)
            .with_max_memtable_size(1024 * 1024)
            .build()
            .map_err(storage_error)?;
        let mut history = Self::empty(config, path, Some(tree));
        history.read_entries()?;
        history.prune(history.max_len)?;
        Ok(history)
    }

    pub(crate) fn in_memory(config: &Config, path: PathBuf) -> Self {
        Self::empty(config, path, None)
    }

    fn empty(config: &Config, path: PathBuf, tree: Option<Tree>) -> Self {
        Self {
            tree,
            handle: Handle::current(),
            path,
            entries: VecDeque::new(),
            next_id: 0,
            max_len: config.max_history_size(),
            ignore_space: config.history_ignore_space(),
            ignore_dups: config.history_duplicates() == HistoryDuplicates::IgnoreConsecutive,
        }
    }

    fn read_entries(&mut self) -> Result<()> {
        let Some(tree) = self.tree.as_ref() else {
            return Ok(());
        };
        let transaction = tree
            .begin_with_mode(Mode::ReadOnly)
            .map_err(storage_error)?;
        let mut entries = transaction
            .range(ENTRY_PREFIX, ENTRY_END)
            .map_err(storage_error)?;
        if entries.seek_first().map_err(storage_error)? {
            loop {
                let id = decode_key(entries.key().user_key())?;
                let text = String::from_utf8(entries.value().map_err(storage_error)?)
                    .map_err(invalid_data)?;
                self.next_id = id
                    .checked_add(1)
                    .ok_or_else(|| invalid_message("history sequence is exhausted"))?;
                self.entries.push_back(Entry { id, text });
                if !entries.next().map_err(storage_error)? {
                    break;
                }
            }
        }
        Ok(())
    }

    fn ignored(&self, line: &str) -> bool {
        self.max_len == 0
            || line.is_empty()
            || (self.ignore_space && line.chars().next().is_some_and(char::is_whitespace))
            || (self.ignore_dups && self.entries.back().is_some_and(|entry| entry.text == line))
    }

    fn insert(&mut self, line: String) -> Result<bool> {
        if self.ignored(&line) {
            return Ok(false);
        }
        let id = self.next_id;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| invalid_message("history sequence is exhausted"))?;
        let evicted = (self.entries.len() == self.max_len)
            .then(|| self.entries.front().map(|entry| entry.id))
            .flatten();
        if let Some(tree) = self.tree.as_ref() {
            let mut transaction = tree.begin().map_err(storage_error)?;
            transaction.set_durability(Durability::Immediate);
            transaction
                .set(encode_key(id), line.as_bytes())
                .map_err(storage_error)?;
            if let Some(evicted) = evicted {
                transaction
                    .delete(encode_key(evicted))
                    .map_err(storage_error)?;
            }
            self.commit(&mut transaction)?;
        }
        self.next_id = next_id;
        if evicted.is_some() {
            let _ = self.entries.pop_front();
        }
        self.entries.push_back(Entry { id, text: line });
        Ok(true)
    }

    fn prune(&mut self, len: usize) -> Result<()> {
        let remove = self.entries.len().saturating_sub(len);
        let ids: Vec<u64> = self
            .entries
            .iter()
            .take(remove)
            .map(|entry| entry.id)
            .collect();
        self.delete(&ids)?;
        let _ = self.entries.drain(..remove);
        Ok(())
    }

    fn delete(&self, ids: &[u64]) -> Result<()> {
        let Some(tree) = self.tree.as_ref() else {
            return Ok(());
        };
        if ids.is_empty() {
            return Ok(());
        }
        let mut transaction = tree.begin().map_err(storage_error)?;
        transaction.set_durability(Durability::Immediate);
        for id in ids {
            transaction.delete(encode_key(*id)).map_err(storage_error)?;
        }
        self.commit(&mut transaction)
    }

    fn commit(&self, transaction: &mut Transaction) -> Result<()> {
        tokio::task::block_in_place(|| self.handle.block_on(transaction.commit()))
            .map_err(storage_error)
    }

    pub(crate) async fn close(&mut self) -> Result<()> {
        if let Some(tree) = self.tree.take() {
            tree.close().await.map_err(storage_error)?;
        }
        Ok(())
    }

    fn search_match<F>(
        &self, term: &str, start: usize, direction: SearchDirection, test: F,
    ) -> Option<SearchResult<'_>>
    where
        F: Fn(&str) -> Option<usize>,
    {
        if term.is_empty() || start >= self.entries.len() {
            return None;
        }
        match direction {
            SearchDirection::Forward => {
                self.entries
                    .iter()
                    .skip(start)
                    .enumerate()
                    .find_map(|(offset, entry)| {
                        test(&entry.text).map(|pos| {
                            SearchResult {
                                entry: Cow::Borrowed(&entry.text),
                                idx: start + offset,
                                pos,
                            }
                        })
                    })
            }
            SearchDirection::Reverse => {
                self.entries
                    .iter()
                    .take(start + 1)
                    .rev()
                    .enumerate()
                    .find_map(|(offset, entry)| {
                        test(&entry.text).map(|pos| {
                            SearchResult {
                                entry: Cow::Borrowed(&entry.text),
                                idx: start - offset,
                                pos,
                            }
                        })
                    })
            }
        }
    }

    fn same_store(&self, path: &Path) -> Result<()> {
        if path == self.path {
            Ok(())
        } else {
            Err(ReadlineError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                "SurrealKV history cannot switch stores",
            )))
        }
    }

    fn flush_wal(&self) -> Result<()> {
        if let Some(tree) = self.tree.as_ref() {
            tree.flush_wal(true).map_err(storage_error)?;
        }
        Ok(())
    }
}

impl History for Surreal {
    fn get(&self, index: usize, _direction: SearchDirection) -> Result<Option<SearchResult<'_>>> {
        Ok(self.entries.get(index).map(|entry| {
            SearchResult {
                entry: Cow::Borrowed(&entry.text),
                idx:   index,
                pos:   0,
            }
        }))
    }

    fn add(&mut self, line: &str) -> Result<bool> { self.insert(line.to_owned()) }

    fn add_owned(&mut self, line: String) -> Result<bool> { self.insert(line) }

    fn len(&self) -> usize { self.entries.len() }

    fn is_empty(&self) -> bool { self.entries.is_empty() }

    fn set_max_len(&mut self, len: usize) -> Result<()> {
        self.prune(len)?;
        self.max_len = len;
        Ok(())
    }

    fn ignore_dups(&mut self, yes: bool) -> Result<()> {
        self.ignore_dups = yes;
        Ok(())
    }

    fn ignore_space(&mut self, yes: bool) { self.ignore_space = yes; }

    fn save(&mut self, path: &Path) -> Result<()> {
        self.same_store(path)?;
        self.flush_wal()
    }

    fn append(&mut self, path: &Path) -> Result<()> { self.save(path) }

    fn load(&mut self, path: &Path) -> Result<()> { self.same_store(path) }

    fn clear(&mut self) -> Result<()> {
        let ids: Vec<u64> = self.entries.iter().map(|entry| entry.id).collect();
        self.delete(&ids)?;
        self.entries.clear();
        Ok(())
    }

    fn search(
        &self, term: &str, start: usize, dir: SearchDirection,
    ) -> Result<Option<SearchResult<'_>>> {
        Ok(self.search_match(term, start, dir, |entry| entry.find(term)))
    }

    fn starts_with(
        &self, term: &str, start: usize, dir: SearchDirection,
    ) -> Result<Option<SearchResult<'_>>> {
        Ok(self.search_match(term, start, dir, |entry| {
            entry.starts_with(term).then_some(term.len())
        }))
    }
}

fn encode_key(id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(ENTRY_PREFIX.len() + size_of::<u64>());
    key.extend_from_slice(ENTRY_PREFIX);
    key.extend_from_slice(&id.to_be_bytes());
    key
}

fn decode_key(key: &[u8]) -> Result<u64> {
    let bytes = key
        .strip_prefix(ENTRY_PREFIX)
        .ok_or_else(|| invalid_message("history key has the wrong prefix"))?;
    let bytes: [u8; size_of::<u64>()] = bytes
        .try_into()
        .map_err(|_error| invalid_message("history key has the wrong length"))?;
    Ok(u64::from_be_bytes(bytes))
}

fn storage_error(error: impl std::fmt::Display) -> ReadlineError {
    ReadlineError::Io(io::Error::other(error.to_string()))
}

fn invalid_data(error: impl std::fmt::Display) -> ReadlineError {
    invalid_message(&error.to_string())
}

fn invalid_message(message: &str) -> ReadlineError {
    ReadlineError::Io(io::Error::new(io::ErrorKind::InvalidData, message))
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use rustyline::history::{History as _, SearchDirection};

    use super::Surreal;

    #[tokio::test(flavor = "multi_thread")]
    async fn entries_survive_reopen_and_stay_bounded() -> eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("history");
        let config = rustyline::Config::builder().max_history_size(2)?.build();

        let mut history = Surreal::open(&config, path.clone())?;
        eyre::ensure!(history.add("one")?, "first entry was not added");
        eyre::ensure!(history.add("two")?, "second entry was not added");
        eyre::ensure!(history.add("three")?, "third entry was not added");
        history.close().await?;

        let mut reopened = Surreal::open(&config, path)?;
        eyre::ensure!(reopened.len() == 2, "history was not pruned to two entries");
        let first = reopened
            .get(0, SearchDirection::Forward)?
            .ok_or_else(|| eyre::eyre!("first history entry is missing"))?;
        eyre::ensure!(first.entry == "two", "oldest retained entry was not `two`");
        eyre::ensure!(
            reopened
                .starts_with("thr", 1, SearchDirection::Reverse)?
                .is_some(),
            "reverse prefix search did not find `three`"
        );
        reopened.clear()?;
        reopened.close().await?;
        Ok(())
    }
}

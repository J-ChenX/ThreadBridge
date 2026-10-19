//! Native history may be compressed in place while the index keeps its .jsonl path.
//! Resolve only the exact .zst sibling and never rewrite Codex's files or index.
use anyhow::{ensure, Result};
use std::{
    fs::{self, File},
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

pub fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::metadata(path) {
        Ok(_) => Ok(path.to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let compressed = crate::collection::suffix(path, ".zst");
            fs::metadata(&compressed)?;
            Ok(compressed)
        }
        Err(error) => Err(error),
    }
}

pub fn reader(path: &Path) -> Result<Box<dyn Read>> {
    let path = resolve(path)?;
    let file = File::open(&path)?;
    if path.extension().is_some_and(|extension| extension == "zst") {
        let mut decoder = zstd::stream::read::Decoder::new(file)?;
        decoder.window_log_max(27)?;
        Ok(Box::new(decoder))
    } else {
        Ok(Box::new(file))
    }
}

pub enum Source {
    Plain(File, u64),
    Compressed(Cursor<Vec<u8>>),
}
impl Source {
    pub fn open(path: &Path, limit: u64) -> Result<Self> {
        let path = resolve(path)?;
        if path.extension().is_some_and(|extension| extension == "zst") {
            let mut bytes = Vec::new();
            reader(&path)?.take(limit + 1).read_to_end(&mut bytes)?;
            ensure!(bytes.len() as u64 <= limit, "rollout_source_limit");
            Ok(Self::Compressed(Cursor::new(bytes)))
        } else {
            let file = File::open(path)?;
            let size = file.metadata()?.len();
            Ok(Self::Plain(file, size))
        }
    }
    pub fn len(&self) -> u64 {
        match self {
            Self::Plain(_, size) => *size,
            Self::Compressed(data) => data.get_ref().len() as u64,
        }
    }
    pub fn scan_cost(&self, tail: u64) -> u64 {
        match self {
            Self::Plain(_, size) => (*size).min(tail),
            Self::Compressed(_) => self.len(),
        }
    }
    pub fn is_compressed(&self) -> bool {
        matches!(self, Self::Compressed(_))
    }
}
impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(file, _) => file.read(buffer),
            Self::Compressed(data) => data.read(buffer),
        }
    }
}
impl Seek for Source {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Plain(file, _) => file.seek(position),
            Self::Compressed(data) => data.seek(position),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_sibling_is_bounded_and_plain_takes_priority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("history.jsonl");
        let compressed = crate::collection::suffix(&path, ".zst");
        fs::write(
            &compressed,
            zstd::stream::encode_all(&b"history\n"[..], 1).unwrap(),
        )
        .unwrap();
        let mut source = Source::open(&path, 8).unwrap();
        assert_eq!(source.len(), 8);
        assert_eq!(source.scan_cost(2), 8);
        source.seek(SeekFrom::Start(2)).unwrap();
        let mut text = String::new();
        source.read_to_string(&mut text).unwrap();
        assert_eq!(text, "story\n");
        assert!(Source::open(&path, 7).is_err());
        fs::write(&path, "live").unwrap();
        assert_eq!(resolve(&path).unwrap(), path);
        assert_eq!(Source::open(&path, 8).unwrap().len(), 4);
        fs::remove_file(&path).unwrap();
        fs::write(&compressed, b"invalid compressed history").unwrap();
        assert!(Source::open(&path, 1024).is_err());
    }
}

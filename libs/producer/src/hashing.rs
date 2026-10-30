//! A reader that hashes what passes through it. The decoder owns one handle;
//! the producer keeps another to drain what the decoder left unread (a zstd
//! frame's tail) and take the file's hash once the day is done.
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::sync::{Arc, Mutex, PoisonError};

struct Inner {
    reader: Box<dyn Read + Send>,
    hasher: Sha256,
    bytes: u64,
}

#[derive(Clone)]
pub(crate) struct Hashed(Arc<Mutex<Inner>>);

impl Hashed {
    pub(crate) fn new(reader: Box<dyn Read + Send>) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            reader,
            hasher: Sha256::new(),
            bytes: 0,
        })))
    }

    /// Reads to the end, then the hex SHA-256 and size of everything read.
    pub(crate) fn finish(&self) -> io::Result<(String, u64)> {
        let mut buf = vec![0; 64 * 1024];
        while self.read_shared(&mut buf)? > 0 {}
        let inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let sum = inner.hasher.clone().finalize();
        Ok((
            sum.iter().map(|b| format!("{b:02x}")).collect(),
            inner.bytes,
        ))
    }

    fn read_shared(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let n = inner.reader.read(buf)?;
        inner.hasher.update(&buf[..n]);
        inner.bytes += n as u64;
        Ok(n)
    }
}

impl Read for Hashed {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_shared(buf)
    }
}

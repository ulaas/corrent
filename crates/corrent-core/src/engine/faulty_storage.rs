//! STABILITY.md S8: the engine's storage with faults injected (a full disk, refused access, I/O
//! errors, slow writes), for tests only. It wraps librqbit's filesystem storage and calls itself
//! that type, so the JSON session database (which accepts only the filesystem storage) keeps
//! working; rules come from `faults` by the profile's folder (each test has its own).
// Test tooling: failing loudly is the point.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::any::TypeId;
use std::io::IoSlice;
use std::path::{Path, PathBuf};

use librqbit::storage::filesystem::FilesystemStorageFactory;
use librqbit::storage::{BoxStorageFactory, StorageFactory, TorrentStorage};
use librqbit::{ManagedTorrentShared, TorrentMetadata};
use librqbit_core::lengths::ValidPieceIndex;

use crate::faults::{StorageOp, storage_check};

#[derive(Clone)]
pub struct FaultyStorageFactory {
    scope: PathBuf,
}

impl FaultyStorageFactory {
    pub fn boxed(scope: &Path) -> BoxStorageFactory {
        Box::new(FaultyStorageFactory { scope: scope.to_path_buf() })
    }
}

impl StorageFactory for FaultyStorageFactory {
    type Storage = Box<dyn TorrentStorage>;

    fn create(&self, shared: &ManagedTorrentShared, metadata: &TorrentMetadata) -> anyhow::Result<Self::Storage> {
        let inner = FilesystemStorageFactory::default().create(shared, metadata)?;
        Ok(Box::new(FaultyStorage { scope: self.scope.clone(), inner: Box::new(inner) }))
    }

    /// The filesystem storage, as far as librqbit's checks go (its JSON database refuses others).
    fn is_type_id(&self, type_id: TypeId) -> bool {
        type_id == TypeId::of::<FilesystemStorageFactory>()
    }

    fn clone_box(&self) -> BoxStorageFactory {
        Box::new(self.clone())
    }
}

struct FaultyStorage {
    scope: PathBuf,
    inner: Box<dyn TorrentStorage>,
}

impl FaultyStorage {
    fn check(&self, op: StorageOp, bytes: u64) -> anyhow::Result<()> {
        storage_check(&self.scope, op, bytes).map_err(anyhow::Error::from)
    }
}

impl TorrentStorage for FaultyStorage {
    fn init(&mut self, shared: &ManagedTorrentShared, metadata: &TorrentMetadata) -> anyhow::Result<()> {
        self.check(StorageOp::Init, 0)?;
        self.inner.init(shared, metadata)
    }

    fn pread_exact(&self, file_id: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        self.check(StorageOp::Read, buf.len() as u64)?;
        self.inner.pread_exact(file_id, offset, buf)
    }

    fn pwrite_all(&self, file_id: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        self.check(StorageOp::Write, buf.len() as u64)?;
        self.inner.pwrite_all(file_id, offset, buf)
    }

    fn pwrite_all_vectored(&self, file_id: usize, offset: u64, bufs: [IoSlice<'_>; 2]) -> anyhow::Result<usize> {
        self.check(StorageOp::Write, bufs.iter().map(|b| b.len() as u64).sum())?;
        self.inner.pwrite_all_vectored(file_id, offset, bufs)
    }

    fn remove_file(&self, file_id: usize, filename: &Path) -> anyhow::Result<()> {
        self.inner.remove_file(file_id, filename)
    }

    fn remove_directory_if_empty(&self, path: &Path) -> anyhow::Result<()> {
        self.inner.remove_directory_if_empty(path)
    }

    fn ensure_file_length(&self, file_id: usize, length: u64) -> anyhow::Result<()> {
        self.inner.ensure_file_length(file_id, length)
    }

    /// Paused and resumed storage stays faulty (librqbit swaps it out on pause).
    fn take(&self) -> anyhow::Result<Box<dyn TorrentStorage>> {
        Ok(Box::new(FaultyStorage { scope: self.scope.clone(), inner: self.inner.take()? }))
    }

    fn on_piece_completed(&self, piece_index: ValidPieceIndex) -> anyhow::Result<()> {
        self.inner.on_piece_completed(piece_index)
    }
}

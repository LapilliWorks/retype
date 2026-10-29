//! Error type.
//!
//! This is the seed of the taxonomy in `docs/errors.md`: codes and categories
//! arrive with the job engine, which is the first caller that has to branch on
//! them. Until then, callers match on variants.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("{0}")]
	Io(#[from] std::io::Error),

	#[error("{} already exists", .0.display())]
	Exists(PathBuf),

	#[error("{} has no extension", .0.display())]
	NoExtension(PathBuf),

	#[error("{} has no parent directory", .0.display())]
	NoParent(PathBuf),

	#[error("{} has no file name", .0.display())]
	NoFileName(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

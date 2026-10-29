//! Atomic output writing and source restoration.
//!
//! Two rules from the specifications live here.
//!
//! `docs/errors.md` §5: an output is written to a temporary file in its target
//! directory and renamed into place only when complete, so a path is either
//! absent or correct, never half written.
//!
//!: when a conversion fails, the renamed path still holds the source
//! format's bytes. The source extension is restored, so that no path carries a
//! format it does not contain.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::ext::split_extension;

/// Prefix for temporary files. Never a valid intent, and never a user name.
pub const TEMP_PREFIX: &str = ".retype-tmp.";

/// Whether a path is one of our temporary files.
pub fn is_temp_file(path: &Path) -> bool {
	path.file_name()
		.and_then(|name| name.to_str())
		.is_some_and(|name| name.starts_with(TEMP_PREFIX))
}

/// Write `target` atomically.
///
/// `write` is handed a temporary path in the same directory as `target`. If it
/// returns `Ok`, the temporary file is renamed onto `target`. If it returns
/// `Err` or panics, the temporary file is removed and `target` is untouched.
pub fn write_atomically(target: &Path, write: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
	let staged = StagedOutput::new(target)?;
	write(staged.temp())?;
	staged.commit()
}

/// A temporary file that becomes `target` on commit and disappears otherwise.
pub struct StagedOutput {
	temp: PathBuf,
	target: PathBuf,
	committed: bool,
}

impl StagedOutput {
	pub fn new(target: &Path) -> Result<Self> {
		let parent = target.parent().ok_or_else(|| Error::NoParent(target.to_path_buf()))?;
		let name = target
			.file_name()
			.ok_or_else(|| Error::NoFileName(target.to_path_buf()))?;

		let temp = parent.join(format!("{TEMP_PREFIX}{}", name.to_string_lossy()));
		Ok(Self {
			temp,
			target: target.to_path_buf(),
			committed: false,
		})
	}

	/// Path the writer should create.
	pub fn temp(&self) -> &Path {
		&self.temp
	}

	/// Rename the temporary file onto the target.
	pub fn commit(mut self) -> Result<()> {
		fs::rename(&self.temp, &self.target)?;
		self.committed = true;
		Ok(())
	}
}

impl Drop for StagedOutput {
	fn drop(&mut self) {
		if !self.committed {
			let _ = fs::remove_file(&self.temp);
		}
	}
}

/// Restore a path to the source extension after a failed conversion.
///
/// Returns the restored path. Refuses to overwrite an existing file rather than
/// losing whichever bytes are already there.
pub fn restore_source_name(path: &Path, source_ext: &str) -> Result<PathBuf> {
	let (stem, _) = split_extension(path).ok_or_else(|| Error::NoExtension(path.to_path_buf()))?;
	let parent = path.parent().ok_or_else(|| Error::NoParent(path.to_path_buf()))?;

	let restored = parent.join(format!("{stem}.{source_ext}"));
	if restored.exists() {
		return Err(Error::Exists(restored));
	}
	fs::rename(path, &restored)?;
	Ok(restored)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn tempdir() -> tempfile::TempDir {
		tempfile::tempdir().expect("temp dir")
	}

	#[test]
	fn writes_target() {
		let dir = tempdir();
		let target = dir.path().join("out.webp");

		write_atomically(&target, |temp| {
			fs::write(temp, b"converted")?;
			Ok(())
		})
		.unwrap();

		assert_eq!(fs::read(&target).unwrap(), b"converted");
	}

	#[test]
	fn leaves_no_temp_on_success() {
		let dir = tempdir();
		let target = dir.path().join("out.webp");

		write_atomically(&target, |temp| {
			fs::write(temp, b"x")?;
			Ok(())
		})
		.unwrap();

		let names: Vec<_> = fs::read_dir(dir.path())
			.unwrap()
			.map(|entry| entry.unwrap().file_name())
			.collect();
		assert_eq!(names, vec![std::ffi::OsString::from("out.webp")]);
	}

	#[test]
	fn failure_leaves_no_temp_and_no_target() {
		let dir = tempdir();
		let target = dir.path().join("out.webp");

		let result = write_atomically(&target, |temp| {
			fs::write(temp, b"partial")?;
			Err(Error::Io(std::io::Error::other("backend failed")))
		});

		assert!(result.is_err());
		assert!(!target.exists());
		assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
	}

	#[test]
	fn failure_keeps_existing_target() {
		let dir = tempdir();
		let target = dir.path().join("out.webp");
		fs::write(&target, b"original").unwrap();

		let result = write_atomically(&target, |_temp| Err(Error::Io(std::io::Error::other("nope"))));

		assert!(result.is_err());
		assert_eq!(fs::read(&target).unwrap(), b"original");
	}

	#[test]
	fn success_overwrites_existing_target() {
		let dir = tempdir();
		let target = dir.path().join("out.webp");
		fs::write(&target, b"old").unwrap();

		write_atomically(&target, |temp| {
			fs::write(temp, b"new")?;
			Ok(())
		})
		.unwrap();

		assert_eq!(fs::read(&target).unwrap(), b"new");
	}

	#[test]
	fn temp_file_detection() {
		assert!(is_temp_file(Path::new("/dir/.retype-tmp.photo.jpg")));
		assert!(!is_temp_file(Path::new("/dir/photo.jpg")));
		assert!(!is_temp_file(Path::new("/dir/.hidden")));
	}

	#[test]
	fn restores_source_extension() {
		let dir = tempdir();
		let renamed = dir.path().join("photo.jpg");
		fs::write(&renamed, b"heic bytes").unwrap();

		let restored = restore_source_name(&renamed, "heic").unwrap();

		assert_eq!(restored, dir.path().join("photo.heic"));
		assert_eq!(fs::read(&restored).unwrap(), b"heic bytes");
		assert!(!renamed.exists());
	}

	#[test]
	fn restore_refuses_to_clobber() {
		let dir = tempdir();
		let renamed = dir.path().join("photo.jpg");
		let occupied = dir.path().join("photo.heic");
		fs::write(&renamed, b"new").unwrap();
		fs::write(&occupied, b"existing").unwrap();

		let error = restore_source_name(&renamed, "heic").unwrap_err();

		assert!(matches!(error, Error::Exists(_)));
		assert_eq!(fs::read(&occupied).unwrap(), b"existing");
		assert!(renamed.exists());
	}

	#[test]
	fn restores_compound_extension() {
		let dir = tempdir();
		let renamed = dir.path().join("backup.zip");
		fs::write(&renamed, b"tar bytes").unwrap();

		let restored = restore_source_name(&renamed, "tar.gz").unwrap();

		assert_eq!(restored, dir.path().join("backup.tar.gz"));
	}

	#[test]
	fn failed_conversion_then_restore_round_trip() {
 // The regression for: the renamed path holds the source bytes, the
		// conversion fails, and the name goes back to the source extension.
		let dir = tempdir();
		let renamed = dir.path().join("photo.jpg");
		fs::write(&renamed, b"heic bytes").unwrap();

		let conversion = write_atomically(&renamed, |temp| {
			fs::write(temp, b"partial")?;
			Err(Error::Io(std::io::Error::other("encoder refused")))
		});
		assert!(conversion.is_err());

		let restored = restore_source_name(&renamed, "heic").unwrap();

		assert_eq!(restored, dir.path().join("photo.heic"));
		assert_eq!(fs::read(&restored).unwrap(), b"heic bytes");
		assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
	}
}

//! Extension splitting, compound extensions included.
//!
//! `docs/grammar.md` §3 anchors an intent on the source stem, so the caller
//! always knows where the extension begins. This module exists for the places
//! where it does not: derived output names, and resolving the format of a
//! source such as `archive.tar.gz`.

use std::path::Path;

/// Known compound extensions, matched longest first so `tar.gz` wins over `gz`.
const COMPOUND_EXTENSIONS: &[&str] = &["tar.lzma", "tar.bz2", "tar.zst", "tar.gz", "tar.xz", "tar.lz", "tar.Z"];

/// Split a file name into stem and extension.
///
/// `archive.tar.gz` becomes `("archive", "tar.gz")`, `photo.jpg` becomes
/// `("photo", "jpg")`. Returns `None` when there is no extension, and for
/// dotfiles whose name would otherwise be an empty stem before a compound
/// extension.
pub fn split_extension(path: &Path) -> Option<(&str, &str)> {
	let name = path.file_name()?.to_str()?;
	split_name_extension(name)
}

fn split_name_extension(name: &str) -> Option<(&str, &str)> {
	for compound in COMPOUND_EXTENSIONS {
		let suffix = format!(".{compound}");
		if let Some(stem) = name.strip_suffix(&suffix) {
			if stem.is_empty() || stem.starts_with('.') {
				return None;
			}
			return Some((stem, compound));
		}
	}

	// Skip the leading dot so a dotfile is not read as an extension.
	let search_start = if name.starts_with('.') { 1 } else { 0 };
	let dot = name[search_start..].rfind('.')? + search_start;
	if dot == name.len() - 1 {
		return None;
	}
	Some((&name[..dot], &name[dot + 1..]))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn simple_extension() {
		assert_eq!(split_extension(Path::new("photo.jpg")), Some(("photo", "jpg")));
	}

	#[test]
	fn compound_tar_gz() {
		assert_eq!(
			split_extension(Path::new("archive.tar.gz")),
			Some(("archive", "tar.gz"))
		);
	}

	#[test]
	fn compound_tar_bz2() {
		assert_eq!(split_extension(Path::new("data.tar.bz2")), Some(("data", "tar.bz2")));
	}

	#[test]
	fn compound_tar_xz() {
		assert_eq!(split_extension(Path::new("pkg.tar.xz")), Some(("pkg", "tar.xz")));
	}

	#[test]
	fn compound_tar_zst() {
		assert_eq!(
			split_extension(Path::new("backup.tar.zst")),
			Some(("backup", "tar.zst"))
		);
	}

	#[test]
	fn no_extension() {
		assert_eq!(split_extension(Path::new("Makefile")), None);
	}

	#[test]
	fn dotfile_no_extension() {
		assert_eq!(split_extension(Path::new(".hidden")), None);
	}

	#[test]
	fn with_directory() {
		assert_eq!(
			split_extension(Path::new("/home/user/photo.png")),
			Some(("photo", "png"))
		);
	}

	#[test]
	fn multiple_dots_not_compound() {
		assert_eq!(split_extension(Path::new("my.file.txt")), Some(("my.file", "txt")));
	}

	#[test]
	fn bare_compound_no_stem() {
		assert_eq!(split_extension(Path::new(".tar.gz")), None);
	}

	#[test]
	fn case_preserved() {
		assert_eq!(split_extension(Path::new("Photo.JPG")), Some(("Photo", "JPG")));
	}
}

//! Rename correlation.
//!
//! Turns filesystem observations into rename facts, without touching the
//! filesystem. The event source lives in the daemon; this is the part that
//! decides what the events mean, and it is pure so the decision can be tested
//! without a platform.
//!
//! Scope is: in-place renames only. Source and target share a directory,
//! the stem is unchanged, and the extension differs. Anything else is a move or
//! a plain rename and carries no intent.
//!
//! Identity is the mechanism: a rename does not change the device or inode, so
//! an event whose path carries an identity already known under another path is
//! a rename, whatever the platform reported. Filesystems that cannot supply a
//! trustworthy identity fall back to the pair of names when the event source
//! provides both.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::ext::split_extension;

/// Filesystem identity of a file: device and inode, or volume and file id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
	pub device: u64,
	pub inode: u64,
}

/// What a rename fact says: this path became that path, and the new name asked
/// for this intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameFact {
	pub source: PathBuf,
	pub source_ext: String,
	pub target: PathBuf,
	pub intent: String,
}

/// One observation of a path that exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
	pub path: PathBuf,
	pub identity: Identity,
	/// Link count. Above one, identity is ambiguous between paths, so it is not
	/// used for correlation.
	pub links: u64,
}

/// Bounded identity map with insertion-order eviction.
#[derive(Debug)]
pub struct Correlator {
	capacity: usize,
	by_identity: HashMap<Identity, PathBuf>,
	by_path: HashMap<PathBuf, Identity>,
	order: VecDeque<PathBuf>,
	/// Sources reported by an event source that splits a rename into two
	/// events, waiting for their target. Bounded, and cleared at the end of a
	/// batch.
	pending: VecDeque<PathBuf>,
	pending_capacity: usize,
}

/// Maximum unmatched rename sources kept between events.
const PENDING_CAPACITY: usize = 128;

impl Correlator {
	pub fn new(capacity: usize) -> Self {
		Self {
			capacity: capacity.max(1),
			by_identity: HashMap::new(),
			by_path: HashMap::new(),
			order: VecDeque::new(),
			pending: VecDeque::new(),
			pending_capacity: PENDING_CAPACITY,
		}
	}

	pub fn len(&self) -> usize {
		self.by_identity.len()
	}

	pub fn is_empty(&self) -> bool {
		self.by_identity.is_empty()
	}

	/// Record a path that exists, and report a rename when its identity was
	/// last seen under a different path.
	pub fn observe(&mut self, sighting: Sighting) -> Option<RenameFact> {
		if sighting.links > 1 {
			// Ambiguous between names; do not correlate by identity.
			self.forget(&sighting.path);
			return None;
		}

		let fact = match self.by_identity.get(&sighting.identity) {
			Some(previous) if previous != &sighting.path => rename_between(previous, &sighting.path),
			_ => None,
		};

		self.remember(sighting.path, sighting.identity);
		fact
	}

	/// Record a source from an event source that reports a rename as two
	/// events, as Windows does. The fact is produced when `rename_to` arrives.
	pub fn rename_from(&mut self, source: &Path) {
		if self.pending.len() == self.pending_capacity {
			self.pending.pop_front();
		}
		self.pending.push_back(source.to_path_buf());
	}

	/// Match a target against a pending source in the same directory.
	///
	/// Windows reports `From` then `To` for each file, and a batch of renames
	/// can interleave, so matching is by directory and oldest first rather than
	/// by adjacency.
	pub fn rename_to(&mut self, target: &Path) -> Option<RenameFact> {
		let index = self
			.pending
			.iter()
			.position(|source| source.parent() == target.parent())?;
		let source = self.pending.remove(index)?;
		self.pair(&source, target)
	}

	/// Drop sources that never found a target. Called at the end of a batch:
	/// an unmatched `From` is a file that left the watched tree, not a rename.
	pub fn end_batch(&mut self) {
		self.pending.clear();
	}

	/// Report a rename when the event source supplies both names.
	pub fn pair(&mut self, source: &Path, target: &Path) -> Option<RenameFact> {
		let fact = rename_between(source, target);
		if let Some(identity) = self.by_path.remove(source) {
			self.remember(target.to_path_buf(), identity);
		}
		fact
	}

	/// Forget a path that no longer exists.
	pub fn forget(&mut self, path: &Path) {
		if let Some(identity) = self.by_path.remove(path) {
			self.by_identity.remove(&identity);
		}
	}

	/// Forget every path under a directory that was renamed.
	///
	/// A directory rename changes its children's paths without per-file events,
	/// so their entries are stale. Re-priming fills them in again.
	pub fn invalidate_prefix(&mut self, dir: &Path) {
		self.by_path.retain(|path, identity| {
			let stale = path.starts_with(dir);
			if stale {
				self.by_identity.remove(identity);
			}
			!stale
		});
		self.order.retain(|path| !path.starts_with(dir));
	}

	fn remember(&mut self, path: PathBuf, identity: Identity) {
		if let Some(previous) = self.by_path.insert(path.clone(), identity) {
			self.by_identity.remove(&previous);
		}
		self.by_identity.insert(identity, path.clone());
		self.order.push_back(path);

		while self.by_identity.len() > self.capacity {
			let Some(oldest) = self.order.pop_front() else { break };
			if let Some(identity) = self.by_path.remove(&oldest) {
				self.by_identity.remove(&identity);
			}
		}
	}
}

/// Whether two names describe an in-place rename with an intent.
fn rename_between(source: &Path, target: &Path) -> Option<RenameFact> {
	if source.parent() != target.parent() {
		return None;
	}

	let (source_stem, source_ext) = split_extension(source)?;
	let target_name = target.file_name()?.to_str()?;

	if !source_stem.is_empty() && source_stem.starts_with('.') {
		return None;
	}

	let intent = target_name.strip_prefix(source_stem)?.strip_prefix('.')?;
	if intent.is_empty() {
		return None;
	}

	// Same extension, case included, is a plain rename: nothing to convert.
	if intent.eq_ignore_ascii_case(source_ext) {
		return None;
	}

	Some(RenameFact {
		source: source.to_path_buf(),
		source_ext: source_ext.to_owned(),
		target: target.to_path_buf(),
		intent: intent.to_owned(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn id(inode: u64) -> Identity {
		Identity { device: 1, inode }
	}

	fn sighting(path: &str, inode: u64) -> Sighting {
		Sighting {
			path: PathBuf::from(path),
			identity: id(inode),
			links: 1,
		}
	}

	#[test]
	fn first_sighting_is_not_a_rename() {
		let mut correlator = Correlator::new(16);
		assert!(correlator.observe(sighting("/dir/photo.heic", 1)).is_none());
		assert_eq!(correlator.len(), 1);
	}

	#[test]
	fn identity_under_a_new_path_is_a_rename() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/photo.heic", 1));

		let rename = correlator.observe(sighting("/dir/photo.webp+q80", 1)).unwrap();

		assert_eq!(rename.source, PathBuf::from("/dir/photo.heic"));
		assert_eq!(rename.source_ext, "heic");
		assert_eq!(rename.target, PathBuf::from("/dir/photo.webp+q80"));
		assert_eq!(rename.intent, "webp+q80");
	}

	#[test]
	fn primed_file_renames_correctly() {
		// The daemon primes identities at registration, so a file that existed
		// before start still correlates.
		let mut correlator = Correlator::new(16);
		for (index, name) in ["a.jpg", "b.jpg", "c.jpg"].iter().enumerate() {
			correlator.observe(sighting(&format!("/dir/{name}"), index as u64 + 1));
		}

		let rename = correlator.observe(sighting("/dir/b.webp", 2)).unwrap();
		assert_eq!(rename.intent, "webp");
		assert_eq!(rename.source, PathBuf::from("/dir/b.jpg"));
	}

	#[test]
	fn same_extension_is_not_a_rename() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/photo.heic", 1));
		assert!(correlator.observe(sighting("/dir/photo.HEIC", 1)).is_none());
		assert!(correlator.observe(sighting("/dir/photo.heic", 1)).is_none());
	}

	#[test]
	fn stem_change_carries_no_intent() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/photo.heic", 1));
		assert!(correlator.observe(sighting("/dir/holiday.jpg", 1)).is_none());
	}

	#[test]
	fn cross_directory_is_a_move() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/a/photo.heic", 1));
		assert!(correlator.observe(sighting("/dir/b/photo.jpg", 1)).is_none());

		// The identity follows the file, so a later in-place rename still works.
		let rename = correlator.observe(sighting("/dir/b/photo.webp", 1)).unwrap();
		assert_eq!(rename.source, PathBuf::from("/dir/b/photo.jpg"));
	}

	#[test]
	fn hard_link_is_not_correlated() {
		let mut correlator = Correlator::new(16);
		correlator.observe(Sighting {
			path: PathBuf::from("/dir/photo.heic"),
			identity: id(1),
			links: 2,
		});
		assert!(correlator.is_empty());

		let mut linked = Correlator::new(16);
		linked.observe(Sighting {
			path: PathBuf::from("/dir/photo.heic"),
			identity: id(1),
			links: 2,
		});
		assert!(linked.observe(sighting("/dir/photo.jpg", 1)).is_none());
	}

	#[test]
	fn pair_reports_without_identity() {
		let mut correlator = Correlator::new(16);
		let rename = correlator
			.pair(Path::new("/dir/clip.mov"), Path::new("/dir/clip.mp4+mute"))
			.unwrap();

		assert_eq!(rename.intent, "mp4+mute");
		assert_eq!(rename.source_ext, "mov");
	}

	#[test]
	fn split_rename_events_are_paired() {
		let mut correlator = Correlator::new(16);
		correlator.rename_from(Path::new("/dir/a.heic"));
		let rename = correlator.rename_to(Path::new("/dir/a.jpg")).unwrap();

		assert_eq!(rename.source, PathBuf::from("/dir/a.heic"));
		assert_eq!(rename.intent, "jpg");
	}

	#[test]
	fn interleaved_split_events_are_paired_without_adjacency() {
		let mut correlator = Correlator::new(16);
		// As a batch may interleave: all sources first, then all targets.
		for name in ["a.heic", "b.heic", "c.heic"] {
			correlator.rename_from(Path::new("/dir/").join(name).as_path());
		}

		let first = correlator.rename_to(Path::new("/dir/a.jpg")).unwrap();
		let second = correlator.rename_to(Path::new("/dir/b.webp")).unwrap();

		assert_eq!(first.source, PathBuf::from("/dir/a.heic"));
		assert_eq!(second.source, PathBuf::from("/dir/b.heic"));
		assert_eq!(second.intent, "webp");
	}

	#[test]
	fn split_rename_in_another_directory_is_a_move() {
		let mut correlator = Correlator::new(16);
		correlator.rename_from(Path::new("/dir/a.heic"));
		assert!(correlator.rename_to(Path::new("/dir/sub/a.jpg")).is_none());
	}

	#[test]
	fn unmatched_source_is_dropped_at_batch_end() {
		let mut correlator = Correlator::new(16);
		correlator.rename_from(Path::new("/dir/a.heic"));
		correlator.end_batch();
		assert!(correlator.rename_to(Path::new("/dir/b.jpg")).is_none());
	}

	#[test]
	fn pending_sources_are_bounded() {
		let mut correlator = Correlator::new(16);
		correlator.pending_capacity = 2;
		for name in ["a", "b", "c"] {
			correlator.rename_from(Path::new(&format!("/dir/{name}.heic")));
		}
		// The oldest was evicted, so only `b` and `c` can match.
		assert!(correlator.rename_to(Path::new("/dir/a.jpg")).is_none());
		assert!(correlator.rename_to(Path::new("/dir/c.jpg")).is_some());
	}

	#[test]
	fn pair_with_different_stems_is_ignored() {
		let mut correlator = Correlator::new(16);
		assert!(
			correlator
				.pair(Path::new("/dir/a.mov"), Path::new("/dir/b.mp4"))
				.is_none()
		);
	}

	#[test]
	fn pair_keeps_identity_tracking() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/clip.mov", 7));
		correlator.pair(Path::new("/dir/clip.mov"), Path::new("/dir/clip.mp4"));

		correlator.observe(sighting("/dir/clip.mp4", 7));
		let rename = correlator.observe(sighting("/dir/clip.webm", 7)).unwrap();
		assert_eq!(rename.intent, "webm");
		assert_eq!(rename.source, PathBuf::from("/dir/clip.mp4"));
	}

	#[test]
	fn forgetting_a_path_drops_the_identity() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/photo.heic", 1));
		correlator.forget(Path::new("/dir/photo.heic"));
		assert!(correlator.is_empty());

		// A recycled inode must not be read as a rename of the deleted file.
		assert!(correlator.observe(sighting("/dir/other.jpg", 1)).is_none());
	}

	#[test]
	fn directory_rename_invalidates_children() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/photos/a.heic", 1));
		correlator.observe(sighting("/dir/keep.heic", 2));

		correlator.invalidate_prefix(Path::new("/dir/photos"));

		assert_eq!(correlator.len(), 1);
		// The stale path is gone, so a rename under the new directory is not
		// reported as coming from the old one.
		assert!(correlator.observe(sighting("/dir/moved/a.jpg", 1)).is_none());
	}

	#[test]
	fn capacity_evicts_oldest() {
		let mut correlator = Correlator::new(2);
		correlator.observe(sighting("/dir/a.heic", 1));
		correlator.observe(sighting("/dir/b.heic", 2));
		correlator.observe(sighting("/dir/c.heic", 3));

		assert_eq!(correlator.len(), 2);
		// `a` was evicted, so its inode is unknown and no rename is reported.
		assert!(correlator.observe(sighting("/dir/a.jpg", 1)).is_none());
		// `c` is still known.
		assert!(correlator.observe(sighting("/dir/c.jpg", 3)).is_some());
	}

	#[test]
	fn compound_target_extension_is_an_intent() {
		let mut correlator = Correlator::new(16);
		correlator.observe(sighting("/dir/data.zip", 1));
		let rename = correlator.observe(sighting("/dir/data.tar.gz", 1)).unwrap();
		assert_eq!(rename.intent, "tar.gz");
	}

	/// Deterministic pseudo random numbers, so the model test is reproducible
	/// without a dependency.
	struct XorShift(u64);

	impl XorShift {
		fn next(&mut self) -> u64 {
			self.0 ^= self.0 << 13;
			self.0 ^= self.0 >> 7;
			self.0 ^= self.0 << 17;
			self.0
		}

		fn below(&mut self, bound: u64) -> u64 {
			self.next() % bound
		}
	}

	#[test]
	fn model_agrees_on_random_rename_sequences() {
		const FORMATS: [&str; 4] = ["heic", "jpg", "webp", "png"];

		for seed in 1..=8u64 {
			let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
			let mut correlator = Correlator::new(4096);
			// Model: identity -> current path, plus each path's current extension.
			let mut live: HashMap<u64, PathBuf> = HashMap::new();
			let mut next_inode = 1u64;

			for _ in 0..200 {
				let choice = rng.below(10);

				if choice < 3 || live.is_empty() {
					// Create a file.
					let inode = next_inode;
					next_inode += 1;
					let path = PathBuf::from(format!("/dir/file{inode}.{}", FORMATS[0]));
					correlator.observe(sighting(path.to_str().unwrap(), inode));
					live.insert(inode, path);
				} else if choice < 8 {
					// Rename one file to a new extension.
					let mut inodes: Vec<u64> = live.keys().copied().collect();
					inodes.sort_unstable();
					let inode = inodes[rng.below(inodes.len() as u64) as usize];
					let current = live[&inode].clone();
					let stem = split_extension(&current).unwrap().0.to_string();
					let ext = FORMATS[rng.below(FORMATS.len() as u64) as usize];
					let target = PathBuf::from(format!("/dir/{stem}.{ext}"));

					let expected = !ext.eq_ignore_ascii_case(split_extension(&current).unwrap().1);
					let rename = correlator.observe(sighting(target.to_str().unwrap(), inode));

					assert_eq!(
						rename.is_some(),
						expected,
						"seed {seed}: inode {inode} {} -> {} should {} be a rename",
						current.display(),
						target.display(),
						if expected { "" } else { "not" }
					);
					if let Some(rename) = rename {
						assert_eq!(rename.source, current);
						assert_eq!(rename.target, target);
					}
					live.insert(inode, target);
				} else {
					// Delete one file.
					let mut inodes: Vec<u64> = live.keys().copied().collect();
					inodes.sort_unstable();
					let inode = inodes[rng.below(inodes.len() as u64) as usize];
					let path = live.remove(&inode).unwrap();
					correlator.forget(&path);
				}
			}
		}
	}
}

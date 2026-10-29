//! Measures what the filesystem actually reports for a rename, and what each
//! correlation strategy concludes from it.
//!
//! Not a test: a probe. Its output becomes `docs/events.md`. It answers two
//! questions per platform: which rename events arrive at all, and whether
//! identity correlation recovers the cases where the pair of names does not.
//!
//! Each scenario is split into a setup and an action, because a rename of a
//! file that was created in the same event batch would say nothing about how a
//! real user's rename is seen: their file existed long before.
//!
//! Usage: `cargo run -p retype-core --example probe_events`

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use retype_core::correlate::{Correlator, Identity, RenameFact, Sighting, coalesce, dedupe};

const SETTLE: Duration = Duration::from_millis(400);
const QUIET: Duration = Duration::from_millis(200);
const LIMIT: Duration = Duration::from_secs(3);

type RawEvent = (String, Vec<PathBuf>);
type Step = Box<dyn Fn(&Path) -> Result<(), Box<dyn Error>>>;

struct Scenario {
	name: &'static str,
	setup: Step,
	act: Step,
}

fn scenarios() -> Vec<Scenario> {
	fn create(name: &'static str) -> Step {
		Box::new(move |root: &Path| Ok(fs::write(root.join(name), b"source bytes")?))
	}

	fn rename(from: &'static str, to: &'static str) -> Step {
		Box::new(move |root: &Path| Ok(fs::rename(root.join(from), root.join(to))?))
	}

	vec![
		Scenario {
			name: "rename in place",
			setup: create("a.heic"),
			act: rename("a.heic", "a.jpg"),
		},
		Scenario {
			name: "rename case only",
			setup: create("b.heic"),
			act: rename("b.heic", "b.HEIC"),
		},
		Scenario {
			name: "rename stem and extension",
			setup: create("c.heic"),
			act: rename("c.heic", "c-renamed.jpg"),
		},
		Scenario {
			name: "rename across directories",
			setup: create("d.heic"),
			act: rename("d.heic", "sub/d.jpg"),
		},
		Scenario {
			name: "two renames in one window",
			setup: create("e.heic"),
			act: Box::new(|root: &Path| {
				fs::rename(root.join("e.heic"), root.join("e.jpg"))?;
				fs::rename(root.join("e.jpg"), root.join("e.webp"))?;
				Ok(())
			}),
		},
		Scenario {
			name: "rename over an existing file",
			setup: Box::new(|root: &Path| {
				fs::write(root.join("f.heic"), b"source bytes")?;
				fs::write(root.join("f.jpg"), b"existing target")?;
				Ok(())
			}),
			act: rename("f.heic", "f.jpg"),
		},
		Scenario {
			name: "twenty renames in one gesture",
			setup: Box::new(|root: &Path| {
				for index in 0..20 {
					fs::write(root.join(format!("g{index}.heic")), b"source bytes")?;
				}
				Ok(())
			}),
			act: Box::new(|root: &Path| {
				for index in 0..20 {
					fs::rename(root.join(format!("g{index}.heic")), root.join(format!("g{index}.jpg")))?;
				}
				Ok(())
			}),
		},
		Scenario {
			name: "rename then write",
			setup: create("h.heic"),
			act: Box::new(|root: &Path| {
				fs::rename(root.join("h.heic"), root.join("h.jpg"))?;
				fs::write(root.join("h.jpg"), b"rewritten by an importer")?;
				Ok(())
			}),
		},
		Scenario {
			name: "rename then delete",
			setup: create("i.heic"),
			act: Box::new(|root: &Path| {
				fs::rename(root.join("i.heic"), root.join("i.jpg"))?;
				fs::remove_file(root.join("i.jpg"))?;
				Ok(())
			}),
		},
	]
}

fn main() -> Result<(), Box<dyn Error>> {
	let dir = tempfile::tempdir()?;
	let root = dir.path().to_path_buf();
	fs::create_dir(root.join("sub"))?;

	let (tx, rx) = mpsc::channel();
	let mut watcher = notify::recommended_watcher(move |event| {
		let _ = tx.send(event);
	})?;
	watcher.watch(&root, RecursiveMode::Recursive)?;

	let mut pair = Correlator::new(4096);
	let mut identity = Correlator::new(4096);

	println!("# filesystem events");
	println!();
	println!("platform: {}", std::env::consts::OS);
	println!();

	for scenario in scenarios() {
		// Setup is fed silently, so the action starts from a primed correlator,
		// which is what the daemon has after a file has existed for a while.
		(scenario.setup)(&root)?;
		let setup_events = drain(&rx, SETTLE);
		feed(&mut pair, &mut identity, &setup_events);

		println!("## {}", scenario.name);
		(scenario.act)(&root)?;
		let events = drain(&rx, SETTLE);

		for (kind, paths) in &events {
			println!("raw: {kind} {}", join(&root, paths));
		}

		let pair_facts = feed_pair(&mut pair, &events);
		let identity_facts = feed_identity(&mut identity, &events);

		report(&root, "pair", &pair_facts);
		report(&root, "identity", &identity_facts);
		println!();
	}

	println!("## notes");
	println!();
	if cfg!(windows) {
		println!(
			"- file identity is not exposed by `std` on Windows, so this probe cannot test the identity strategy there"
		);
	}
	Ok(())
}

fn feed(pair: &mut Correlator, identity: &mut Correlator, events: &[RawEvent]) {
	feed_pair(pair, events);
	feed_identity(identity, events);
}

/// Feed the name pairing strategy.
///
/// Platforms differ in how they describe a rename: Linux sends one event with
/// both paths, Windows sends a `From` event and a `To` event, and macOS sends a
/// `From` and a single-path `Any`. All three are handled here, which is the
/// point of measuring rather than assuming.
fn feed_pair(pair: &mut Correlator, events: &[RawEvent]) -> Vec<RenameFact> {
	let mut facts = Vec::new();

	for (kind, paths) in events {
		if paths.is_empty() {
			continue;
		}
		if kind.contains("(Both)") && paths.len() >= 2 {
			if let Some(fact) = pair.pair(&paths[0], &paths[1]) {
				facts.push(fact);
			}
		} else if kind.contains("(From)") {
			pair.rename_from(&paths[0]);
		} else if kind.contains("(To)")
			&& let Some(fact) = pair.rename_to(&paths[0])
		{
			facts.push(fact);
		}
	}

	pair.end_batch();
	coalesce(dedupe(facts))
}

/// Feed a debounced batch to the identity correlator.
///
/// Order matters: appearances are observed before disappearances, because a
/// rename reports the old path as gone. Feeding the old path first would
/// discard the identity before the new path claimed it, which would defeat
/// identity correlation exactly where it is needed, on platforms that do not
/// report both names.
fn feed_identity(identity: &mut Correlator, events: &[RawEvent]) -> Vec<RenameFact> {
	let mut paths: Vec<PathBuf> = Vec::new();
	for (_, event_paths) in events {
		for path in event_paths {
			if !paths.contains(path) {
				paths.push(path.clone());
			}
		}
	}

	let mut facts = Vec::new();
	for path in &paths {
		if let Some(id) = identity_of(path) {
			let links = fs::symlink_metadata(path).map(|meta| links(&meta)).unwrap_or(1);
			if let Some(fact) = identity.observe(Sighting {
				path: path.clone(),
				identity: id,
				links,
			}) {
				facts.push(fact);
			}
		}
	}
	for path in &paths {
		if identity_of(path).is_none() {
			identity.forget(path);
		}
	}
	facts
}

fn drain(rx: &mpsc::Receiver<notify::Result<notify::Event>>, settle: Duration) -> Vec<RawEvent> {
	let mut events = Vec::new();
	let start = Instant::now();
	let mut deadline = start + settle;

	while Instant::now() < deadline && start.elapsed() < LIMIT {
		match rx.recv_timeout(QUIET) {
			Ok(Ok(event)) => {
				events.push((format!("{:?}", event.kind), event.paths));
				deadline = Instant::now() + QUIET;
			}
			Ok(Err(error)) => events.push((format!("error: {error}"), Vec::new())),
			Err(mpsc::RecvTimeoutError::Timeout) => {}
			Err(mpsc::RecvTimeoutError::Disconnected) => break,
		}
	}
	events
}

fn report(root: &Path, strategy: &str, facts: &[RenameFact]) {
	if facts.is_empty() {
		println!("{strategy}: no rename facts");
		return;
	}
	for fact in facts {
		println!(
			"{strategy}: {} ({}) -> {} intent={}",
			relative(root, &fact.source),
			fact.source_ext,
			relative(root, &fact.target),
			fact.intent
		);
	}
}

fn join(root: &Path, paths: &[PathBuf]) -> String {
	paths
		.iter()
		.map(|path| relative(root, path))
		.collect::<Vec<_>>()
		.join(" -> ")
}

fn relative(root: &Path, path: &Path) -> String {
	path.strip_prefix(root).unwrap_or(path).display().to_string()
}

#[cfg(unix)]
fn identity_of(path: &Path) -> Option<Identity> {
	use std::os::unix::fs::MetadataExt;
	let meta = fs::symlink_metadata(path).ok()?;
	Some(Identity {
		device: meta.dev(),
		inode: meta.ino(),
	})
}

#[cfg(not(unix))]
fn identity_of(_path: &Path) -> Option<Identity> {
	None
}

#[cfg(unix)]
fn links(meta: &fs::Metadata) -> u64 {
	use std::os::unix::fs::MetadataExt;
	meta.nlink()
}

#[cfg(not(unix))]
fn links(_meta: &fs::Metadata) -> u64 {
	1
}

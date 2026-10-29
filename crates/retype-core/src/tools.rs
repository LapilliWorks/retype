//! External tool detection.
//!
//! Mirrors the support matrix in `docs/tools.md`: a capability belongs to a
//! tool version with the right components present. Detection is a preflight,
//! never a runtime decision about what a plan may do; the planner reads the
//! configuration, and this reports what the configuration will find.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A tool the product can use, with the floor version and the components that
/// have to be present for the claimed formats to actually work.
#[derive(Debug)]
pub struct ToolSpec {
	pub name: &'static str,
	pub command: &'static str,
	pub probe: &'static [&'static str],
	pub min_version: &'static str,
	/// Substrings that must appear in the probe output, e.g. `heic` for the
	/// ImageMagick HEIC delegate.
	pub components: &'static [&'static str],
	pub purpose: &'static str,
}

pub const TOOLS: &[ToolSpec] = &[
	ToolSpec {
		name: "magick",
		command: "magick",
		probe: &["-version"],
		min_version: "7.1",
		components: &["heic", "webp"],
		purpose: "image conversion",
	},
	ToolSpec {
		name: "ffmpeg",
		command: "ffmpeg",
		probe: &["-version"],
		min_version: "6.0",
		components: &[],
		purpose: "audio and video conversion",
	},
	ToolSpec {
		name: "pandoc",
		command: "pandoc",
		probe: &["--version"],
		min_version: "3.0",
		components: &[],
		purpose: "document conversion",
	},
	ToolSpec {
		name: "7z",
		command: "7z",
		probe: &[],
		min_version: "23.0",
		components: &[],
		purpose: "archive conversion",
	},
];

/// PDF engines in preference order, per `docs/tools.md` §4.1.
pub const PDF_ENGINES: &[&str] = &["typst", "xelatex", "lualatex", "pdflatex", "weasyprint", "wkhtmltopdf"];

/// A dotted numeric version, comparable in the obvious way.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(Vec<u32>);

impl Version {
	/// First dotted numeric run in a line, e.g. `9.0.2` out of
	/// `ffmpeg version 9.0.2-4 Copyright`.
	pub fn parse(text: &str) -> Option<Self> {
		text.split_whitespace().find_map(|word| {
			let candidate: String = word.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
			let trimmed = candidate.trim_end_matches('.');
			if !trimmed.contains('.') {
				return None;
			}
			let parts: Vec<u32> = trimmed.split('.').filter_map(|p| p.parse().ok()).collect();
			(!parts.is_empty()).then_some(Self(parts))
		})
	}
}

impl fmt::Display for Version {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let parts: Vec<String> = self.0.iter().map(u32::to_string).collect();
		f.write_str(&parts.join("."))
	}
}

/// Outcome of probing one tool.
#[derive(Debug)]
pub struct ToolStatus {
	pub spec: &'static ToolSpec,
	pub path: Option<PathBuf>,
	pub version: Option<Version>,
	/// Components named by the spec that the probe output did not mention.
	pub missing_components: Vec<&'static str>,
	pub error: Option<String>,
}

impl ToolStatus {
	/// Present, new enough, and carrying every component the spec requires.
	pub fn is_usable(&self) -> bool {
		self.path.is_some() && self.version.is_some() && self.missing_components.is_empty() && self.error.is_none()
	}

	pub fn summary(&self) -> String {
		match (&self.path, &self.version) {
			(Some(_), Some(version)) => {
				let mut line = version.to_string();
				if !self.missing_components.is_empty() {
					line.push_str(&format!("  missing components: {}", self.missing_components.join(", ")));
				}
				line
			}
			(None, _) => "not found".to_string(),
			(Some(_), None) => "version unknown".to_string(),
		}
	}
}

/// Probe one tool.
pub fn probe(spec: &'static ToolSpec) -> ToolStatus {
	let path = find_in_path(&std::env::var_os("PATH").unwrap_or_default(), spec.command);

	let Some(path) = path else {
		return ToolStatus {
			spec,
			path: None,
			version: None,
			missing_components: spec.components.to_vec(),
			error: None,
		};
	};

	match Command::new(&path).args(spec.probe).output() {
		Ok(output) => {
			let stdout = String::from_utf8_lossy(&output.stdout);
			let stderr = String::from_utf8_lossy(&output.stderr);
			let text = format!("{stdout}{stderr}");

			// The version line is not always first: `7z` prints a blank line
			// and a banner before it.
			let version = text.lines().find_map(Version::parse);
			let lowered = text.to_lowercase();
			let missing_components = spec
				.components
				.iter()
				.copied()
				.filter(|component| !lowered.contains(&component.to_lowercase()))
				.collect();

			ToolStatus {
				spec,
				path: Some(path),
				version,
				missing_components,
				error: None,
			}
		}
		Err(error) => ToolStatus {
			spec,
			path: Some(path),
			version: None,
			missing_components: spec.components.to_vec(),
			error: Some(error.to_string()),
		},
	}
}

/// Probe every known tool.
pub fn probe_all() -> Vec<ToolStatus> {
	TOOLS.iter().map(probe).collect()
}

/// First PDF engine present, per the preference order.
pub fn resolve_pdf_engine() -> Option<&'static str> {
	let path_env = std::env::var_os("PATH").unwrap_or_default();
	PDF_ENGINES
		.iter()
		.copied()
		.find(|candidate| find_in_path(&path_env, candidate).is_some())
}

/// Whether a tool is new enough, when a version was parsed at all.
pub fn meets_minimum(status: &ToolStatus) -> bool {
	match &status.version {
		Some(version) => Version::parse(status.spec.min_version).is_none_or(|minimum| *version >= minimum),
		None => false,
	}
}

/// Look a command up in a `PATH` value. Split out so it is testable without
/// mutating the process environment, which edition 2024 makes unsafe.
pub fn find_in_path(path_env: &OsStr, command: &str) -> Option<PathBuf> {
	let names: Vec<String> = if cfg!(windows) {
		vec![format!("{command}.exe"), command.to_string()]
	} else {
		vec![command.to_string()]
	};

	std::env::split_paths(path_env)
		.filter(|dir| !dir.as_os_str().is_empty())
		.flat_map(|dir| names.iter().map(move |name| dir.join(name)))
		.find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
	path.is_file()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn version_from_imagemagick() {
		let line = "Version: ImageMagick 7.1.2-34 Q16 x86_64 20261014 https://imagemagick.org";
		assert_eq!(Version::parse(line).unwrap().to_string(), "7.1.2");
	}

	#[test]
	fn version_from_ffmpeg() {
		let line = "ffmpeg version 9.0.2-4 Copyright (c) 2000-2026 the FFmpeg developers";
		assert_eq!(Version::parse(line).unwrap().to_string(), "9.0.2");
	}

	#[test]
	fn version_from_sevenzip() {
		assert_eq!(
			Version::parse("7-Zip 26.05 (x64) : Copyright (c) 1999-2026 Igor Pavlov : 2026-10-21")
				.unwrap()
				.to_string(),
			"26.5"
		);
	}

	#[test]
	fn version_from_pandoc() {
		assert_eq!(Version::parse("pandoc 3.11").unwrap().to_string(), "3.11");
	}

	#[test]
	fn version_absent() {
		assert!(Version::parse("no digits here").is_none());
		assert!(Version::parse("").is_none());
		assert!(Version::parse("build 2026").is_none());
	}

	#[test]
	fn version_ordering() {
		let a = Version::parse("7.1.2").unwrap();
		let b = Version::parse("7.10").unwrap();
		let c = Version::parse("9.0.2").unwrap();
		assert!(a < b);
		assert!(b < c);
	}

	#[test]
	fn minimum_satisfied() {
		let spec = &TOOLS[0];
		let mut status = ToolStatus {
			spec,
			path: Some(PathBuf::from("/usr/bin/magick")),
			version: Some(Version::parse("7.1.2").unwrap()),
			missing_components: Vec::new(),
			error: None,
		};
		assert!(meets_minimum(&status));
		assert!(status.is_usable());

		status.version = Some(Version::parse("6.9.11").unwrap());
		assert!(!meets_minimum(&status));

		status.version = Some(Version::parse("7.1.2").unwrap());
		status.missing_components = vec!["heic"];
		assert!(!status.is_usable());
	}

	#[test]
	fn path_lookup_ignores_empty_entries() {
		let dir = tempfile::tempdir().unwrap();
		let script = dir.path().join("probe-tool");
		std::fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();

		let path_env = std::ffi::OsString::from(format!(":/nonexistent:{}", dir.path().display()));
		assert!(find_in_path(&path_env, "probe-tool").is_some());
		assert!(find_in_path(&path_env, "absent-tool").is_none());
	}

	#[test]
	fn tool_table_is_consistent() {
		let names: Vec<&str> = TOOLS.iter().map(|spec| spec.name).collect();
		let mut unique = names.clone();
		unique.sort_unstable();
		unique.dedup();
		assert_eq!(names.len(), unique.len(), "tool names must be unique");

		for spec in TOOLS {
			assert!(
				Version::parse(spec.min_version).is_some(),
				"minimum version for {} must parse",
				spec.name
			);
		}
	}
}

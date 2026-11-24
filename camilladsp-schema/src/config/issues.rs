// CamillaDSP - A flexible tool for processing audio
// Copyright (C) 2026 Henrik Enquist
//
// This file is part of CamillaDSP.
//
// CamillaDSP is free software; you can redistribute it and/or modify it
// under the terms of either:
//
// a) the GNU General Public License version 3,
//    or
// b) the Mozilla Public License Version 2.0.
//
// You should have received copies of the GNU General Public License and the
// Mozilla Public License along with this program. If not, see
// <https://www.gnu.org/licenses/> and <https://www.mozilla.org/MPL/2.0/>.

//! The problems that validating a config finds, each with its location.

use serde::{Deserialize, Serialize};
use std::error;
use std::fmt;

/// One step on the way to the value an issue is about: a map key or a list index.
///
/// Serialized untagged, so a path comes out as a plain list such as
/// `["pipeline", 2, "names", 0]`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum PathElement {
    Key(String),
    Index(usize),
}

impl From<&str> for PathElement {
    fn from(key: &str) -> Self {
        PathElement::Key(key.to_string())
    }
}

impl From<&String> for PathElement {
    fn from(key: &String) -> Self {
        PathElement::Key(key.clone())
    }
}

impl From<String> for PathElement {
    fn from(key: String) -> Self {
        PathElement::Key(key)
    }
}

impl From<usize> for PathElement {
    fn from(index: usize) -> Self {
        PathElement::Index(index)
    }
}

/// Build a `Vec<PathElement>` from keys and indexes, `issue_path!["filters", name, "parameters"]`.
macro_rules! issue_path {
    ($($element:expr),* $(,)?) => {
        vec![$($crate::config::PathElement::from($element)),*]
    };
}
pub(crate) use issue_path;

/// Write a path the way the YAML parser does, `pipeline[2].names[0]`.
pub fn format_path(path: &[PathElement]) -> String {
    let mut text = String::new();
    for element in path {
        match element {
            PathElement::Key(key) => {
                if !text.is_empty() {
                    text.push('.');
                }
                text.push_str(key);
            }
            PathElement::Index(index) => {
                text.push_str(&format!("[{index}]"));
            }
        }
    }
    text
}

/// What sort of problem an issue is. The validator does not decide how serious
/// each kind is, that is up to whoever asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub enum IssueKind {
    /// The config is wrong as written.
    Invalid,
    /// A coefficient file or capture input file that the config names does not
    /// exist. CamillaDSP cannot run without it, but a config editor may want to
    /// accept the config anyway, since the file can be added afterwards.
    MissingFile,
    /// The config uses a device type that this build of CamillaDSP does not
    /// have, such as `Wasapi` on Linux. The rest of the config is still checked.
    Unsupported,
}

/// A single problem with a config.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Issue {
    /// Where in the config the problem is. Empty for the config as a whole.
    pub path: Vec<PathElement>,
    pub message: String,
    pub kind: IssueKind,
}

impl Issue {
    pub fn invalid(path: Vec<PathElement>, message: impl Into<String>) -> Self {
        Issue {
            path,
            message: message.into(),
            kind: IssueKind::Invalid,
        }
    }

    pub fn missing_file(path: Vec<PathElement>, message: impl Into<String>) -> Self {
        Issue {
            path,
            message: message.into(),
            kind: IssueKind::MissingFile,
        }
    }

    pub fn unsupported(path: Vec<PathElement>, message: impl Into<String>) -> Self {
        Issue {
            path,
            message: message.into(),
            kind: IssueKind::Unsupported,
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", format_path(&self.path), self.message)
        }
    }
}

/// Every problem found in a config, in the order they were found.
///
/// Validation returns this as its error, and any issue at all makes the config
/// invalid for CamillaDSP. Displayed as one issue per line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Issues {
    issues: Vec<Issue>,
}

impl Issues {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, issue: Issue) {
        self.issues.push(issue);
    }

    /// Record a problem with the config as written.
    pub fn invalid(&mut self, path: Vec<PathElement>, message: impl Into<String>) {
        self.push(Issue::invalid(path, message));
    }

    /// Record a file that the config names but that does not exist.
    pub fn missing_file(&mut self, path: Vec<PathElement>, message: impl Into<String>) {
        self.push(Issue::missing_file(path, message));
    }

    /// Take over the issues of a part of the config, whose paths start at `prefix`.
    pub fn nest(&mut self, prefix: Vec<PathElement>, other: Issues) {
        for mut issue in other.issues {
            let mut path = prefix.clone();
            path.append(&mut issue.path);
            issue.path = path;
            self.issues.push(issue);
        }
    }

    /// Take over the issues from a validation result, if it failed.
    pub fn nest_result<T>(&mut self, prefix: Vec<PathElement>, result: Result<T, Issues>) {
        if let Err(other) = result {
            self.nest(prefix, other);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.issues.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Issue> {
        self.issues.iter()
    }

    pub fn as_slice(&self) -> &[Issue] {
        &self.issues
    }

    pub fn into_vec(self) -> Vec<Issue> {
        self.issues
    }

    /// `Ok(value)` if no issues were found, otherwise the issues.
    pub fn into_result<T>(self, value: T) -> Result<T, Issues> {
        if self.issues.is_empty() {
            Ok(value)
        } else {
            Err(self)
        }
    }
}

impl From<Issue> for Issues {
    fn from(issue: Issue) -> Self {
        Issues {
            issues: vec![issue],
        }
    }
}

impl IntoIterator for Issues {
    type Item = Issue;
    type IntoIter = std::vec::IntoIter<Issue>;

    fn into_iter(self) -> Self::IntoIter {
        self.issues.into_iter()
    }
}

impl<'a> IntoIterator for &'a Issues {
    type Item = &'a Issue;
    type IntoIter = std::slice::Iter<'a, Issue>;

    fn into_iter(self) -> Self::IntoIter {
        self.issues.iter()
    }
}

impl fmt::Display for Issues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, issue) in self.issues.iter().enumerate() {
            if n > 0 {
                writeln!(f)?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl error::Error for Issues {}

#[cfg(test)]
mod tests {
    use super::{Issue, Issues, format_path};

    #[test]
    fn path_is_written_like_the_yaml_parser() {
        let issue = Issue::invalid(issue_path!["pipeline", 2, "names", 0], "x");
        assert_eq!(format_path(&issue.path), "pipeline[2].names[0]");
        let issue = Issue::invalid(issue_path!["devices", "capture"], "x");
        assert_eq!(format_path(&issue.path), "devices.capture");
        assert_eq!(format_path(&[]), "");
    }

    #[test]
    fn nested_issues_get_the_prefix() {
        let mut inner = Issues::new();
        inner.invalid(issue_path!["freq"], "Frequency must be > 0");
        let mut outer = Issues::new();
        outer.nest(issue_path!["filters", "lp", "parameters"], inner);
        assert_eq!(
            outer.into_vec(),
            vec![Issue::invalid(
                issue_path!["filters", "lp", "parameters", "freq"],
                "Frequency must be > 0"
            )]
        );
    }

    #[test]
    fn one_issue_per_line() {
        let mut issues = Issues::new();
        issues.invalid(issue_path![], "first");
        issues.invalid(issue_path!["pipeline", 1], "second");
        assert_eq!(issues.to_string(), "first\npipeline[1]: second");
    }

    #[test]
    fn serializes_as_plain_lists() {
        let issue =
            Issue::missing_file(issue_path!["filters", "fir", "parameters", "filename"], "x");
        let json = yaml_serde::to_string(&issue).unwrap();
        assert!(json.contains("kind: MissingFile"), "{json}");
        let issue = Issue::invalid(issue_path!["pipeline", 2], "y");
        let yaml = yaml_serde::to_string(&Issues::from(issue)).unwrap();
        assert!(yaml.contains("- pipeline\n  - 2"), "{yaml}");
    }
}

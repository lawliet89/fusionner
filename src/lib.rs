//! [`fusionner`](https://github.com/lawliet89/fusionner) is French for merge.
//! This tool exists to create merge commits between your watched topic branches
//! and your target default branch.
//!
//! These merged commits can then be tested in your CI tool.
// Consider the diagram below:
//!
//! <img src="https://cdn.rawgit.com/lawliet89/fusionner/0d517230/images/branch_diagram.svg" style="width: 500px;" />
//!
//! Normally, tests will be run on the commit labelled `Pull Request`. Ideally, we would like to run tests
//! on a merge commit with your `master` branch. This is what `fusionner` does!
//!
//! If your `master` branch has moved on, `fusionner` will update the merge commit with the new commits from `master`.
//!
//! ## Prior Art
//!
//! `fusionner` is inspired by [bors](https://github.com/graydon/bors) and 
//! [`refs/pul/xxx/merge`](https://help.github.com/articles/checking-out-pull-requests-locally/) references that 
//! Github provides.
//!
//! ## Usage
//!
//! If you are looking for usage, refer to the [repository](https://github.com/lawliet89/fusionner). This documentation
//! is intended for using fusionner as a library in your Rust application.
//!

#![deny(missing_docs)]
#![doc(test(attr(allow(unused_variables), deny(warnings))))]

#[macro_use]
extern crate log;
#[macro_use]
extern crate serde_derive;

extern crate git2;
extern crate libgit2_sys as git2_raw;
extern crate regex;
extern crate serde;
extern crate toml;

#[cfg(test)]
extern crate rand;
#[cfg(test)]
extern crate tempdir;
#[cfg(test)]
extern crate url;

#[macro_use]
mod utils;
#[cfg(test)]
#[macro_use]
mod test;
pub mod merger;
pub mod git;

use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::str::FromStr;
use std::string::ParseError;
use std::vec::Vec;

use regex::RegexSet;

#[derive(Deserialize, Serialize, Eq, PartialEq, Clone, Debug)]
/// Configuration struct for the repository
pub struct RepositoryConfiguration {
    /// URI to the repository remote.
    pub uri: String,
    /// Path to checkout the repository to locally
    pub checkout_path: String,
    /// Fetch refspecs to add to the repository being checked out.
    /// You should make sure that the references you are watching is covered by these refspecs
    pub fetch_refspecs: Vec<String>,
    /// Push refspecs to add to the repository being checked out.
    /// This is not really useful at the moment because fusionner will always merge to
    /// refs/fusionner/* and automatically adds the right refspecs for you
    pub push_refspecs: Vec<String>,
    // Authentication Options
    /// Username to authenticate with the remote
    pub username: Option<String>,
    /// Password to authenticate with the remote
    pub password: Option<Password>,
    /// Path to private key to authenticate with the remote. If the remote requrests for a key and
    /// this is not specified, we will try to request the key from ssh-agent
    pub key: Option<String>,
    /// Passphrase to the private key for authentication
    pub key_passphrase: Option<Password>,
    /// The name to create merge commits under.
    /// If unspecified, will use the global configuration in Git. Otherwise we will use some generic one
    pub signature_name: Option<String>,
    /// The email to create merge commits under.
    /// If unspecified, will use the global configuration in Git. Otherwise we will use some generic one
    pub signature_email: Option<String>,
}

#[derive(Deserialize, Serialize, PartialOrd, Eq, PartialEq, Clone)]
/// A tuple struct to hold passwords. Implements `fmt::Display` and `fmt::Debug` to not leak during printing
pub struct Password {
    /// The wrapped password string
    pub password: String,
}

impl Password {
    /// Create a new password struct
    pub fn new(password: &str) -> Password {
        Password { password: password.to_string() }
    }
}

impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "***")
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "***")
    }
}

impl Deref for Password {
    type Target = str;

    fn deref(&self) -> &str {
        &*self.password
    }
}

impl FromStr for Password {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Password, ParseError> {
        Ok(Password::new(s))
    }
}

#[derive(Debug)]
/// Convenience struct to hold references to watch for changes to be merged into some `target_reference`.
pub struct WatchReferences {
    regex_set: RegexSet,
    exact_list: Vec<String>,
}

impl WatchReferences {
    /// Create watch references based on a list of exact references, or some regular expressions
    /// that will match to references.
    pub fn new<T: AsRef<str>>(exacts: &[T], regexes: &[T]) -> Result<WatchReferences, regex::Error>
        where T: std::fmt::Display
    {
        let exact_list = exacts.iter().map(|s| s.to_string()).collect();
        let regex_set = RegexSet::new(regexes)?;

        Ok(WatchReferences {
            regex_set: regex_set,
            exact_list: exact_list,
        })
    }

    /// Given a set of Remote heads as advertised by the remote, return a set of remtoe heads
    /// which exist based on the watch references
    pub fn resolve_watch_refs(&self, remote_ls: &[git::RemoteHead]) -> HashSet<String> {
        let mut refs = HashSet::new();

        // Flatten and resolve symbolic targets
        let remote_ls: Vec<String> = remote_ls.iter().map(|r| r.flatten_clone()).collect();

        for exact_match in self.exact_list.iter().filter(|s| remote_ls.contains(s)) {
            refs.insert(exact_match.to_string());
        }

        for regex_match in remote_ls.iter().filter(|s| self.regex_set.is_match(s)) {
            refs.insert(regex_match.to_string());
        }

        refs
    }
}

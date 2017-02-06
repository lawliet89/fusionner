use std::vec::Vec;
use std::collections::HashMap;

use super::git2;
use super::RepositoryConfiguration;
use super::git::{Repository, Remote};
#[macro_use]
use super::utils;

static DEFAULT_NOTES_NAMESPACE: &'static str = "fusionner";

pub struct Merger<'repo> {
    repository: &'repo Repository<'repo>,
    remote: Remote<'repo>,
    namespace: String,
}

/// A `Note` is stored for each commit on the topic branches' current head
#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Note {
    /// The commit hash for this topic branch's head
    pub merge_commit: String,
    /// The parent commit on the default branch for the merge commit
    pub default_parent: String,
    /// The reference for the merge commit, if any
    pub merge_reference: Option<String>,
}

impl<'repo> Merger<'repo> {
    pub fn new(repository: &'repo Repository<'repo>,
               remote: Option<&str>,
               namespace: Option<&str>)
               -> Result<Merger<'repo>, git2::Error> {
        let remote = repository.remote(remote)?;
        Ok(Merger {
            repository: repository,
            remote: remote,
            namespace: namespace.or(Some(DEFAULT_NOTES_NAMESPACE)).unwrap().to_string(),
        })
    }

    /// Add refspecs to a remote to fetch/push commit notes, specific for fusionner
    pub fn add_note_refspecs(&self) -> Result<(), git2::Error> {
        let refspec = format!("{0}/*:{0}/*", self.notes_reference_base());
        self.remote.add_refspec(&refspec, git2::Direction::Fetch)?;
        self.remote.add_refspec(&refspec, git2::Direction::Push)
    }

    pub fn fetch_notes(&mut self, commits: &[&str]) -> Result<(), git2::Error> {
        let refs: Vec<String> = commits.iter().map(|commit| self.note_ref(commit)).collect();
        let refs_refs: Vec<&str> = utils::as_str_slice(&refs);

        self.remote.fetch(&refs_refs)
    }

    /// Find note for commit. Make sure you have fetched them first
    pub fn find_note(&self, oid: git2::Oid) ->  Result<Note, git2::Error> {
        let notes_ref = self.notes_reference_base();
        let note = self.repository.repository.find_note(Some(&notes_ref), oid)?;
        note.message()
            .ok_or(git2::Error::from_str(&"Invalid message in note for oid"))
            .and_then(|note| utils::deserialize_toml(&note).map_err(|e| git2::Error::from_str(&e)))
    }

    fn note_ref(&self, commit: &str) -> String {
        format!("{}/{}", self.notes_reference_base(), commit)
    }

    fn notes_reference_base(&self) -> String {
        format!("refs/notes/{}", self.namespace)
    }
}

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
        let remote_name = self.remote.name().ok_or(git2::Error::from_str("Un-named remote used"))?;

        info!("Adding notes refspecs");
        if let None = Merger::find_matching_refspec(self.remote.refspecs(), git2::Direction::Fetch, &refspec) {
            info!("No existing fetch refpecs found: adding {}", refspec);
            self.repository.repository.remote_add_fetch(remote_name, &refspec)?;
        }

        if let None = Merger::find_matching_refspec(self.remote.refspecs(), git2::Direction::Push, &refspec) {
            info!("No existing push refpecs found: adding {}", refspec);
            self.repository.repository.remote_add_push(remote_name, &refspec)?;
        }
        Ok(())
    }

    pub fn fetch_notes(&mut self, commits: &[&str]) -> Result<(), git2::Error> {
        let refs: Vec<String> = commits.iter().map(|commit| self.note_ref(commit)).collect();
        let refs_refs: Vec<&str> = refs.iter().map(AsRef::as_ref).collect();

        self.remote.fetch(&refs_refs)
    }

    /// Find notes for commits. Make sure you have fetched them first
    pub fn find_notes(&self, commits: &[&str]) -> HashMap<String, Result<Note, git2::Error>> {
        let notes_ref = self.notes_reference_base();

        commits.iter()
            .map(|commit| (commit, git2::Oid::from_str(commit)))
            .map(|(commit, oid)| {
                (commit, oid.and_then(|oid| self.repository.repository.find_note(Some(&notes_ref), oid)))
            })
            .map(|(commit, note)| {
                let note = match note {
                    Err(e) => Err(e),
                    Ok(note) => {
                        note.message()
                            .ok_or(git2::Error::from_str(&"Invalid message in note for commit"))
                            .and_then(|note| utils::deserialize_toml(&note).map_err(|e| git2::Error::from_str(&e)))
                    }
                };

                (commit.to_string(), note)
            })
            .collect()
    }

    fn note_ref(&self, commit: &str) -> String {
        format!("{}/{}", self.notes_reference_base(), commit)
    }

    fn notes_reference_base(&self) -> String {
        format!("refs/notes/{}", self.namespace)
    }

    fn find_matching_refspec<'a>(mut refspecs: git2::Refspecs<'a>,
                                 direction: git2::Direction,
                                 refspec: &str)
                                 -> Option<git2::Refspec<'a>> {
        refspecs.find(|r| {
            let rs = r.str();
            enum_equals!(r.direction(), git2::Direction::Fetch) && rs.is_some() && rs.unwrap() == refspec
        })
    }
}

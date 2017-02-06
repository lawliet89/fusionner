use std::collections::HashMap;

use super::git2;
use super::RepositoryConfiguration;
use super::git::Repository;
#[macro_use]
use super::utils;

static DEFAULT_NOTES_NAMESPACE: &'static str = "refs/notes/fusionner";

pub struct Merger<'repo> {
    repository: &'repo Repository<'repo>,
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
    pub merge_reference: Option<String>
}

impl<'repo> Merger<'repo> {
    pub fn new(repository: &'repo Repository<'repo>, namespace: Option<&str>) -> Merger<'repo> {
        Merger { repository: repository, namespace: namespace.or(Some(DEFAULT_NOTES_NAMESPACE)).unwrap().to_string() }
    }

    /// Add refspecs to a remote to fetch/push commit notes, specific for fusionner
    pub fn add_note_refspecs(&self, remote: Option<&str>) -> Result<(), git2::Error> {
        let refspec = format!("{0}/*:{0}/*", self.namespace);
        let remote = self.repository.remote(remote)?;
        let remote_name = remote.name().ok_or(git2::Error::from_str("Un-named remote used"))?;

        info!("Adding notes refspecs");
        if let None = Merger::find_matching_refspec(remote.refspecs(), git2::Direction::Fetch, &refspec) {
            info!("No existing fetch refpecs found: adding {}", refspec);
            self.repository.repository.remote_add_fetch(remote_name, &refspec)?;
        }

        if let None = Merger::find_matching_refspec(remote.refspecs(), git2::Direction::Push, &refspec) {
            info!("No existing push refpecs found: adding {}", refspec);
            self.repository.repository.remote_add_push(remote_name, &refspec)?;
        }
        Ok(())
    }

    // pub fn fetch_notes<T: AsRef<str>>(&self, commits: &[T]) -> Result<HashMap<String, Option<Note>>, git2::Error> {
    //
    // }

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

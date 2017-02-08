use std::vec::Vec;

use super::{git2, git2_raw};
use super::git::{Repository, Remote};
use super::utils;

static DEFAULT_NOTES_NAMESPACE: &'static str = "fusionner";
static DEFAULT_NERGE_REFERENCE_BASE: &'static str = "refs/fusionner";
const NOTE_VERSION: u8 = 1;
static NOTE_ID: &'static str = "fusionner <https://github.com/lawliet89/fusionner>";

pub struct Merger<'repo> {
    repository: &'repo Repository<'repo>,
    remote: Remote<'repo>,
    namespace: String,
    merge_reference_namer: MergeReferenceNamer,
}

/// A `Note` is stored for each commit on the topic branches' current head
#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Note {
    /// For human readers to know where this is from
    pub _note_origin: String,
    /// Version of the note
    pub _version: u8,
    /// The commit hash for this topic branch's head
    pub merge_oid: String,
    /// The parent commit on the target branch for the merge commit
    pub target_parent_oid: String,
    /// Merge Parents, other than the target parent
    pub parents_oid: Vec<String>,
    /// The reference for the merge commit, if any
    pub merge_reference: Option<String>,
}

// TODO: Allow customizing of this, but only in code
pub enum MergeReferenceNamer {
    Default,
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
            merge_reference_namer: MergeReferenceNamer::Default,
        })
    }

    /// Add refspecs to a remote to fetch/push commit notes, specific for fusionner
    pub fn add_note_refspecs(&self) -> Result<(), git2::Error> {
        let src = self.notes_reference_base();
        let refspec = self.remote.generate_refspec(&src, true).map_err(|e| git_err!(&e))?;

        self.remote.add_refspec(&refspec, git2::Direction::Fetch)?;
        self.remote.add_refspec(&refspec, git2::Direction::Push)
    }

    pub fn fetch_notes(&mut self, commits: &[&str]) -> Result<(), git2::Error> {
        let refs: Vec<String> = commits.iter().map(|commit| self.note_ref(commit)).collect();
        let refs_refs: Vec<&str> = utils::as_str_slice(&refs);

        self.remote.fetch(&refs_refs)
    }

    /// Find note for commit. Make sure you have fetched them first
    pub fn find_note(&self, oid: git2::Oid) -> Result<Note, git2::Error> {
        let notes_ref = self.notes_reference_base();
        let note = self.repository.repository.find_note(Some(&notes_ref), oid)?;
        note.message()
            .ok_or(git_err!(&"Invalid message in note for oid"))
            .and_then(|note| super::deserialize_toml(&note).map_err(|e| git_err!(&e)))
    }

    /// Returns OID of note
    pub fn add_note(&self, note: &Note, oid: git2::Oid) -> Result<git2::Oid, git2::Error> {
        let signature = self.repository.signature()?;
        let serialized_note = super::serialize_toml(&note).map_err(|e| git_err!(&e))?;

        self.repository.repository.note(&signature,
                                        &signature,
                                        Some(&self.notes_reference_base()),
                                        oid,
                                        &serialized_note,
                                        true)
    }

    /// Determine if a merge should be made
    pub fn should_merge(&self, oid: git2::Oid, target_oid: git2::Oid) -> (bool, Option<Note>) {
        info!("Deciding if we should merge {} into {}", oid, target_oid);
        let note = self.find_note(oid);
        debug!("Note search result: {:?}", note);
        match note {
            Err(_) => (true, None),
            Ok(note) => {
                let oid = git2::Oid::from_str(&note.target_parent_oid);
                let result = match oid {
                    Err(_) => true,
                    Ok(oid) => oid != target_oid,
                };
                (result, Some(note))
            }
        }
    }

    /// Performs a merge and return a note intended for `oid`
    pub fn merge(&self,
                 oid: git2::Oid,
                 target_oid: git2::Oid,
                 reference: &str,
                 target_reference: &str)
                 -> Result<Note, git2::Error> {
        let our_commit = self.repository.repository.find_commit(target_oid)?;
        let their_commit = self.repository.repository.find_commit(oid)?;

        debug!("Merging index");
        let mut merged_index = self.repository.repository.merge_commits(&our_commit, &their_commit, None)?;
        if index_in_conflict(&mut merged_index.iter()) {
            return Err(git_err!("Index is in conflict after merge -- skipping"));
        }

        debug!("Writing tree");
        let tree_oid = merged_index.write_tree_to(&self.repository.repository)?;
        debug!("Tree OID {}", tree_oid);
        let tree = self.repository.repository.find_tree(tree_oid)?;

        let commit_reference = self.merge_reference_namer.resolve(reference, target_reference, oid, target_oid);
        info!("Merge will be created with reference {}", commit_reference);
        if let Ok(mut commit_reference_lookup) = self.repository.repository.find_reference(&commit_reference) {
            info!("Existing reference exists -- deleting");
            commit_reference_lookup.delete()?;
        }

        let signature = self.repository.signature()?;
        let commit_message = Merger::merge_commit_message(oid, target_oid);
        let merge_oid = self.repository
            .repository
            .commit(Some(&commit_reference),
                    &signature,
                    &signature,
                    &commit_message,
                    &tree,
                    &[&our_commit, &their_commit])?;

        Ok(Note::new(merge_oid, target_oid, &[oid], Some(&commit_reference)))
    }

    pub fn push(&mut self) -> Result<(), git2::Error> {
        info!("Pushing with configured refspecs");
        self.remote.push(&[])
    }

    fn merge_commit_message(base_oid: git2::Oid, target_oid: git2::Oid) -> String {
        format!("Merge {} into {}", base_oid, target_oid)
    }

    fn note_ref(&self, commit: &str) -> String {
        format!("{}/{}", self.notes_reference_base(), commit)
    }

    fn notes_reference_base(&self) -> String {
        format!("refs/notes/{}", self.namespace)
    }
}

impl Note {
    fn new(merge_oid: git2::Oid,
           target_parent_oid: git2::Oid,
           parents: &[git2::Oid],
           merge_reference: Option<&str>)
           -> Note {
        Note {
            _note_origin: NOTE_ID.to_string(),
            _version: NOTE_VERSION,
            merge_oid: format!("{}", merge_oid),
            target_parent_oid: format!("{}", target_parent_oid),
            parents_oid: parents.iter().map(|oid| format!("{}", oid)).collect(),
            merge_reference: merge_reference.and_then(|s| Some(s.to_string())),
        }
    }
}

impl MergeReferenceNamer {
    pub fn resolve(&self, reference: &str, _target_reference: &str, _oid: git2::Oid, _target_oid: git2::Oid) -> String {
        match self {
            &MergeReferenceNamer::Default => {
                format!("{}/{}",
                        DEFAULT_NERGE_REFERENCE_BASE,
                        reference.replace("refs/", ""))
            }
        }
    }

    pub fn reference(&self) -> String {
        DEFAULT_NERGE_REFERENCE_BASE.to_string()
    }

    pub fn add_default_refspecs(remote: &Remote) -> Result<(), git2::Error> {
        let src = MergeReferenceNamer::Default.reference();
        let refspec = remote.generate_refspec(&src, true).map_err(|e| git_err!(&e))?;
        remote.add_refspec(&refspec, git2::Direction::Push)
    }
}

/// Gets the stage number from a Git index entry
/// The meaning of the fields corresponds to core Git's documentation (in "Documentation/technical/index-format.txt").
fn git_index_entry_stage(entry: &git2::IndexEntry) -> u16 {
    (entry.flags & git2_raw::GIT_IDXENTRY_STAGEMASK) >> git2_raw::GIT_IDXENTRY_STAGESHIFT
}

/// From the stage number of a Git Index entry, determine if it's in conflict
/// https://libgit2.github.com/libgit2/#HEAD/group/index/git_index_entry_is_conflict
fn git_index_entry_is_conflict(entry: &git2::IndexEntry) -> bool {
    git_index_entry_stage(entry) > 0
}

fn index_in_conflict(entries: &mut git2::IndexEntries) -> bool {
    entries.any(|ref entry| git_index_entry_is_conflict(entry))
}

#[cfg(test)]
mod tests {
    use merger::{Merger, Note};
    use git;
    use git2;

    fn head_oid(repo: &git::Repository) -> git2::Oid {
        let reference = not_err!(repo.repository.head());
        not_none!(reference.target())
    }

    fn make_note(oid: git2::Oid) -> Note {
        Note::new(oid, oid, &[oid], Some("refs/fusionner/some-merge"))
    }

    fn add_branch_commit(repo: &git::Repository) -> git2::Oid {
        let repo = &repo.repository;
        let mut index = repo.index().unwrap();
        let id = index.write_tree().unwrap();

        let tree = repo.find_tree(id).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("refs/heads/branch"), &sig, &sig, "branch",
                    &tree, &[]).unwrap()
    }

    #[test]
    fn default_note_refspecs_are_added() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, None));
        not_err!(merger.add_note_refspecs());

        let remote = repo.remote(None).unwrap();

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Fetch;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/fusionner:refs/remotes/origin/notes/fusionner" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Push;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/fusionner:refs/remotes/origin/notes/fusionner" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));
    }

    #[test]
    fn custom_note_refspecs_are_added() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar")));
        not_err!(merger.add_note_refspecs());

        let remote = repo.remote(None).unwrap();

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Fetch;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/foobar:refs/remotes/origin/notes/foobar" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Push;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/foobar:refs/remotes/origin/notes/foobar" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));
    }

    #[test]
    fn notes_are_added_and_retrieved() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar")));
        let oid = head_oid(&repo);

        let note = make_note(oid);
        not_err!(merger.add_note(&note, oid));

        let found_note = not_err!(merger.find_note(oid));

        assert_eq!(note, found_note);
    }

    #[test]
    fn should_merge_on_missing_note() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar")));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);

        let (should_merge, found_note) = merger.should_merge(branch_oid, oid);
        assert!(should_merge);
        assert!(found_note.is_none());
    }

    #[test]
    fn should_merge_on_unequal_target_oid() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar")));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);

        let note = make_note(branch_oid);
        not_err!(merger.add_note(&note, branch_oid));

        let (should_merge, found_note) = merger.should_merge(branch_oid, oid);
        assert!(should_merge);
        assert_eq!(note, not_none!(found_note));
    }

    #[test]
    fn should_not_merge_on_equal_target_oid() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar")));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);

        let mut note = make_note(branch_oid);
        note.target_parent_oid = format!("{}", oid);
        not_err!(merger.add_note(&note, branch_oid));

        let (should_merge, found_note) = merger.should_merge(branch_oid, oid);
        assert!(!should_merge);
        assert_eq!(note, not_none!(found_note));
    }
}

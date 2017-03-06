//! Contains the logic to create merge commits.
//!
//! Metadata from fusionner are stored as [git object notes](https://git-scm.com/docs/git-notes).
//! Read [this](https://git-scm.com/blog/2010/08/25/notes.html) for an introduction.
//! Each commit handled by fusionner will have a `Note` serialised to TOML stored under some configured
//! namespace. The default is `fusionner`. Each note may contain multiple `Merge`s.
//!
//! You can list the notes in your command line by running `git notes --ref fusionner list`.
//!
//! # Terminology
//! `oid` and `reference` refer to the OID and Git reference for the commit that you are intending to merge into
//! some `target_oid` and `target_reference`. The former pair usually corresponds to some topic branch while the
//! latter pair is usually some default branch (i.e. `master`).

use std::collections::HashMap;
use std::fmt;
use std::vec::Vec;

use super::{git2, git2_raw};
use super::git::{Repository, Remote};
use super::utils;

static DEFAULT_NOTES_NAMESPACE: &'static str = "fusionner";
static DEFAULT_NERGE_REFERENCE_BASE: &'static str = "refs/fusionner";
const NOTE_VERSION: u8 = 1;
static NOTE_ID: &'static str = "fusionner <https://github.com/lawliet89/fusionner>";

/// Contains the logic to create, and manage merge commits
pub struct Merger<'repo, 'cb> {
    repository: &'repo Repository<'repo>,
    remote: Remote<'repo>,
    namespace: String,
    merge_reference_namer: MergeReferenceNamer<'cb>,
}

/// A `HashMap` of `Merge` where the key is a `String` corresponding to the `target_reference` of the merge.
/// This ensures that only one merge commit per `target_reference` is tracked.
pub type Merges = HashMap<String, Merge>;

/// A `Note` is stored for each commit on the topic branches' current head
#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Note {
    /// For human readers to know where this is from. A fixed string.
    pub _note_origin: String,
    /// Version of the note. Currently version 1
    pub _version: u8,
    /// List of merge commits for the current OID.
    /// This is a `HashMap` where the keys are the target references
    /// Because of the key, the list of Merges has the invariant that each target reference
    /// shall only have one entry each in the list of merge commits
    pub merges: Merges,
}

/// Denotes a single Merge commit for some target reference. Stored in a `Note`.
#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Merge {
    /// The OID for the merge commit
    pub merge_oid: String,
    /// The oid on the target branch that was used for the merge commit
    pub target_parent_oid: String,
    /// Reference of the target branch parent
    pub target_parent_reference: String,
    /// Any other merge parents, other than the target parent
    pub parents_oid: Vec<String>,
    /// The reference for the merge commit
    pub merge_reference: String,
}

/// Type for callback implementing custom merge reference naming.
/// The meanings for the input arguments are:
/// `Fn(reference: &str, target_reference: &str, oid: git2::Oid, target_oid: git2::Oid) -> String`
pub type MergeReferenceNamerCallback<'a> = Fn(&str, &str, git2::Oid, git2::Oid) -> String + 'a;

// TODO: Allow customizing of this, but only in code
/// Customise how the merge references are named.
pub enum MergeReferenceNamer<'cb> {
    /// The default namer will create a reference at `refs/fusionner/{reference}/{target}`
    /// where `{target}` is the target reference, and `{reference}~ is the reference that is being
    /// merged into target.
    ///
    /// _Note: The default namer will strip everything until the last `/` so make sure you don't use `/` in your
    /// branch names to avoid collision._
    Default,
    /// Use a function that will return the name of the merge reference
    /// # Example:
    /// ```
    /// use fusionner::merger::*;
    /// let cb = Box::new(move |reference : &str, target_ref : &str, _oid : _, _target_oid : _| {
    ///                     format!("refs/merge/{}/{}", target_ref, reference)
    ///                   });
    /// let namer = MergeReferenceNamer::Custom(cb);
    /// ```
    Custom(Box<MergeReferenceNamerCallback<'cb>>),
}

/// Enum returned by `Merger::should_merge` depending on the state of affairs
#[derive(Eq, PartialEq, Clone, Debug)]
pub enum ShouldMergeResult {
    /// A merge is required. This variant includes a `Note` for the commit, if one exists.
    /// If one exists, a `Merge` should be appended to this. Otherwise, a new `Note` with one `Merge` should be
    /// created
    Merge(Option<Note>),
    /// An up to date (i.e. target reference OID matches) merge commit is stored in the `Note` with the target
    /// reference exists. Nothing to do.
    ExistingMergeInSameTargetReference(Note),
    /// An up to date (i.e. target reference OID matches) merge commit is stored in the `Note`. The `proposed_merge`
    /// should be merged into the note
    ExistingMergeInDifferentTargetReference {
        /// Note found for commit
        note: Note,
        /// All the merges for this commit
        merges: Vec<Merge>,
        /// A proposed `Merge` that should be appended to the `Note`
        proposed_merge: Merge,
    },
}

impl<'repo, 'cb> Merger<'repo, 'cb> {
    /// Create a new merger.
    ///
    /// Provide the name of the remote to use, or the default (usually `origin`) will be used.
    ///
    /// # Examples
    /// ```
    /// use fusionner::RepositoryConfiguration;
    /// use fusionner::git::Repository;
    /// use fusionner::merger::*;
    ///
    /// let configuration = RepositoryConfiguration {
    ///     uri: "https://github.com/lawliet89/fusionner.git".to_string(),
    ///     checkout_path: "/tmp/checkout".to_string(),
    ///     fetch_refspecs: vec![],
    ///     push_refspecs: vec![],
    ///     username: None,
    ///     password: None,
    ///     key: None,
    ///     key_passphrase: None,
    ///     signature_name: None,
    ///     signature_email: None,
    /// };
    ///
    /// let repo = Repository::clone_or_open(&configuration).unwrap();
    /// let cb = Box::new(move |reference : &str, target_ref : &str, _oid : _, _target_oid : _| {
    ///                     format!("refs/merge/{}/{}", target_ref, reference)
    ///                   });
    /// let namer = MergeReferenceNamer::Custom(cb);
    /// let merger = Merger::new(&repo, None, None, Some(namer));
    /// ```
    pub fn new(repository: &'repo Repository<'repo>,
               remote_name: Option<&str>,
               namespace: Option<&str>,
               merge_reference_namer: Option<MergeReferenceNamer<'cb>>)
               -> Result<Merger<'repo, 'cb>, git2::Error> {
        let remote = repository.remote(remote_name)?;
        Ok(Merger {
            repository: repository,
            remote: remote,
            namespace: namespace.or_else(|| Some(DEFAULT_NOTES_NAMESPACE)).unwrap().to_string(),
            merge_reference_namer: merge_reference_namer.or(Some(MergeReferenceNamer::Default)).unwrap(),
        })
    }

    /// Add refspecs to a remote to fetch/push commit notes, specific for fusionner
    /// This is based on the `namespace` provided when creating a new `Merger`.
    pub fn add_note_refspecs(&self) -> Result<(), git2::Error> {
        let refspec = format!("+{0}:{0}", self.notes_reference());

        self.remote.add_refspec(&refspec, git2::Direction::Fetch)?;
        self.remote.add_refspec(&refspec, git2::Direction::Push)
    }

    /// Fetch notes based on the `namespace` provided when creating a new `Merger` from the remote configured
    pub fn fetch_notes(&mut self) -> Result<(), git2::Error> {
        let refs = [format!("+{0}:{0}", self.notes_reference())];

        self.remote.fetch(&utils::as_str_slice(&refs))
    }

    /// Find note for commit. Make sure you have fetched them first
    pub fn find_note(&self, oid: git2::Oid) -> Result<Note, git2::Error> {
        let notes_ref = self.notes_reference();
        let note = self.repository.repository.find_note(Some(&notes_ref), oid)?;
        note.message()
            .ok_or_else(|| git_err!(&"Invalid message in note for oid"))
            .and_then(|note| utils::deserialize_toml(note).map_err(|e| git_err!(&e)))
    }

    /// Add note for the OID. Will serialise to TOML before storage. Returns OID of note.
    pub fn add_note(&self, note: &Note, oid: git2::Oid) -> Result<git2::Oid, git2::Error> {
        let signature = self.repository.signature()?;
        let serialized_note = utils::serialize_toml(&note).map_err(|e| git_err!(&e))?;

        self.repository.repository.note(&signature,
                                        &signature,
                                        Some(&self.notes_reference()),
                                        oid,
                                        &serialized_note,
                                        true)
    }

    /// Determine if a merge should be made, based on the rules below.
    ///
    /// In general, you should prefer to use the convenience function `check_and_merge` instead which will do
    /// everything for you. For usage of this function, refer to the source code of `check_and_merge`.
    ///
    /// 1. Find the note for the `oid` in question.
    /// 2. If no note could be found, return `ShouldMergeResult::Merge(None)`.
    /// 3. If a note could be found, find from its list of `Merge`s if any merge with the `target_oid` could be found.
    /// 4. If none could be found, return `ShouldMergeResult::Merge(Some(note))` where `note` is the `Note` found.
    /// 5. If the found `Merge` has its `target_reference` match, then we will return
    /// `ShouldMergeResult::ExistingMergeInSameTargetReference`.
    /// 6. Otherwise, we will construct a `proposed_merge` and return
    /// `ShouldMergeResult::ExistingMergeInDifferentTargetReference`.
    pub fn should_merge(&self,
                        oid: git2::Oid,
                        target_oid: git2::Oid,
                        reference: &str,
                        target_reference: &str)
                        -> ShouldMergeResult {
        info!("Deciding if we should merge {} into {}", oid, target_oid);
        let note = self.find_note(oid);
        debug!("Note search result: {:?}", note);

        if note.is_err() {
            return ShouldMergeResult::Merge(None);
        }

        let note = note.unwrap();
        let matching_merges = note.find_matching_merges(target_oid);
        if matching_merges.is_empty() {
            ShouldMergeResult::Merge(Some(note.clone()))
        } else {
            match matching_merges.get(&target_reference.to_string()) {
                None => {
                    let commit_reference = self.merge_reference_namer
                        .resolve(reference, target_reference, oid, target_oid);
                    let &&Merge { ref merge_oid, .. } = matching_merges.values().take(1).collect::<Vec<_>>()[0];
                    // should be safe to unwrap
                    let merge_oid = git2::Oid::from_str(merge_oid).unwrap();
                    let proposed_merge = Merge::new(merge_oid,
                                                    target_oid,
                                                    target_reference,
                                                    &[oid],
                                                    &commit_reference);
                    ShouldMergeResult::ExistingMergeInDifferentTargetReference {
                        note: note.clone(),
                        merges: matching_merges.values().map(|merge| (*merge).clone()).collect(),
                        proposed_merge: proposed_merge,
                    }
                }
                Some(_) => ShouldMergeResult::ExistingMergeInSameTargetReference(note.clone()),
            }
        }
    }

    /// Performs a merge and return a `Merge` entry intended for `oid`. You should then add the `Merge` into the
    /// `Note` for `oid`.
    ///
    /// In general, you should prefer to use the convenience function `check_and_merge` instead which will do
    /// everything for you. For usage of this function, refer to the source code of `check_and_merge`.
    pub fn merge(&self,
                 oid: git2::Oid,
                 target_oid: git2::Oid,
                 reference: &str,
                 target_reference: &str)
                 -> Result<Merge, git2::Error> {
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
        let commit_message = Merger::merge_commit_message(oid, target_oid, reference, target_reference);
        let merge_oid = self.repository
            .repository
            .commit(Some(&commit_reference),
                    &signature,
                    &signature,
                    &commit_message,
                    &tree,
                    &[&our_commit, &their_commit])?;

        Ok(Merge::new(merge_oid,
                      target_oid,
                      target_reference,
                      &[oid],
                      &commit_reference))
    }


    /// Convenience function to check if a merge is required, and merge if needed.
    /// Will fetch remote merge references. Will push, if desired.
    /// This function calls both `should_merge` and `merge`.
    pub fn check_and_merge(&mut self,
                           oid: git2::Oid,
                           target_oid: git2::Oid,
                           reference: &str,
                           target_ref: &str,
                           push: bool)
                           -> Result<(Merge, ShouldMergeResult), git2::Error> {
        let should_merge = self.should_merge(oid, target_oid, reference, target_ref);
        info!("Merging {} ({}) into {} ({}): {}",
              reference,
              oid,
              target_ref,
              target_oid,
              should_merge);
        debug!("{:?}", should_merge);

        let mut push_reference = vec![self.notes_reference()];

        let merge = match should_merge {
            ShouldMergeResult::Merge(ref note) => {
                info!("Performing merge");
                let merge = self.merge(oid, target_oid, reference, target_ref)?;
                push_reference.push(merge.merge_reference.to_string());

                let note = match *note {
                    None => Note::new_with_merge(merge.clone()),
                    Some(ref note) => {
                        let mut note = note.clone();
                        note.append_with_merge(merge.clone());
                        note
                    }
                };

                info!("Adding note: {:?}", note);
                self.add_note(&note, oid)?;
                merge
            }
            ShouldMergeResult::ExistingMergeInSameTargetReference(ref note) => {
                info!("Merge commit is up to date");
                // Should be safe to unwrap
                let merge = note.merges[target_ref].clone();
                // Fetch merge
                let fetch_refspec = [::git::RefspecStr::as_forced(&merge.merge_reference)];
                self.remote.fetch(&utils::as_str_slice(&fetch_refspec))?;
                merge
            }
            ShouldMergeResult::ExistingMergeInDifferentTargetReference { ref note, ref merges, ref proposed_merge } => {
                info!("Merge found under other target references: {:?}", merges);
                let mut note = note.clone();
                note.append_with_merge(proposed_merge.clone());
                info!("Adding note: {:?}", note);
                self.add_note(&note, oid)?;
                // Fetch merge
                let fetch_refspec = [::git::RefspecStr::as_forced(&proposed_merge.merge_reference)];
                self.remote.fetch(&utils::as_str_slice(&fetch_refspec))?;
                proposed_merge.clone()
            }
        };

        if push {
            let refspecs: Vec<String> = push_reference.iter()
                .map(|s| ::git::RefspecStr::as_forced(s))
                .collect();
            let refspecs_slice: Vec<&str> = refspecs.iter().map(|s| &**s).collect();
            info!("Pushing to {:?}", refspecs);
            self.remote.push(&refspecs_slice)?;
        }

        Ok((merge, should_merge))
    }

    fn merge_commit_message(base_oid: git2::Oid,
                            target_oid: git2::Oid,
                            reference: &str,
                            target_reference: &str)
                            -> String {
        format!("Merge {0} ({2}) into {1} ({3})",
                reference,
                target_reference,
                base_oid,
                target_oid)
    }

    /// Returns the reference for the notes that fusionner will create, based on the namespace provided when
    /// creating a new `Merger`.
    pub fn notes_reference(&self) -> String {
        format!("refs/notes/{}", self.namespace)
    }
}

impl Note {
    /// Create a new `Note` with the list of `Merges`.
    pub fn new(merges: Merges) -> Note {
        Note {
            _note_origin: NOTE_ID.to_string(),
            _version: NOTE_VERSION,
            merges: merges,
        }
    }

    /// Convenience function to create a new `Note` with one `Merge`.
    pub fn new_with_merge(merge: Merge) -> Note {
        Self::new([(merge.target_parent_reference.to_string(), merge)].iter().cloned().collect())
    }

    /// Appends `Merge` to the `Note`, preserving the invariant that one `Merge` exists per `target_reference`.
    /// Returns the previous `Merge` if it existed
    pub fn append_with_merge(&mut self, merge: Merge) -> Option<Merge> {
        self.merges.insert(merge.target_parent_reference.to_string(), merge)
    }

    /// Find `Merge`s in the note tha corresponds to `target_oid`, regardless of their `target_reference`.
    pub fn find_matching_merges(&self, target_oid: git2::Oid) -> HashMap<&String, &Merge> {
        self.merges
            .iter()
            .filter(|&(_target_parent_reference, merge)| {
                let oid = git2::Oid::from_str(&merge.target_parent_oid);
                match oid {
                    Err(_) => false,
                    Ok(oid) => oid == target_oid,
                }
            })
            .collect()
    }
}

impl Merge {
    /// Creates a new `Merge`.
    pub fn new(merge_oid: git2::Oid,
               target_parent_oid: git2::Oid,
               target_parent_reference: &str,
               parents: &[git2::Oid],
               merge_reference: &str)
               -> Merge {
        Merge {
            merge_oid: format!("{}", merge_oid),
            target_parent_oid: format!("{}", target_parent_oid),
            target_parent_reference: target_parent_reference.to_string(),
            parents_oid: parents.iter().map(|oid| format!("{}", oid)).collect(),
            merge_reference: merge_reference.to_string(),
        }
    }
}

impl<'cb> MergeReferenceNamer<'cb> {
    /// Returns the name of the merge reference based on the rules provided.
    pub fn resolve(&self, reference: &str, target_reference: &str, oid: git2::Oid, target_oid: git2::Oid) -> String {
        match *self {
            MergeReferenceNamer::Default => {
                format!("{}/{}/{}",
                        DEFAULT_NERGE_REFERENCE_BASE,
                        Self::reference_last_item(reference),
                        Self::reference_last_item(target_reference))
            }
            MergeReferenceNamer::Custom(ref cb) => cb(reference, target_reference, oid, target_oid),
        }
    }

    /// Return the "based" of the reference used in the `Default` namer
    fn default_reference_base() -> String {
        DEFAULT_NERGE_REFERENCE_BASE.to_string()
    }

    /// Add the refspecs used in the `Default` namer to the remote.
    pub fn add_default_refspecs(remote: &Remote) -> Result<(), git2::Error> {
        let src = MergeReferenceNamer::default_reference_base();
        let refspec = remote.generate_refspec(&src, true).map_err(|e| git_err!(&e))?;
        remote.add_refspec(&refspec, git2::Direction::Push)
    }

    fn reference_last_item(reference: &str) -> String {
        reference.split('/').last().or(Some("")).map(|s| s.to_string()).unwrap()
    }
}

impl fmt::Display for ShouldMergeResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let formatted = match *self {
            ShouldMergeResult::Merge(_) => "Merge required",
            ShouldMergeResult::ExistingMergeInSameTargetReference { .. } => "An up to date merge exists",
            ShouldMergeResult::ExistingMergeInDifferentTargetReference { .. } => {
                "A merge under another target reference exists"
            }
        };
        write!(f, "{}", formatted)
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
    use std::cell::Cell;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    use git;
    use git2;
    use rand;
    use rand::Rng;

    use merger::{Merger, Note, Merge, ShouldMergeResult, MergeReferenceNamer};

    fn head_oid(repo: &git::Repository) -> git2::Oid {
        let reference = not_err!(repo.repository.head());
        not_none!(reference.target())
    }

    fn make_merge(oid: git2::Oid, target_oid: git2::Oid, target_reference: &str) -> Merge {
        Merge::new(oid,
                   target_oid,
                   target_reference,
                   &[],
                   "refs/fusionner/some-merge")
    }

    fn make_note(oid: git2::Oid, target_oid: git2::Oid, target_reference: &str) -> Note {
        let merge = make_merge(oid, target_oid, target_reference);
        Note::new_with_merge(merge)
    }

    fn add_branch_commit(repo: &git::Repository) -> git2::Oid {
        add_branch_commit_with_reference(repo, "refs/heads/branch")
    }

    fn add_branch_commit_with_reference(repo: &git::Repository, reference: &str) -> git2::Oid {
        let repo = &repo.repository;
        let mut parent_commit = vec![];

        // Checkout tree if it exists
        let resolved_reference = repo.find_reference(reference);
        if let Ok(resolved_reference) = resolved_reference {
            let resolved_reference = resolved_reference.resolve().unwrap();
            let oid = resolved_reference.target().unwrap();
            let commit = repo.find_commit(oid).unwrap();
            let tree = commit.tree().unwrap();

            let mut checkout_builder = git2::build::CheckoutBuilder::new();
            checkout_builder.force();

            repo.checkout_tree(tree.as_object(), Some(&mut checkout_builder)).unwrap();
            parent_commit.push(commit);
        }

        let mut index = repo.index().unwrap();
        let workdir = repo.workdir().unwrap();
        let random_string = rand::thread_rng()
            .gen_ascii_chars()
            .take(10)
            .collect::<String>();
        let file = workdir.join(&random_string);
        println!("{:?}", file);

        {
            let mut random_file = File::create(&file).unwrap();
            random_file.write_all(random_string.as_bytes()).unwrap();
        }
        // Add file to index
        index.add_path(Path::new(&random_string)).unwrap();

        let id = index.write_tree_to(repo).unwrap();

        let tree = repo.find_tree(id).unwrap();
        let sig = repo.signature().unwrap();

        let parents: Vec<&git2::Commit> = parent_commit.iter().map(|c| c).collect();

        repo.commit(Some(reference), &sig, &sig, "branch", &tree, &parents)
            .unwrap()
    }

    #[test]
    fn default_note_refspecs_are_added() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, None, None));
        not_err!(merger.add_note_refspecs());

        let remote = repo.remote(None).unwrap();

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Fetch;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/fusionner:refs/notes/fusionner" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Push;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/fusionner:refs/notes/fusionner" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));
    }

    #[test]
    fn custom_note_refspecs_are_added() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));
        not_err!(merger.add_note_refspecs());

        let remote = repo.remote(None).unwrap();

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Fetch;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/foobar:refs/notes/foobar" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));

        not_none!(remote.refspecs().find(|r| {
            let refspec = r.str();
            let direction = git2::Direction::Push;
            refspec.is_some() && refspec.unwrap() == "+refs/notes/foobar:refs/notes/foobar" &&
            git::Remote::direction_eq(&r.direction(), &direction)
        }));
    }

    #[test]
    fn correct_merges_are_found_from_note() {
        let oid = not_err!(git2::Oid::from_str("78314aadffca88c53c91f8e3bea9b6d582504ba6"));
        let target_oid = not_err!(git2::Oid::from_str("99dbca6642980876d004188b52f01d01d48a82e0"));

        let merges = [("refs/heads/master".to_string(), make_merge(oid, target_oid, "refs/heads/master")),
                      ("refs/heads/develop".to_string(), make_merge(oid, target_oid, "refs/heads/develop"))]
            .iter()
            .cloned()
            .collect();
        let note = Note::new(merges);
        let matching_merges = note.find_matching_merges(target_oid);
        assert_eq!(2, matching_merges.len());
    }

    #[test]
    fn merge_smoke_test() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let should_merge = merger.should_merge(branch_oid, oid, reference, target_reference);
        assert_matches!(should_merge, ShouldMergeResult::Merge(None));

        // First merge completes successfully
        not_err!(merger.merge(branch_oid, oid, reference, target_reference));

        // Second merge to the same reference should not fail
        let merge = not_err!(merger.merge(branch_oid, oid, reference, target_reference));

        let note = Note::new_with_merge(merge);
        // We can add the note to the repository
        not_err!(merger.add_note(&note, branch_oid));

        // And we should not meed to merge again
        let should_merge = merger.should_merge(branch_oid, oid, reference, target_reference);
        assert_matches!(should_merge, ShouldMergeResult::ExistingMergeInSameTargetReference{..})
    }

    #[test]
    fn check_and_merge_smoke_test() {
        let (td, raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let mut merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let (merge, _should_merge) =
            not_err!(merger.check_and_merge(branch_oid, oid, reference, target_reference, false));
        assert_eq!(merge.target_parent_oid, format!("{}", oid));
        assert_eq!(merge.target_parent_reference, target_reference);
        assert_eq!(merge.parents_oid, vec![format!("{}", branch_oid)]);

        let merge_oid = not_err!(git2::Oid::from_str(&merge.merge_oid));
        not_err!(raw.find_commit(merge_oid));
    }

    #[test]
    fn notes_are_added_and_retrieved() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));
        let oid = head_oid(&repo);

        let note = make_note(oid, oid, "refs/heads/master");
        not_err!(merger.add_note(&note, oid));

        let found_note = not_err!(merger.find_note(oid));

        assert_eq!(note, found_note);
    }

    #[test]
    fn should_merge_on_missing_note() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let should_merge = merger.should_merge(branch_oid, oid, reference, target_reference);
        assert_matches!(should_merge, ShouldMergeResult::Merge(None));
    }

    #[test]
    fn should_not_merge_on_equal_target_oid_for_same_target_reference() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let note = make_note(branch_oid, oid, target_reference);
        not_err!(merger.add_note(&note, branch_oid));

        let should_merge = merger.should_merge(branch_oid, oid, reference, target_reference);
        assert_matches!(should_merge, ShouldMergeResult::ExistingMergeInSameTargetReference{ .. });
    }

    #[test]
    fn should_merge_on_unequal_target_oid_for_same_target_reference() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let note = make_note(branch_oid, oid, target_reference);
        not_err!(merger.add_note(&note, branch_oid));

        let new_branch_oid = add_branch_commit_with_reference(&repo, reference);

        assert!(branch_oid != new_branch_oid);

        let should_merge = merger.should_merge(new_branch_oid, oid, reference, target_reference);
        assert_matches!(should_merge, ShouldMergeResult::Merge(None));
    }

    #[test]
    fn should_not_merge_on_equal_target_oid_for_different_target_reference() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);
        let merger = not_err!(Merger::new(&repo, None, Some("foobar"), None));

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let note = make_note(branch_oid, oid, target_reference);
        not_err!(merger.add_note(&note, branch_oid));

        let new_target_reference = "refs/heads/develop";
        let should_merge = merger.should_merge(branch_oid, oid, reference, new_target_reference);
        assert_matches!(should_merge, ShouldMergeResult::ExistingMergeInDifferentTargetReference{ .. });
    }

    #[test]
    fn notes_only_has_latest_merge_for_target_reference() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);

        let oid = head_oid(&repo);
        let branch_oid = add_branch_commit(&repo);
        let _reference = "refs/heads/branch";
        let target_reference = "refs/heads/master";

        let mut note = make_note(branch_oid, oid, target_reference);

        let new_target_oid = add_branch_commit_with_reference(&repo, target_reference);
        let merge = make_merge(oid, new_target_oid, target_reference);
        let old_merge = not_none!(note.append_with_merge(merge));

        assert_eq!(format!("{}", oid), old_merge.target_parent_oid);
    }

    #[test]
    fn correct_default_merge_reference_is_returned() {
        let (td, _raw) = ::test::raw_repo_init();
        let config = ::test::config_init(&td);
        let repo = ::test::repo_init(&config);

        let oid = head_oid(&repo);

        let expected = "refs/fusionner/some-branch/master";
        let actual = MergeReferenceNamer::Default.resolve("refs/heads/some-branch", "refs/heads/master", oid, oid);
        assert_eq!(expected, actual);
    }

    #[test]
    fn custom_merge_reference_namer_is_invoked() {
        let hit = Cell::new(false);

        {
            let namer = MergeReferenceNamer::Custom(Box::new(|reference: &str,
                                                              target_reference: &str,
                                                              _oid: git2::Oid,
                                                              _target_oid: git2::Oid| {
                hit.set(true);

                format!("{};{}", reference, target_reference)
            }));

            let (td, _raw) = ::test::raw_repo_init();
            let config = ::test::config_init(&td);
            let repo = ::test::repo_init(&config);
            let merger = not_err!(Merger::new(&repo, None, Some("foobar"), Some(namer)));

            let oid = head_oid(&repo);
            let branch_oid = add_branch_commit(&repo);
            let reference = "refs/heads/branch";
            let target_reference = "refs/heads/master";

            let merge = not_err!(merger.merge(branch_oid, oid, reference, target_reference));

            assert_eq!("refs/heads/branch;refs/heads/master", merge.merge_reference);
        }
        assert!(hit.get());
    }
}

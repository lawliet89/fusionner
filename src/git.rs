use super::git2;
use std::vec::Vec;
use std::path::Path;
use std::str;
use super::RepositoryConfiguration;

pub struct Repository<'repo> {
    pub repository: git2::Repository,
    details: &'repo RepositoryConfiguration<'repo>,
}

pub struct Remote<'repo> {
    pub remote: git2::Remote<'repo>,
    repository: &'repo Repository<'repo>,
}

#[derive(Clone, Debug)]
pub struct RemoteHead {
    pub is_local: bool,
    pub oid: git2::Oid,
    pub loid: git2::Oid,
    pub name: String,
    pub symref_target: Option<String>,
}

impl RemoteHead {
    pub fn flatten(&self) -> &str {
        match self.symref_target {
            Some(ref s) => s,
            _ => &self.name,
        }
    }

    pub fn flatten_clone(&self) -> String {
        self.flatten().to_string()
    }
}

impl<'repo> Repository<'repo> {
    pub fn new(repository: git2::Repository,
               configuration: &'repo RepositoryConfiguration<'repo>)
               -> Repository<'repo> {
        Repository {
            repository: repository,
            details: configuration,
        }
    }

    pub fn clone_or_open(repo_details: &'repo RepositoryConfiguration<'repo>)
                         -> Result<Repository<'repo>, git2::Error> {
        Repository::open(repo_details).or_else(|err| if err.code() == git2::ErrorCode::NotFound {
            info!("Repository not found at {} -- cloning",
                  repo_details.checkout_path);
            Repository::clone(repo_details)
        } else {
            Err(err)
        })
    }

    pub fn open(repo_details: &'repo RepositoryConfiguration<'repo>) -> Result<Repository<'repo>, git2::Error> {
        info!("Opening repository at {}", &repo_details.checkout_path);
        git2::Repository::open(&repo_details.checkout_path).and_then(|repo| Ok(Repository::new(repo, repo_details)))
    }

    pub fn clone(repo_details: &'repo RepositoryConfiguration<'repo>) -> Result<Repository<'repo>, git2::Error> {
        let remote_callbacks = Repository::remote_callbacks(repo_details);

        let mut fetch_optoons = git2::FetchOptions::new();
        fetch_optoons.remote_callbacks(remote_callbacks);

        let mut repo_builder = git2::build::RepoBuilder::new();
        repo_builder.fetch_options(fetch_optoons);

        info!("Cloning repository from {} into {}",
              repo_details.uri,
              repo_details.checkout_path);
        repo_builder.clone(&repo_details.uri, &Path::new(&repo_details.checkout_path))
            .and_then(|repo| Ok(Repository::new(repo, repo_details)))
    }

    fn remote_callbacks(repo_details: &'repo RepositoryConfiguration<'repo>) -> git2::RemoteCallbacks<'repo> {
        debug!("Making remote authentication callbacks");
        let mut remote_callbacks = git2::RemoteCallbacks::new();
        let repo_details = repo_details.clone();
        remote_callbacks.credentials(move |_uri, username, cred_type| {
                let username = username.or_else(|| match repo_details.username {
                    Some(ref username) => Some(username),
                    None => None,
                });
                if cred_type.intersects(git2::USERNAME) && username.is_some() {
                    git2::Cred::username(username.as_ref().unwrap())
                } else if cred_type.intersects(git2::USER_PASS_PLAINTEXT) && username.is_some() &&
                          repo_details.password.is_some() {
                    git2::Cred::userpass_plaintext(username.as_ref().unwrap(),
                                                   repo_details.password.as_ref().unwrap())
                } else if cred_type.intersects(git2::SSH_KEY) && username.is_some() {
                    if repo_details.key.is_some() {
                        git2::Cred::ssh_key(username.unwrap(),
                                            None,
                                            Path::new(repo_details.key.as_ref().unwrap()),
                                            repo_details.key_passphrase.as_ref().map(|x| &**x))
                    } else {
                        git2::Cred::ssh_key_from_agent(username.unwrap())
                    }

                } else {
                    let config = git2::Config::open_default()?;
                    git2::Cred::credential_helper(&config, &repo_details.uri, username)
                }
            })
            .transfer_progress(|progress| {
                // TODO: Maybe throttle this, or update UI
                if progress.received_objects() == progress.total_objects() {
                    debug!("Resolving deltas {}/{}\r",
                           progress.indexed_deltas(),
                           progress.total_deltas());
                } else if progress.total_objects() > 0 {
                    debug!("Received {}/{} objects ({}) in {} bytes\r",
                           progress.received_objects(),
                           progress.total_objects(),
                           progress.indexed_objects(),
                           progress.received_bytes());
                }
                true
            })
            .sideband_progress(|data| {
                debug!("remote: {}", str::from_utf8(data).unwrap());
                true
            })
            .update_tips(|refname, a, b| {
                if a.is_zero() {
                    debug!("[new]     {:20} {}", b, refname);
                } else {
                    debug!("[updated] {:10}..{:10} {}", a, b, refname);
                }
                true
            });
        remote_callbacks
    }

    // Get default (origin) remote
    pub fn origin_remote(&self) -> Result<Remote, git2::Error> {
        Ok(Remote {
            remote: self.repository.find_remote("origin")?,
            repository: self,
        })
    }
}

impl<'repo> Remote<'repo> {
    pub fn connect<'connection>(&'connection mut self)
                                -> Result<git2::RemoteConnection<'repo, 'connection, 'connection>, git2::Error> {
        let callbacks = Repository::remote_callbacks(self.repository.details);
        info!("Connecting to remote");
        self.remote.connect(git2::Direction::Fetch, Some(callbacks), None)
    }

    pub fn disconnect(&mut self) {
        self.remote.disconnect();
    }

    pub fn remote_ls(&mut self) -> Result<Vec<RemoteHead>, git2::Error> {
        let mut connection = self.connect()?;
        info!("Retrieving remote references `git ls-remote`");
        let heads = connection.list()?;
        Ok(heads.iter()
            .map(|head| {
                RemoteHead {
                    is_local: head.is_local(),
                    oid: head.oid(),
                    loid: head.loid(),
                    name: head.name().to_string(),
                    symref_target: head.symref_target().map(|s| s.to_string()),
                }
            })
            .collect())
    }

    // Get the remote reference of renote HEAD (i.e. default branch)
    pub fn head(&mut self) -> Result<Option<String>, git2::Error> {
        let mut connection = self.connect()?;
        info!("Retrieving remote references `git ls-remote`");
        let heads = connection.list()?;
        Ok(Remote::resolve_head(heads))
    }

    // Resolve the remote HEAD (i.e. default branch) from a list of heads
    // and return the remote reference
    pub fn resolve_head(heads: &[git2::RemoteHead]) -> Option<String> {
        heads.iter()
            .find(|head| head.name() == "HEAD" && head.symref_target().is_some())
            .and_then(|head| Some(head.symref_target().unwrap().to_string()))
    }

    pub fn fetch(&mut self, refspecs: &[&str]) -> Result<(), git2::Error> {
        let mut fetch_options = git2::FetchOptions::new();
        let callbacks = Repository::remote_callbacks(self.repository.details);
        fetch_options.remote_callbacks(callbacks)
            .prune(git2::FetchPrune::On);

        self.remote.fetch(refspecs, Some(&mut fetch_options), None)?;
        self.remote.disconnect();
        Ok(())
    }
}

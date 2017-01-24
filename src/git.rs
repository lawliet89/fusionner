use super::git2;
use std::path::Path;
use super::RepositoryConfiguration;

pub struct Repository<'repo> {
    repository: git2::Repository,
    details: &'repo RepositoryConfiguration<'repo>,
}

#[derive(Debug)]
pub struct RemoteHead {
    pub oid: git2::Oid,
    pub loid: git2::Oid,
    pub name: String,
    pub symref_target: Option<String>,
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
            });
        remote_callbacks
    }

    // Get default (origin) remote
    fn origin_remote(&self) -> Result<git2::Remote, git2::Error> {
        let mut remote = self.repository.find_remote("origin")?;
        Ok(remote)
    }

    // Get remote references
    pub fn remote_refs(&self) -> Result<Vec<RemoteHead>, git2::Error> {
        let mut remote = self.origin_remote()?;
        if !remote.connected() {
            let callbacks = Repository::remote_callbacks(self.details);
            info!("Connecting to remote");
            // TODO: The library will panic! if credentials are needed...
            // http://alexcrichton.com/git2-rs/src/git2/remote.rs.html#101
            remote.connect(git2::Direction::Fetch, Some(callbacks), None)?;
        }

        let result;
        {
            info!("Retrieving remote references");
            let heads = remote.list()?;
            result = Ok(heads.iter()
                .map(|head| {
                    RemoteHead {
                        oid: head.oid(),
                        loid: head.loid(),
                        name: head.name().to_string(),
                        symref_target: head.symref_target().and_then(|s| Some(s.to_string())),
                    }
                })
                .collect());
        }
        remote.disconnect();
        result
    }
}

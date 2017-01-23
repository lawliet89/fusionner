use super::git2;
use std::path::Path;
use super::RepositoryConfiguration;

pub fn clone_or_open(repo_details: &RepositoryConfiguration) -> Result<Repository, git2::Error> {
    let repo = open(&repo_details.checkout_path).or_else(|err| if err.code() == git2::ErrorCode::NotFound {
        info!("Repository not found at {} -- cloning",
              repo_details.checkout_path);
        clone(repo_details)
    } else {
        Err(err)
    });
    repo
}

pub fn open(checkout_path: &str) -> Result<Repository, git2::Error> {
    info!("Opening repository at {}", checkout_path);
    Repository::open(checkout_path)
}

pub fn clone(repo_details: &RepositoryConfiguration) -> Result<Repository, git2::Error> {
    debug!("Making remote authentication callbacks");
    let mut remote_callbacks = git2::RemoteCallbacks::new();
    remote_callbacks.credentials(|_uri, username, cred_type| {
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
            } else if cred_type.intersects(git2::SSH_KEY) && repo_details.key.is_some() && username.is_some() {
                git2::Cred::ssh_key(username.unwrap(),
                                    None,
                                    Path::new(repo_details.key.as_ref().unwrap()),
                                    repo_details.key_passphrase.as_ref().map(|x| &**x))
            } else {
                Err(git2::Error::from_str("Missing credentials to authenticate with remote Git repository"))
            }
        })
        .transfer_progress(|progress| {
            // TODO: Maybe throttle this, or update UI
            debug!("Received Objects: {}/{}\nIndexed Objects: {}\nIndexed Deltas: {}/{}\nBytes Received: {}",
                   progress.received_objects(),
                   progress.total_objects(),
                   progress.indexed_objects(),
                   progress.indexed_deltas(),
                   progress.total_deltas(),
                   progress.received_bytes());
            true
        });

    let mut fetch_optoons = git2::FetchOptions::new();
    fetch_optoons.remote_callbacks(remote_callbacks);

    let mut repo_builder = git2::build::RepoBuilder::new();
    repo_builder.fetch_options(fetch_optoons);

    info!("Cloning repository from {} into {}",
          repo_details.uri,
          repo_details.checkout_path);
    repo_builder.clone(&repo_details.uri, &Path::new(&repo_details.checkout_path))
}

use std::path::{Path};
use tempdir::TempDir;
use url::Url;

use super::git2;

macro_rules! not_err {
    ($e:expr) => (match $e {
        Ok(e) => e,
        Err(e) => panic!("{} failed with {}", stringify!($e), e),
    })
}

macro_rules! is_err {
    ($e:expr) => (match $e {
        Ok(e) => panic!("{} did not return with an error, but with {}", stringify!($e), e),
        Err(e) => e,
    })
}

macro_rules! not_none {
    ($e:expr) => (match $e {
        Some(e) => e,
        None => panic!("{} failed with None", stringify!($e)),
    })
}

pub fn raw_repo_init() -> (TempDir, git2::Repository) {
    let td = TempDir::new("test").unwrap();
    let repo = git2::Repository::init(td.path()).unwrap();
    {
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "name").unwrap();
        config.set_str("user.email", "email").unwrap();
        let mut index = repo.index().unwrap();
        let id = index.write_tree().unwrap();

        let tree = repo.find_tree(id).unwrap();
        let sig = repo.signature().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();

        repo.remote("origin", &path2url(&td.path())).unwrap();
    }
    (td, repo)
}

pub fn config_init(tempdir: &TempDir) -> ::RepositoryConfiguration {
    let path = tempdir.path();
    ::RepositoryConfiguration {
        uri: path2url(&path),
        checkout_path: path.to_str().unwrap().to_string(),
        remote: None,
        notes_namespace: None,
        fetch_refspecs: vec![],
        push_refspecs: vec![],
        username: Some("foobar".to_string()),
        password: Some("password".to_string()),
        key: Some("/path/to/some.key".to_string()),
        key_passphrase: None,
        target_ref: None,
        signature_name: None,
        signature_email: None,
    }
}

pub fn repo_init<'a>(config: &'a ::RepositoryConfiguration) -> ::git::Repository<'a> {
    ::git::Repository::open(&config).unwrap()
}

pub fn path2url(path: &Path) -> String {
    Url::from_file_path(path).unwrap().to_string()
}

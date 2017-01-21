extern crate docopt;
extern crate env_logger;
extern crate git2;
#[macro_use]
extern crate log;
extern crate rustc_serialize;
extern crate toml;

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::Read;

use docopt::Docopt;
use git2::Repository;

const USAGE: &'static str = "
fusionner

Usage:
  fusionner <configuration-file>
  rcanary (-h | --help)
Options:
  -h --help     Show this screen.
";

#[derive(RustcDecodable, Debug)]
struct Args {
    arg_configuration_file: String,
}

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Config {
    repository: RepositoryConfiguration,
}

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct RepositoryConfiguration {
    uri: String,
    username: Option<String>,
    password: Option<String>,
    key: Option<String>,
    key_passphrase: Option<String>,
    checkout_path: String,
}

fn main() {
    env::set_var("RUST_LOG", "fusionner=debug"); // TODO: use a proper logger

    env_logger::init().unwrap();
    let args: Args = Docopt::new(USAGE)
        .and_then(|d| d.decode())
        .unwrap_or_else(|e| e.exit());

    let config = read_config(&args.arg_configuration_file)
        .map_err(|err| {
            panic!("failed to read configuration file {}: {}",
                   &args.arg_configuration_file,
                   err)
        })
        .unwrap();
    debug!("Configuration parsed {:?}", config);

    let repo = clone_or_open(&config.repository);
    match repo {
        Err(e) => println!("{:?}", e),
        Ok(_) => println!("Done"),
    }
}

fn read_config(path: &str) -> Result<Config, Box<Error>> {
    info!("Reading configuration from '{}'", path);
    let mut file = File::open(&path)?;
    let mut config_toml = String::new();
    file.read_to_string(&mut config_toml)?;

    let parsed_toml = toml::Parser::new(&config_toml)
        .parse()
        .expect("Error parsing config file");

    let config = toml::Value::Table(parsed_toml);
    toml::decode(config).ok_or_else(|| panic!("error deserializing config"))
}

fn clone_or_open(repo_details: &RepositoryConfiguration) -> Result<Repository, git2::Error> {
    let repo = open(&repo_details.checkout_path).or_else(|err| if err.code() == git2::ErrorCode::NotFound {
        clone(repo_details)
    } else {
        Err(err)
    });
    repo
}

fn open(checkout_path: &str) -> Result<Repository, git2::Error> {
    Repository::open(checkout_path)
}

fn clone(repo_details: &RepositoryConfiguration) -> Result<Repository, git2::Error> {
    let mut remote_callbacks = git2::RemoteCallbacks::new();
    remote_callbacks.credentials(|_uri, username, cred_type| {
            let username = username.or_else(|| match repo_details.username {
                Some(ref username) => Some(username),
                None => None,
            });
            if cred_type.intersects(git2::USERNAME) && username.is_some() {
                git2::Cred::username(username.as_ref().unwrap())
            } else if cred_type.intersects(git2::USER_PASS_PLAINTEXT) && username.is_some() && password.is_some() {
                git2::Cred::userpass_plaintext(username.as_ref().unwrap(),
                                               repo_details.password.as_ref().unwrap())
            } else if cred_type.intersects(git2::SSH_KEY) && repo_details.key.is_some() && username.is_some() {
                git2::Cred::ssh_key(username.unwrap(),
                                    None,
                                    std::path::Path::new(repo_details.key.as_ref().unwrap()),
                                    repo_details.key_passphrase.as_ref().map(|x| &**x))
            } else {
                Err(git2::Error::from_str("Missing credentials to authenticate with remote Git repository"))
            }
        })
        .sideband_progress(|text: &[u8]| {
            info!("{:?}", text);
            true
        });
    Err(git2::Error::from_str("Oh no"))
}

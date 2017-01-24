extern crate docopt;
extern crate env_logger;
extern crate git2;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rustc_serialize;
extern crate toml;

mod git;

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::Read;
use std::marker::PhantomData;

use docopt::Docopt;
use regex::Regex;

lazy_static! {
    static ref MATCH_ALL: Regex = Regex::new(".+").unwrap();
}

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

const DEFAULT_INTERVAL: u64 = 30;

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Config<'config> {
    repository: RepositoryConfiguration<'config>,
    interval: Option<u64>,
}

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct RepositoryConfiguration<'config> {
    uri: String,
    username: Option<String>,
    password: Option<String>,
    key: Option<String>,
    key_passphrase: Option<String>,
    checkout_path: String,
    merge_ref: Option<String>,
    watch_ref_regex: Option<String>,
    target_head: Option<String>, // TODO: Support specifying branch name instead of references
    phantom: PhantomData<&'config String>,
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

    match process(&config) {
        Ok(_) => std::process::exit(0),
        Err(err) => {
            println!("Error: {}", err);
            std::process::exit(1);
        }
    };

}

fn process(config: &Config) -> Result<(), String> {
    let repo = git::Repository::clone_or_open(&config.repository).map_err(|e| format!("{:?}", e))?;
    let interal_seconds = config.interval.or(Some(DEFAULT_INTERVAL)).unwrap();
    let interval = std::time::Duration::from_secs(interal_seconds);

    let watch_regex = match config.repository.watch_ref_regex {
        None => None,
        Some(ref regex) => Some(Regex::new(regex).map_err(|e| format!("{:?}", e))?)
    };

    // let target_head = match config.repository.target_head {
    //     Some(head) => {
    //         info!("Target Reference Specified: {}", head);
    //         let remote_refs = repo.remote_refs().map_err(|e| format!("{:?}", e))?;
    //
    //     }
    // };

    loop {
        if let Err(e) = process_loop(&repo, &watch_regex) {
            println!("Error: {:?}", e);
        }
        info!("Sleeping for {:?} seconds", interal_seconds);
        std::thread::sleep(interval);
    }

    Ok(())
}

fn process_loop(repo: &git::Repository, watch_regex: &Option<Regex>) -> Result<(), git2::Error> {
    let remote_refs = repo.remote_refs()?;
    let filtered_heads = remote_refs.iter().filter(|head| {
        watch_regex.as_ref().unwrap_or(&*MATCH_ALL).is_match(&head.name)
    });

    Ok(())
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

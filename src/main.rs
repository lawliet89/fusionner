extern crate docopt;
extern crate env_logger;
extern crate git2;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rustc_serialize;
extern crate toml;

mod git;

use std::env;
use std::fs::File;
use std::io::Read;
use std::marker::PhantomData;
use std::collections::HashSet;
use std::vec::Vec;

use docopt::Docopt;
use regex::{Regex, RegexSet};

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
    watch_refs: Vec<WatchReferenceConfiguration>,
    target_ref: Option<String>, // TODO: Support specifying branch name instead of references
    _marker: PhantomData<&'config String>,
}

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub enum WatchReferenceConfiguration {
    Exact(String),
    Regex(String),
}

/// "Compiled" watch reference
struct WatchReferences {
    regex_set: RegexSet,
    exact_list: Vec<String>,
}

fn main() {
    let return_code;
    {
        env::set_var("RUST_LOG", "fusionner=debug"); // TODO: use a proper logger

        env_logger::init().unwrap();
        let args: Args = Docopt::new(USAGE)
            .and_then(|d| d.decode())
            .unwrap_or_else(|e| e.exit());

        let config = read_config(&args.arg_configuration_file)
            .map_err(|err| {
                panic!("Failed to read configuration file {}: {}",
                       &args.arg_configuration_file,
                       err)
            })
            .unwrap();
        debug!("Configuration parsed {:?}", config);

        return_code = match process(&config) {
            Ok(_) => 0,
            Err(err) => {
                println!("Error: {}", err);
                1
            }
        };
    }
    println!("Exiting with code {}", return_code);
    std::process::exit(return_code);
}

fn process(config: &Config) -> Result<(), String> {
    let repo = git::Repository::clone_or_open(&config.repository).map_err(|e| format!("{:?}", e))?;
    let mut origin = repo.origin_remote().map_err(|e| format!("{:?}", e))?;

    let interal_seconds = config.interval.or(Some(DEFAULT_INTERVAL)).unwrap();
    let interval = std::time::Duration::from_secs(interal_seconds);

    let target_ref = resolve_target_ref(&config.repository.target_ref, &mut origin).map_err(|e| format!("{:?}", e))?;

    loop {
        if let Err(e) = process_loop(&config, &repo, &mut origin, &target_ref) {
            println!("Error: {:?}", e);
        }
        info!("Sleeping for {:?} seconds", interal_seconds);
        std::thread::sleep(interval);
    }

    Ok(())
}

fn process_loop(config: &Config,
                repo: &git::Repository,
                remote: &mut git::Remote,
                target_ref: &str)
                -> Result<(), git2::Error> {

    let remote_ls = remote.remote_ls()?; // Update remote heads
    let remote_ls: Vec<String> = remote_ls.iter().map(|r| r.flatten_clone()).collect();
    let watch_refs =
        compile_watch_refs(&config.repository.watch_refs).map_err(|e| git2::Error::from_str(&format!("{:?}", e)))?;
    let watch_heads = resolve_watch_refs(&watch_refs, &remote_ls);

    info!("Remote references matching watch references ({}): {:?}",
          watch_heads.len(),
          watch_heads);

    remote.disconnect();
    Ok(())
}

fn resolve_target_ref(target_ref: &Option<String>, remote: &mut git::Remote) -> Result<String, git2::Error> {
    match target_ref {
        &Some(ref reference) => {
            info!("Target Reference Specified: {}", reference);
            let remote_refs = remote.remote_ls()?;
            if let None = remote_refs.iter().find(|head| &head.name == reference) {
                return Err(git2::Error::from_str(&format!("Could not find {} on remote", reference)));
            }
            Ok(reference.to_string())
        }
        &None => {
            let head = remote.head()?;
            if let None = head {
                return Err(git2::Error::from_str("Could not find a default HEAD on remote"));
            }
            let head = head.unwrap();
            info!("Target Reference set to remote HEAD: {}", head);
            Ok(head)
        }
    }
}

fn compile_watch_refs(watchrefs_config: &[WatchReferenceConfiguration]) -> Result<WatchReferences, regex::Error> {
    let exact_list: Vec<String> = watchrefs_config.iter()
        .map(|watchref| match watchref {
            &WatchReferenceConfiguration::Exact(ref s) => Some(s.to_string()),
            _ => None,
        })
        .filter(|o| o.is_some())
        .map(|o| o.unwrap())
        .collect();
    info!("{} Exact Watch References", exact_list.len());

    let regex_set: Vec<String> = watchrefs_config.iter()
        .map(|watchref| match watchref {
            &WatchReferenceConfiguration::Regex(ref regex) => Some(regex.to_string()),
            _ => None,
        })
        .filter(|o| o.is_some())
        .map(|o| o.unwrap())
        .collect();
    info!("{} Regex Watch References", regex_set.len());
    let regex_set = RegexSet::new(regex_set)?;

    Ok(WatchReferences {
        regex_set: regex_set,
        exact_list: exact_list,
    })
}

fn resolve_watch_refs(watchrefs: &WatchReferences, remote_ls: &[String]) -> HashSet<String> {
    let mut refs = HashSet::new();

    for exact_match in watchrefs.exact_list.iter().filter(|s| remote_ls.contains(s)) {
        refs.insert(exact_match.to_string());
    }

    for regex_match in remote_ls.iter().filter(|s| watchrefs.regex_set.is_match(s)) {
        refs.insert(regex_match.to_string());
    }

    refs
}

fn read_config(path: &str) -> Result<Config, String> {
    info!("Reading configuration from '{}'", path);
    let mut file = File::open(&path).map_err(|e| format!("{:?}", e))?;
    let mut config_toml = String::new();
    file.read_to_string(&mut config_toml).map_err(|e| format!("{:?}", e))?;

    let parsed_toml = toml::Parser::new(&config_toml).parse();
    if let None = parsed_toml {
        return Err("Error parsing configuration TOML".to_string());
    }

    let config = toml::Value::Table(parsed_toml.unwrap());
    rustc_serialize::Decodable::decode(&mut toml::Decoder::new(config)).map_err(|e| format!("{:?}", e))?
}

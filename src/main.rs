extern crate docopt;
extern crate env_logger;
extern crate git2;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rustc_serialize;
extern crate toml;

#[macro_use]
mod utils;
mod merger;
mod git;

use std::env;
use std::fs::File;
use std::io::Read;
use std::marker::PhantomData;
use std::collections::{HashSet, HashMap};
use std::vec::Vec;

use docopt::Docopt;
use regex::{Regex, RegexSet};

const USAGE: &'static str = "
fusionner

Usage:
  fusionner [options] <configuration-file> (<watch-ref> | --watch-regex=<regex>)...
  fusionner -h | --help

Use with a <configuration-file> to specify your repository information.
Use <watch-ref> to define the Git references to watch for commits.
Use --watch-regex=<regex> instead to specify references that matches the Regex

Options:
  -h --help    Show this screen.
";

#[derive(RustcDecodable, Debug)]
struct Args {
    arg_configuration_file: String,
    flag_watch_regex: Vec<String>,
    arg_watch_ref: Vec<String>,
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
    checkout_path: String,
    remote: Option<String>,
    notes_namespace: Option<String>,
    fetch_refspecs: Vec<String>,
    push_refspecs: Vec<String>,
    // Authentication Options
    username: Option<String>,
    password: Option<String>,
    key: Option<String>,
    key_passphrase: Option<String>,
    // Matching settings
    merge_ref: Option<String>,
    target_ref: Option<String>, // TODO: Support specifying branch name instead of references
    _marker: PhantomData<&'config String>,
}

/// "Compiled" watch reference
#[derive(Debug)]
pub struct WatchReferences {
    regex_set: RegexSet,
    exact_list: Vec<String>,
}

impl WatchReferences {
    fn new<T: AsRef<str>>(exacts: &[T], regexes: &[T]) -> Result<WatchReferences, regex::Error>
        where T: std::fmt::Display
    {
        let exact_list = exacts.iter().map(|s| s.to_string()).collect();
        let regex_set = RegexSet::new(regexes)?;

        Ok(WatchReferences {
            regex_set: regex_set,
            exact_list: exact_list,
        })
    }
}

fn main() {
    let return_code;
    {
        env::set_var("RUST_LOG", "fusionner=debug"); // TODO: use a proper logger

        env_logger::init().unwrap();
        let args: Args = Docopt::new(USAGE)
            .and_then(|d| d.decode())
            .unwrap_or_else(|e| e.exit());

        debug!("Arguments parsed: {:?}", args);

        let config = read_config(&args.arg_configuration_file)
            .map_err(|err| {
                panic!("Failed to read configuration file {}: {}",
                       &args.arg_configuration_file,
                       err)
            })
            .unwrap();
        debug!("Configuration parsed {:?}", config);

        let watch_refs = WatchReferences::new(args.arg_watch_ref.as_slice(),
                                              args.flag_watch_regex.as_slice())
            .map_err(|err| panic!("Failed to compile watch reference regex: {:?}", err))
            .unwrap();

        info!("Watch Referemces: {:?}", watch_refs);

        return_code = match process(&config, &watch_refs) {
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

fn process(config: &Config, watch_refs: &WatchReferences) -> Result<(), String> {
    let repo = git::Repository::clone_or_open(&config.repository).map_err(|e| format!("{:?}", e))?;
    let remote_name = utils::to_option_str(&config.repository.remote);
    let mut remote = repo.remote(remote_name).map_err(|e| format!("{:?}", e))?;
    let mut merger =
        merger::Merger::new(&repo,
                            remote_name,
                            utils::to_option_str(&config.repository.notes_namespace)).map_err(|e| format!("{:?}", e))?;
    merger.add_note_refspecs().map_err(|e| format!("{:?}", e))?;

    remote.add_refspecs(&utils::as_str_slice(&config.repository.fetch_refspecs),
                      git2::Direction::Fetch)
        .map_err(|e| format!("{:?}", e))?;
    remote.add_refspecs(&utils::as_str_slice(&config.repository.push_refspecs),
                      git2::Direction::Push)
        .map_err(|e| format!("{:?}", e))?;

    let interal_seconds = config.interval.or(Some(DEFAULT_INTERVAL)).unwrap();
    let interval = std::time::Duration::from_secs(interal_seconds);

    let target_ref = resolve_target_ref(&config.repository.target_ref, &mut remote).map_err(|e| format!("{:?}", e))?;

    loop {
        if let Err(e) = process_loop(&config,
                                     &repo,
                                     &mut remote,
                                     &mut merger,
                                     watch_refs,
                                     &target_ref) {
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
                merger: &mut merger::Merger,
                watch_refs: &WatchReferences,
                target_ref: &str)
                -> Result<(), git2::Error> {

    info!("Retrieving remote heads");
    let remote_ls = remote.remote_ls()?; // Update remote heads
    let remote_ls: Vec<String> = remote_ls.iter().map(|r| r.flatten_clone()).collect();
    info!("{} remote heads found", remote_ls.len());
    debug!("{:?}", remote_ls);

    let watch_heads = resolve_watch_refs(&watch_refs, &remote_ls);

    info!("{} remote references matched watch references",
          watch_heads.len());
    debug!("{:?}", watch_heads);

    info!("Fetching matched remotes");
    remote.fetch(&watch_heads.iter().map(|s| s.as_str()).collect::<Vec<&str>>())?;

    info!("Finding references");
    let references: HashMap<String, git2::Reference> = watch_heads.iter()
        .map(|reference| {
            let resolved_reference = repo.repository
                .find_reference(reference)
                .and_then(|reference| reference.resolve());
            (reference.to_string(), resolved_reference)
        })
        .filter(|&(ref reference, ref resolved_reference)| {
            match resolved_reference {
                &Err(ref e) => {
                    warn!("Invalid reference {}: {:?}", reference, e);
                    false
                },
                &Ok(_) => true
            }
        })
        .map(|(reference, resolved_reference)| (reference, resolved_reference.unwrap()))
        .collect();

    info!("Resolving OID for references");
    let oids: HashMap<String, git2::Oid> = references.iter()
        .map(|(reference, resolved_reference)| {
            let oid = resolved_reference.target().ok_or(git2::Error::from_str("Unknown reference"));
            (reference.to_string(), oid)
        })
        .filter(|&(ref reference, ref oid)| {
            match oid {
                &Err(ref e) => {
                    warn!("Unable to find OID for reference {}: {:?}", reference, e);
                    false
                },
                &Ok(_) => true
            }
        })
        .map(|(reference, oid)| (reference, oid.unwrap()))
        .collect();
    debug!("{:?}", oids);

    info!("Fetching notes for commits");
    let commits : Vec<String> = oids.values().map(|oid| format!("{}", oid)).collect();
    merger.fetch_notes(utils::as_str_slice(&commits).as_slice())?;

    info!("Parsing notes for commits");
    let notes = merger.find_notes(&oids.values().map(|oid| *oid).collect::<Vec<git2::Oid>>());
    debug!("{:?}", notes);

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

    utils::deserialize_toml(&config_toml)
}

extern crate fusionner;
extern crate docopt;
extern crate fern;
extern crate git2;
extern crate libgit2_sys as git2_raw;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rustc_serialize;
extern crate time;
extern crate toml;
#[cfg(test)]
extern crate tempdir;
#[cfg(test)]
extern crate url;

#[macro_use]
mod utils;
#[cfg(test)]
#[macro_use]
mod test;

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::vec::Vec;

use fusionner::*;
use fusionner::{merger, git};
use docopt::Docopt;

const USAGE: &'static str = "
fusionner

Usage:
  fusionner [options] <configuration-file> (<watch-ref> | --watch-regex=<regex>)...
  fusionner -h | --help

Use with a <configuration-file> to specify your repository information.
Use <watch-ref> to define the Git references to watch for commits.
Use --watch-regex=<regex> instead to specify references that matches the Regex

Options:
  --remote=<remote>                 Name of the remote to use. [default: origin]
  --notes-namespace=<namespace>     Metadata generated by fusionner is stored as Git notes.
                                    Namespace for the Git notes that fusionner will create. [default: fusionner]
  --target-reference=<reference>    The target reference for references to be meged against. [default: HEAD]
  --log-level=<log-level>           The default log level is `info`.
                                    Can be set to `trace`, `debug`, `info`, `warn`, or `error` [default: info]
  -h --help                         Show this screen.
";

#[derive(RustcDecodable, Debug)]
struct Args {
    arg_configuration_file: String,
    flag_watch_regex: Vec<String>,
    flag_log_level: String,
    flag_target_reference: String,
    flag_remote: String,
    flag_notes_namespace: String,
    arg_watch_ref: Vec<String>,
}

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
/// Configuration for fusionner
pub struct Config {
    /// Repository configuration
    pub repository: RepositoryConfiguration,
    /// Interval, in seconds, between loops to look for new commits. Defaults to 30
    pub interval: Option<u64>,
}

const DEFAULT_INTERVAL: u64 = 30;

impl Config {
    /// Read configuration from a TOML file
    pub fn read_config(path: &str) -> Result<Config, String> {
        info!("Reading configuration from '{}'", path);
        let mut file = File::open(&path).map_err(|e| format!("{:?}", e))?;
        let mut config_toml = String::new();
        file.read_to_string(&mut config_toml).map_err(|e| format!("{:?}", e))?;

        utils::deserialize_toml(&config_toml)
    }
}

macro_rules! return_if_empty {
    ($x:expr, $err:expr) => {
        {
            let x = $x;
            match x.len() {
                0 => return Err($err),
                _ => x
            }
        }
    }
}

macro_rules! map_err {
    ($x:expr) => {
        $x.map_err(|e| format!("{:?}", e))
    }
}

fn main() {
    let return_code;
    {
        let args: Args = Docopt::new(USAGE)
            .and_then(|d| d.decode())
            .unwrap_or_else(|e| e.exit());

        let logger_config = configure_logger(Some(args.flag_log_level.clone()));
        if let Err(e) = fern::init_global_logger(logger_config, log::LogLevelFilter::Debug) {
            panic!("Failed to initialize global logger: {}", e);
        }

        debug!("Arguments parsed: {:?}", args);

        let config = Config::read_config(&args.arg_configuration_file)
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

        return_code = match process(&config,
                                    &watch_refs,
                                    Some(args.flag_target_reference),
                                    Some(args.flag_remote),
                                    Some(args.flag_notes_namespace)) {
            Ok(_) => 0,
            Err(err) => {
                error!("Error: {}", err);
                1
            }
        };
    }
    info!("Exiting with code {}", return_code);
    std::process::exit(return_code);
}

fn process(config: &Config,
           watch_refs: &WatchReferences,
           target_ref: Option<String>,
           remote_name: Option<String>,
           notes_namespace: Option<String>)
           -> Result<(), String> {
    // Create our structs
    let repo = map_err!(git::Repository::clone_or_open(&config.repository))?;
    let remote_name = to_option_str(&remote_name);
    let mut remote = map_err!(repo.remote(remote_name))?;
    let mut merger = map_err!(merger::Merger::new(&repo, remote_name, to_option_str(&notes_namespace), None))?;

    // Add the necessary refspecs
    map_err!(merger.add_note_refspecs())?;
    map_err!(merger::MergeReferenceNamer::add_default_refspecs(&remote))?;

    map_err!(remote.add_refspecs(&utils::as_str_slice(&config.repository.fetch_refspecs),
                                 git2::Direction::Fetch))?;
    map_err!(remote.add_refspecs(&utils::as_str_slice(&config.repository.push_refspecs),
                                 git2::Direction::Push))?;

    let target_ref = map_err!(remote.resolve_target_ref(to_option_str(&target_ref)))?;

    // Setup intervals
    let interal_seconds = config.interval.or(Some(DEFAULT_INTERVAL)).unwrap();
    let interval = std::time::Duration::from_secs(interal_seconds);

    loop {
        if let Err(e) = process_loop(&mut remote, &mut merger, watch_refs, &target_ref) {
            warn!("Error: {:?}", e);
        }
        info!("Sleeping for {:?} seconds", interal_seconds);
        std::thread::sleep(interval);
    }

    Ok(())
}

fn process_loop(remote: &mut git::Remote,
                merger: &mut merger::Merger,
                watch_refs: &WatchReferences,
                target_ref: &str)
                -> Result<(), git2::Error> {

    info!("Retrieving remote heads");
    let remote_ls = return_if_empty!(remote.remote_ls()?, git_err!("No remote references found"));

    info!("{} remote heads found", remote_ls.len());
    debug!("{:?}", remote_ls);

    let watch_heads = return_if_empty!(watch_refs.resolve_watch_refs(&remote_ls),
                                       git_err!("No matching watched reference found"));

    info!("{} remote references matched watch references",
          watch_heads.len());
    debug!("{:?}", watch_heads);

    info!("Fetching matched remotes and target reference");
    let mut fetch_refs = watch_heads.iter().map(|s| s.as_str()).collect::<Vec<&str>>();
    fetch_refs.push(target_ref);

    {
        let forced_fetch_refs: Vec<String> = fetch_refs.iter()
            .map(|s| git::RefspecStr::to_forced(s))
            .collect();
        let forced_fetch_refs_slice: Vec<&str> = forced_fetch_refs.iter().map(|s| &**s).collect();

        remote.fetch(&forced_fetch_refs_slice)?;
    }

    info!("Resolving references and oid");
    let oids: HashMap<String, git2::Oid> = resolve_oids(fetch_refs.as_slice(), remote_ls.as_slice())
        .iter()
        .filter(|&(reference, oid)| match oid {
            &None => {
                warn!("No OID found for reference {}", reference);
                false
            }
            &Some(_) => true,
        })
        .map(|(reference, oid)| (reference.to_string(), oid.unwrap()))
        .collect();
    let oids = return_if_empty!(oids, git_err!("No valid OIDs resolved"));
    debug!("{:?}", oids);

    info!("Resolving reference and OID for target reference");
    let target_oid = resolve_oid(target_ref, &remote_ls).ok_or(git_err!("Unable to find OID for target reference"))?;

    info!("Fetching notes for commits");
    merger.fetch_notes()?;

    let mut push_references = HashSet::<String>::new();
    for (reference, oid) in oids {
        match merger.check_and_merge(oid, target_oid, &reference, target_ref, true) {
            Ok((merge, _should_merge)) => {
                push_references.insert(merge.merge_reference);
            }
            Err(e) => {
                error!("Error processing {} ({}): {:?}", reference, oid, e);
            }
        }
    }

    remote.disconnect();
    Ok(())
}

// TODO: Support logging to file/stderr/etc.
fn configure_logger<'a>(log_level: Option<String>) -> fern::DispatchConfig<'a> {
    let log_level = resolve_log_level(&log_level)
        .or_else(|| {
            panic!("Unknown log level `{}``", log_level.as_ref().unwrap());
        })
        .unwrap();

    fern::DispatchConfig {
        format: Box::new(|msg: &str, level: &log::LogLevel, _location: &log::LogLocation| {
            format!("[{}][{}] {}",
                    time::now().strftime("%FT%T%z").unwrap(),
                    level,
                    msg)
        }),
        output: vec![fern::OutputConfig::stdout()],
        level: log_level,
    }
}

fn resolve_log_level(log_level: &Option<String>) -> Option<log::LogLevelFilter> {
    match to_option_str(log_level) {
        Some("trace") => Some(log::LogLevelFilter::Trace),
        Some("debug") => Some(log::LogLevelFilter::Debug),
        Some("warn") => Some(log::LogLevelFilter::Warn),
        Some("error") => Some(log::LogLevelFilter::Error),
        None | Some("info") => Some(log::LogLevelFilter::Info),
        Some(_) => None,
    }
}

fn resolve_oids(references: &[&str], remote_ls: &[git::RemoteHead]) -> HashMap<String, Option<git2::Oid>> {
    references.iter()
        .map(|reference| (reference.to_string(), resolve_oid(reference, remote_ls)))
        .collect()
}

fn resolve_oid(reference: &str, remote_ls: &[git::RemoteHead]) -> Option<git2::Oid> {
    match remote_ls.iter().find(|head| head.name == *reference) {
        Some(head) => Some(head.oid),
        None => None,
    }
}

fn to_option_str(opt: &Option<String>) -> Option<&str> {
    opt.as_ref().map(|s| &**s)
}

#[cfg(test)]
mod tests {
    use Config;

    #[test]
    fn config_reading_smoke_test() {
        not_err!(Config::read_config("tests/fixtures/config.toml"));
    }
}

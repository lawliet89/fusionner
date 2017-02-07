extern crate fusionner;
extern crate docopt;
extern crate env_logger;
extern crate git2;
extern crate libgit2_sys as git2_raw;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rustc_serialize;
extern crate toml;

mod utils;

use std::env;
use std::collections::HashMap;
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
  -h --help    Show this screen.
";

#[derive(RustcDecodable, Debug)]
struct Args {
    arg_configuration_file: String,
    flag_watch_regex: Vec<String>,
    arg_watch_ref: Vec<String>,
}

const DEFAULT_INTERVAL: u64 = 30;

fn main() {
    let return_code;
    {
        env::set_var("RUST_LOG", "fusionner=debug"); // TODO: use a proper logger

        env_logger::init().unwrap();
        let args: Args = Docopt::new(USAGE)
            .and_then(|d| d.decode())
            .unwrap_or_else(|e| e.exit());

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
    let remote_name = to_option_str(&config.repository.remote);
    let mut remote = repo.remote(remote_name).map_err(|e| format!("{:?}", e))?;
    let mut merger =
        merger::Merger::new(&repo,
                            remote_name,
                            to_option_str(&config.repository.notes_namespace)).map_err(|e| format!("{:?}", e))?;
    merger.add_note_refspecs().map_err(|e| format!("{:?}", e))?;
    merger::MergeReferenceNamer::add_default_refspecs(&remote).map_err(|e| format!("{:?}", e))?;

    remote.add_refspecs(&utils::as_str_slice(&config.repository.fetch_refspecs),
                      git2::Direction::Fetch)
        .map_err(|e| format!("{:?}", e))?;
    remote.add_refspecs(&utils::as_str_slice(&config.repository.push_refspecs),
                      git2::Direction::Push)
        .map_err(|e| format!("{:?}", e))?;

    let interal_seconds = config.interval.or(Some(DEFAULT_INTERVAL)).unwrap();
    let interval = std::time::Duration::from_secs(interal_seconds);

    let target_ref = config.repository.resolve_target_ref(&mut remote).map_err(|e| format!("{:?}", e))?;

    loop {
        if let Err(e) = process_loop(&repo, &mut remote, &mut merger, watch_refs, &target_ref) {
            println!("Error: {:?}", e);
        }
        info!("Sleeping for {:?} seconds", interal_seconds);
        std::thread::sleep(interval);
    }

    Ok(())
}

// TODO: Early return if nothing is found
fn process_loop(repo: &git::Repository,
                remote: &mut git::Remote,
                merger: &mut merger::Merger,
                watch_refs: &WatchReferences,
                target_ref: &str)
                -> Result<(), git2::Error> {

    info!("Retrieving remote heads");
    let remote_ls = remote.remote_ls()?; // Update remote heads

    info!("{} remote heads found", remote_ls.len());
    debug!("{:?}", remote_ls);

    let watch_heads = watch_refs.resolve_watch_refs(&remote_ls);

    info!("{} remote references matched watch references",
          watch_heads.len());
    debug!("{:?}", watch_heads);

    info!("Fetching matched remotes and target reference");
    let mut fetch_refs = watch_heads.iter().map(|s| s.as_str()).collect::<Vec<&str>>();
    fetch_refs.push(target_ref);
    remote.fetch(&fetch_refs)?;

    // TODO: Resolve via remote heads
    info!("Resolving references");
    let references: HashMap<String, git2::Reference> = watch_heads.iter()
        .map(|reference| {
            let resolved_reference = repo.repository
                .find_reference(reference)
                .and_then(|reference| reference.resolve());
            (reference.to_string(), resolved_reference)
        })
        .filter(|&(ref reference, ref resolved_reference)| match resolved_reference {
            &Err(ref e) => {
                warn!("Invalid reference {}: {:?}", reference, e);
                false
            }
            &Ok(_) => true,
        })
        .map(|(reference, resolved_reference)| (reference, resolved_reference.unwrap()))
        .collect();

    info!("Resolving OID for references");
    let oids: HashMap<String, git2::Oid> = references.iter()
        .map(|(reference, resolved_reference)| {
            let oid = resolved_reference.target().ok_or(git2::Error::from_str("Unknown reference"));
            (reference.to_string(), oid)
        })
        .filter(|&(ref reference, ref oid)| match oid {
            &Err(ref e) => {
                warn!("Unable to find OID for reference {}: {:?}", reference, e);
                false
            }
            &Ok(_) => true,
        })
        .map(|(reference, oid)| (reference, oid.unwrap()))
        .collect();
    debug!("{:?}", oids);

    info!("Resolving reference and OID for target reference");
    let resolved_target = repo.repository
        .find_reference(target_ref)
        .and_then(|reference| reference.resolve())?;
    let target_oid = resolved_target.target().ok_or(git2::Error::from_str("Unable to find OID for target reference"))?;

    info!("Fetching notes for commits");
    let commits: Vec<String> = oids.values().map(|oid| format!("{}", oid)).collect();
    merger.fetch_notes(utils::as_str_slice(&commits).as_slice())?;

    for (reference, oid) in oids {
        let (should_merge, note) = merger.should_merge(oid, target_oid);
        info!("Merging {} ({} into {})?: {}",
              reference,
              oid,
              target_oid,
              should_merge);
        debug!("Note found: {:?}", note);
        if !should_merge {
            info!("Merge commit is up to date");
            continue;
        }

        info!("Performing merge");
        let merged_note = merger.merge(oid, target_oid, &reference, target_ref)?;

        info!("Adding note: {:?}", merged_note);
        merger.add_note(&merged_note, oid)?;
    }
    info!("Pushing to remote");
    merger.push()?;

    remote.disconnect();
    Ok(())
}

fn to_option_str(opt: &Option<String>) -> Option<&str> {
    opt.as_ref().map(|s| &**s)
}

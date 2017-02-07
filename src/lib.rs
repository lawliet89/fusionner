extern crate env_logger;
extern crate git2;
extern crate libgit2_sys as git2_raw;
#[macro_use]
extern crate log;
extern crate regex;
extern crate rustc_serialize;
extern crate toml;

#[macro_use]
mod utils;
pub mod merger;
pub mod git;

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::marker::PhantomData;
use std::vec::Vec;

use regex::RegexSet;
use rustc_serialize::{Decodable, Encodable};

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct Config<'config> {
    pub repository: RepositoryConfiguration<'config>,
    pub interval: Option<u64>,
}

#[derive(RustcDecodable, RustcEncodable, Eq, PartialEq, Clone, Debug)]
pub struct RepositoryConfiguration<'config> {
    pub uri: String,
    pub checkout_path: String,
    pub remote: Option<String>,
    pub notes_namespace: Option<String>,
    pub fetch_refspecs: Vec<String>,
    pub push_refspecs: Vec<String>,
    // Authentication Options
    pub username: Option<String>,
    pub password: Option<String>,
    pub key: Option<String>,
    pub key_passphrase: Option<String>,
    // Matching settings
    pub merge_ref: Option<String>,
    pub target_ref: Option<String>, // TODO: Support specifying branch name instead of references
    pub _marker: PhantomData<&'config String>,
    // Signature settings
    pub signature_name: Option<String>,
    pub signature_email: Option<String>,
}

/// "Compiled" watch reference
#[derive(Debug)]
pub struct WatchReferences {
    regex_set: RegexSet,
    exact_list: Vec<String>,
}

impl<'config> Config<'config> {
    pub fn read_config(path: &str) -> Result<Config, String> {
        info!("Reading configuration from '{}'", path);
        let mut file = File::open(&path).map_err(|e| format!("{:?}", e))?;
        let mut config_toml = String::new();
        file.read_to_string(&mut config_toml).map_err(|e| format!("{:?}", e))?;

        deserialize_toml(&config_toml)
    }
}

impl<'config> RepositoryConfiguration<'config> {
    pub fn resolve_target_ref(&self, remote: &mut git::Remote) -> Result<String, git2::Error> {
        match self.target_ref {
            Some(ref reference) => {
                info!("Target Reference Specified: {}", reference);
                let remote_refs = remote.remote_ls()?;
                if let None = remote_refs.iter().find(|head| &head.name == reference) {
                    return Err(git_err!(&format!("Could not find {} on remote", reference)));
                }
                Ok(reference.to_string())
            }
            None => {
                let head = remote.head()?;
                if let None = head {
                    return Err(git_err!("Could not find a default HEAD on remote"));
                }
                let head = head.unwrap();
                info!("Target Reference set to remote HEAD: {}", head);
                Ok(head)
            }
        }
    }
}

impl WatchReferences {
    pub fn new<T: AsRef<str>>(exacts: &[T], regexes: &[T]) -> Result<WatchReferences, regex::Error>
        where T: std::fmt::Display
    {
        let exact_list = exacts.iter().map(|s| s.to_string()).collect();
        let regex_set = RegexSet::new(regexes)?;

        Ok(WatchReferences {
            regex_set: regex_set,
            exact_list: exact_list,
        })
    }

    /// Given a set of Remote heads as advertised by the remote, return a set of remtoe heads
    /// which exist based on the watch references
    pub fn resolve_watch_refs(&self, remote_ls: &Vec<git::RemoteHead>) -> HashSet<String> {
        let mut refs = HashSet::new();

        // Flatten and resolve symbolic targets
        let remote_ls: Vec<String> = remote_ls.iter().map(|r| r.flatten_clone()).collect();

        for exact_match in self.exact_list.iter().filter(|s| remote_ls.contains(s)) {
            refs.insert(exact_match.to_string());
        }

        for regex_match in remote_ls.iter().filter(|s| self.regex_set.is_match(s)) {
            refs.insert(regex_match.to_string());
        }

        refs
    }
}

fn deserialize_toml<T>(toml: &str) -> Result<T, String>
    where T: Decodable
{
    let parsed_toml = toml::Parser::new(&toml).parse();
    if let None = parsed_toml {
        return Err("Error parsing TOML".to_string());
    }

    let table = toml::Value::Table(parsed_toml.unwrap());
    Decodable::decode(&mut toml::Decoder::new(table)).map_err(|e| format!("{:?}", e))
}

fn serialize_toml<T>(obj: &T) -> Result<String, String>
    where T: Encodable
{
    let mut encoder = toml::Encoder::new();
    obj.encode(&mut encoder).map_err(|e| format!("{:?}", e))?;
    Ok(toml::Value::Table(encoder.toml).to_string())
}

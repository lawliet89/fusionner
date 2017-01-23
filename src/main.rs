extern crate docopt;
extern crate env_logger;
extern crate git2;
#[macro_use]
extern crate log;
extern crate rustc_serialize;
extern crate toml;

mod git;

use std::env;
use std::error::Error;
use std::fs::File;
use std::io::Read;

use docopt::Docopt;

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

    match process(&config) {
        Ok(result) => {
            println!("{}", result);
            std::process::exit(0);
        }
        Err(err) => {
            println!("Error: {}", err);
            std::process::exit(1);
        }
    };

}

fn process(config: &Config) -> Result<String, String> {
    let repo = try!(git::clone_or_open(&config.repository).map_err(|e| format!("{:?}", e)));
    Ok("Done".to_string())
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

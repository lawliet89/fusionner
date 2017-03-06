//! This is a utilities module. This is for utilitiy methods shared between the library and binary
use std::vec::Vec;
use rustc_serialize::{Decodable, Encodable};
use toml;

macro_rules! git_err {
    ($x:expr) => {
        git2::Error::from_str($x)
    }
}

pub fn as_str_slice(input: &[String]) -> Vec<&str> {
    input.iter().map(AsRef::as_ref).collect()
}

pub fn deserialize_toml<T>(toml: &str) -> Result<T, String>
    where T: Decodable
{
    let parsed_toml = toml::Parser::new(toml).parse();
    if parsed_toml.is_none() {
        return Err("Error parsing TOML".to_string());
    }

    let table = toml::Value::Table(parsed_toml.unwrap());
    Decodable::decode(&mut toml::Decoder::new(table)).map_err(|e| format!("{:?}", e))
}

#[allow(dead_code)]
pub fn serialize_toml<T>(obj: &T) -> Result<String, String>
    where T: Encodable
{
    let mut encoder = toml::Encoder::new();
    obj.encode(&mut encoder).map_err(|e| format!("{:?}", e))?;
    Ok(toml::Value::Table(encoder.toml).to_string())
}

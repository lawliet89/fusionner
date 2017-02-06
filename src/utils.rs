use std::vec::Vec;

use rustc_serialize::Decodable;
use super::toml;

pub fn to_option_str(opt: &Option<String>) -> Option<&str> {
    opt.as_ref().map(|s| &**s)
}

pub fn deserialize_toml<T>(toml: &str) -> Result<T, String>
    where T: Decodable
{
    let parsed_toml = toml::Parser::new(&toml).parse();
    if let None = parsed_toml {
        return Err("Error parsing TOML".to_string());
    }

    let table = toml::Value::Table(parsed_toml.unwrap());
    Decodable::decode(&mut toml::Decoder::new(table)).map_err(|e| format!("{:?}", e))?
}

pub fn as_str_slice<'a>(input: &'a [String]) -> Vec<&'a str> {
    input.iter().map(AsRef::as_ref).collect()
}

macro_rules! enum_equals(
    ($enum_a:expr, $enum_b:pat) => (
        match $enum_a {
            $enum_b => true,
            _ => false
        }
    )
);

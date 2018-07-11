//! This is a utilities module. This is for utilitiy methods shared between the library and binary
use std::vec::Vec;

use serde::de::DeserializeOwned;
use serde::Serialize;
use toml;

macro_rules! git_err {
    ($x:expr) => {
        git2::Error::from_str($x)
    };
}

pub fn as_str_slice(input: &[String]) -> Vec<&str> {
    input.iter().map(AsRef::as_ref).collect()
}

pub fn deserialize_toml<T>(toml: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    toml::from_str(toml).map_err(|e| e.to_string())
}

#[allow(dead_code)]
pub fn serialize_toml<T>(obj: &T) -> Result<String, String>
where
    T: Serialize,
{
    toml::to_string(obj).map_err(|e| e.to_string())
}

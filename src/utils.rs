use std::vec::Vec;

use rustc_serialize::{Decodable, Encodable};
use super::{git2, git2_raw, toml};

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
    Decodable::decode(&mut toml::Decoder::new(table)).map_err(|e| format!("{:?}", e))
}

pub fn serialize_toml<T>(obj: &T) -> Result<String, String>
    where T: Encodable
{
    let mut encoder = toml::Encoder::new();
    obj.encode(&mut encoder).map_err(|e| format!("{:?}", e))?;
    Ok(toml::Value::Table(encoder.toml).to_string())
}

pub fn as_str_slice<'a>(input: &'a [String]) -> Vec<&'a str> {
    input.iter().map(AsRef::as_ref).collect()
}

/// Gets the stage number from a Git index entry
/// The meaning of the fields corresponds to core Git's documentation (in "Documentation/technical/index-format.txt").
pub fn git_index_entry_stage(entry: &git2::IndexEntry) -> u16 {
    (entry.flags & git2_raw::GIT_IDXENTRY_STAGEMASK) >> git2_raw::GIT_IDXENTRY_STAGESHIFT
}

/// From the stage number of a Git Index entry, determine if it's in conflict
/// https://libgit2.github.com/libgit2/#HEAD/group/index/git_index_entry_is_conflict
pub fn git_index_entry_is_conflict(entry: &git2::IndexEntry) -> bool {
    git_index_entry_stage(entry) > 0
}

pub fn index_in_conflict(entries: &mut git2::IndexEntries) -> bool {
    entries.any(|ref entry| git_index_entry_is_conflict(entry))
}

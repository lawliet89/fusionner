//! This is a utilities module. This is for utilitiy methods shared between the library and binary
use std::vec::Vec;

macro_rules! git_err {
    ($x:expr) => {
        git2::Error::from_str($x)
    }
}

pub fn as_str_slice<'a>(input: &'a [String]) -> Vec<&'a str> {
    input.iter().map(AsRef::as_ref).collect()
}

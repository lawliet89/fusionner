pub fn to_option_str(opt: &Option<String>) -> Option<&str> {
    opt.as_ref().map(|s| &**s)
}

macro_rules! enum_equals(
    ($enum_a:expr, $enum_b:pat) => (
        match $enum_a {
            $enum_b => true,
            _ => false
        }
    )
);

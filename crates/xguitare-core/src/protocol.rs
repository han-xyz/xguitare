use std::fmt;

use serde::{Deserialize, Serialize};

/// Which starter Bank the first run seeds into a missing `bank_dir`; without a `bank_dir` in the config file, `[banks]` in `state.toml` can name each kind's directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BankKind {
    #[default]
    Guitar,
    Bass,
}

impl fmt::Display for BankKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BankKind::Guitar => "guitar",
            BankKind::Bass => "bass",
        })
    }
}

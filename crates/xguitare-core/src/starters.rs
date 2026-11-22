use xguitare_protocol::BankKind;

use crate::firstrun::{StarterIr, starter_irs, starter_presets};
use crate::{BankEntry, Preset};

/// The starter Presets and Cabinet IRs of a kind that a Bank lacks, each list in starter order.
pub(crate) struct Missing {
    /// (name, TOML text) of each starter Preset no valid Preset in the Bank names.
    pub(crate) presets: Vec<(String, &'static str)>,
    pub(crate) irs: Vec<StarterIr>,
}

/// The starters of `kind` missing from `entries`: a Preset when no valid entry has its name, compared case-insensitively; a Cabinet IR when `resolves` says its name finds no file.
pub(crate) fn missing_starters(
    entries: &[BankEntry],
    kind: BankKind,
    resolves: impl Fn(&str) -> bool,
) -> Missing {
    let is_named = |wanted: &str| {
        entries.iter().any(
            |entry| matches!(entry, BankEntry::Valid { preset, .. } if preset.name.to_lowercase() == wanted),
        )
    };
    let presets = starter_presets(kind)
        .iter()
        .map(|(_, text)| {
            let name = Preset::parse(text).expect("a starter Preset parses").name;
            (name, *text)
        })
        .filter(|(name, _)| !is_named(&name.to_lowercase()))
        .collect();
    let irs = starter_irs(kind)
        .iter()
        .filter(|(file, _)| !resolves(file))
        .copied()
        .collect();
    Missing { presets, irs }
}

use xguitare_dsp::modules::{
    cabinet, chorus, compressor, delay, drive, flanger, fuzz, gain, gate, preamp, reverb, tonestack,
};
use xguitare_dsp::{
    Chorus, Compressor, Delay, Drive, Flanger, Fuzz, Gain, Gate, Module, ModuleSpec, Preamp,
    Reverb, Tonestack,
};

struct Entry {
    spec: &'static ModuleSpec,
    construct: Option<fn() -> Box<dyn Module>>,
}

static ENTRIES: &[Entry] = &[
    Entry {
        spec: &gain::SPEC,
        construct: Some(|| Box::new(Gain::new())),
    },
    Entry {
        spec: &gate::SPEC,
        construct: Some(|| Box::new(Gate::new())),
    },
    Entry {
        spec: &compressor::SPEC,
        construct: Some(|| Box::new(Compressor::new())),
    },
    Entry {
        spec: &drive::SPEC,
        construct: Some(|| Box::new(Drive::new())),
    },
    Entry {
        spec: &fuzz::SPEC,
        construct: Some(|| Box::new(Fuzz::new())),
    },
    Entry {
        spec: &preamp::SPEC,
        construct: Some(|| Box::new(Preamp::new())),
    },
    Entry {
        spec: &tonestack::SPEC,
        construct: Some(|| Box::new(Tonestack::new())),
    },
    Entry {
        spec: &cabinet::SPEC,
        construct: None,
    },
    Entry {
        spec: &delay::SPEC,
        construct: Some(|| Box::new(Delay::new())),
    },
    Entry {
        spec: &chorus::SPEC,
        construct: Some(|| Box::new(Chorus::new())),
    },
    Entry {
        spec: &flanger::SPEC,
        construct: Some(|| Box::new(Flanger::new())),
    },
    Entry {
        spec: &reverb::SPEC,
        construct: Some(|| Box::new(Reverb::new())),
    },
];

pub struct ModuleRegistry {
    entries: &'static [Entry],
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        ModuleRegistry { entries: ENTRIES }
    }
}

impl ModuleRegistry {
    pub fn spec(&self, type_name: &str) -> Option<&'static ModuleSpec> {
        self.entry(type_name).map(|entry| entry.spec)
    }

    /// Every registered ModuleSpec, in registry order.
    pub fn specs(&self) -> impl Iterator<Item = &'static ModuleSpec> + '_ {
        self.entries.iter().map(|entry| entry.spec)
    }

    pub fn construct(&self, type_name: &str) -> Option<Box<dyn Module>> {
        self.entry(type_name)?
            .construct
            .map(|construct| construct())
    }

    fn entry(&self, type_name: &str) -> Option<&'static Entry> {
        self.entries
            .iter()
            .find(|entry| entry.spec.type_name == type_name)
    }
}

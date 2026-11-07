use xguitare_dsp::modules::{
    cabinet, chorus, compressor, delay, drive, flanger, fuzz, gain, gate, preamp, reverb, tonestack,
};
use xguitare_dsp::{
    Chorus, Compressor, Delay, Drive, Flanger, Fuzz, Gain, Gate, Module, ModuleSpec, Preamp,
    Reverb, Tonestack,
};

struct Entry(&'static ModuleSpec, Option<fn() -> Box<dyn Module>>);

static ENTRIES: &[Entry] = &[
    Entry(&gain::SPEC, Some(|| Box::new(Gain::new()))),
    Entry(&gate::SPEC, Some(|| Box::new(Gate::new()))),
    Entry(&compressor::SPEC, Some(|| Box::new(Compressor::new()))),
    Entry(&drive::SPEC, Some(|| Box::new(Drive::new()))),
    Entry(&fuzz::SPEC, Some(|| Box::new(Fuzz::new()))),
    Entry(&preamp::SPEC, Some(|| Box::new(Preamp::new()))),
    Entry(&tonestack::SPEC, Some(|| Box::new(Tonestack::new()))),
    Entry(&cabinet::SPEC, None),
    Entry(&delay::SPEC, Some(|| Box::new(Delay::new()))),
    Entry(&chorus::SPEC, Some(|| Box::new(Chorus::new()))),
    Entry(&flanger::SPEC, Some(|| Box::new(Flanger::new()))),
    Entry(&reverb::SPEC, Some(|| Box::new(Reverb::new()))),
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
        self.entry(type_name).map(|Entry(spec, _)| *spec)
    }

    /// Every registered ModuleSpec, in registry order.
    pub fn specs(&self) -> impl Iterator<Item = &'static ModuleSpec> + '_ {
        self.entries.iter().map(|Entry(spec, _)| *spec)
    }

    pub fn construct(&self, type_name: &str) -> Option<Box<dyn Module>> {
        self.entry(type_name)?.1.map(|construct| construct())
    }

    fn entry(&self, type_name: &str) -> Option<&'static Entry> {
        self.entries
            .iter()
            .find(|Entry(spec, _)| spec.type_name == type_name)
    }
}

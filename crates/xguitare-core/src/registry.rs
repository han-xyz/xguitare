use xguitare_dsp::{Channels, Drive, Gain, Gate, Module, ModuleSpec, Preamp, Tonestack};

// Placeholder: parse and the layout check know the cabinet; task 08 replaces it with the dsp spec and a constructor.
static CABINET: ModuleSpec = ModuleSpec {
    type_name: "cabinet",
    channels: Channels::MonoToStereo,
    params: &[],
};

struct Entry {
    spec: &'static ModuleSpec,
    construct: Option<fn() -> Box<dyn Module>>,
}

pub struct ModuleRegistry {
    entries: Vec<Entry>,
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        ModuleRegistry {
            entries: vec![
                Entry {
                    spec: Gain::new().spec(),
                    construct: Some(|| Box::new(Gain::new())),
                },
                Entry {
                    spec: Gate::new().spec(),
                    construct: Some(|| Box::new(Gate::new())),
                },
                Entry {
                    spec: Drive::new().spec(),
                    construct: Some(|| Box::new(Drive::new())),
                },
                Entry {
                    spec: Preamp::new().spec(),
                    construct: Some(|| Box::new(Preamp::new())),
                },
                Entry {
                    spec: Tonestack::new().spec(),
                    construct: Some(|| Box::new(Tonestack::new())),
                },
                Entry {
                    spec: &CABINET,
                    construct: None,
                },
            ],
        }
    }
}

impl ModuleRegistry {
    pub fn spec(&self, type_name: &str) -> Option<&'static ModuleSpec> {
        self.entry(type_name).map(|entry| entry.spec)
    }

    pub fn construct(&self, type_name: &str) -> Option<Box<dyn Module>> {
        self.entry(type_name)?
            .construct
            .map(|construct| construct())
    }

    fn entry(&self, type_name: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|entry| entry.spec.type_name == type_name)
    }
}

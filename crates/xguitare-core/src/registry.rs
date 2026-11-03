use xguitare_dsp::modules::cabinet::Cabinet;
use xguitare_dsp::{Gain, Module, ModuleSpec};

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
                    spec: Cabinet::new(None).spec(),
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

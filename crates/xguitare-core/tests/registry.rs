use xguitare_core::ModuleRegistry;
use xguitare_dsp::modules::cabinet::Cabinet;
use xguitare_dsp::{Module, Unit};

#[test]
fn specs_equal_constructed_module_specs() {
    let registry = ModuleRegistry::default();
    let mut types = Vec::new();
    for spec in registry.specs() {
        let built = match registry.construct(spec.type_name) {
            Some(module) => module.spec(),
            None => Cabinet::new(None).spec(),
        };
        assert!(std::ptr::eq(spec, built), "{}", spec.type_name);
        assert!(std::ptr::eq(registry.spec(spec.type_name).unwrap(), spec));
        types.push(spec.type_name);
    }
    assert_eq!(
        types,
        [
            "gain",
            "gate",
            "compressor",
            "autowah",
            "drive",
            "fuzz",
            "preamp",
            "tonestack",
            "eq",
            "cabinet",
            "delay",
            "chorus",
            "flanger",
            "reverb"
        ]
    );
}

#[test]
fn labelled_params_count_their_labels_from_zero() {
    let registry = ModuleRegistry::default();
    let labelled = registry
        .specs()
        .flat_map(|spec| spec.params)
        .filter(|param| !param.labels.is_empty());
    for param in labelled {
        assert!(param.integer, "{} is not integer", param.name);
        assert_eq!(param.min, 0.0, "{} min", param.name);
        assert_eq!(
            param.max,
            (param.labels.len() - 1) as f32,
            "{} max",
            param.name
        );
        assert_eq!(param.unit, Unit::Linear, "{} unit", param.name);
        assert!(
            param.default.fract() == 0.0 && param.default <= param.max,
            "{} default",
            param.name
        );
    }
}

use xguitare_core::ModuleRegistry;
use xguitare_dsp::Module;
use xguitare_dsp::modules::cabinet::Cabinet;

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
            "drive",
            "fuzz",
            "preamp",
            "tonestack",
            "cabinet",
            "delay",
            "chorus",
            "flanger",
            "reverb"
        ]
    );
}

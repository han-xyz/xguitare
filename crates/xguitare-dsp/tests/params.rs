use xguitare_dsp::modules::cabinet::Cabinet;
use xguitare_dsp::{
    Autowah, Chorus, Compressor, Delay, Drive, Flanger, Fuzz, Gain, Gate, Module, Preamp, Reverb,
    Tonestack, Unit,
};

#[test]
fn integer_params_are_stages_and_pingpong() {
    let modules: Vec<Box<dyn Module>> = vec![
        Box::new(Gate::new()),
        Box::new(Drive::new()),
        Box::new(Gain::new()),
        Box::new(Preamp::new()),
        Box::new(Tonestack::new()),
        Box::new(Cabinet::new(None)),
        Box::new(Delay::new()),
        Box::new(Reverb::new()),
    ];
    let integer: Vec<String> = modules
        .iter()
        .flat_map(|module| {
            let spec = module.spec();
            spec.params
                .iter()
                .filter(|param| param.integer)
                .map(|param| format!("{}.{}", spec.type_name, param.name))
        })
        .collect();
    assert_eq!(
        integer,
        [
            "preamp.stages",
            "cabinet.low_cut",
            "cabinet.high_cut",
            "delay.pingpong"
        ]
    );
}

#[test]
fn labelled_params_count_their_labels_from_zero() {
    let modules: Vec<Box<dyn Module>> = vec![
        Box::new(Gate::new()),
        Box::new(Compressor::new()),
        Box::new(Drive::new()),
        Box::new(Fuzz::new()),
        Box::new(Gain::new()),
        Box::new(Preamp::new()),
        Box::new(Tonestack::new()),
        Box::new(Cabinet::new(None)),
        Box::new(Delay::new()),
        Box::new(Chorus::new()),
        Box::new(Flanger::new()),
        Box::new(Reverb::new()),
        Box::new(Autowah::new()),
    ];
    let labelled = modules
        .iter()
        .flat_map(|module| module.spec().params)
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

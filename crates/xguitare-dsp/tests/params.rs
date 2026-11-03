use xguitare_dsp::modules::cabinet::Cabinet;
use xguitare_dsp::{Delay, Drive, Gain, Gate, Module, Preamp, Reverb, Tonestack};

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
    assert_eq!(integer, ["preamp.stages", "delay.pingpong"]);
}

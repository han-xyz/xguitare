const README: &str = include_str!("../../../README.md");
const CRUNCH: &str = include_str!("../bank/02-crunch.toml");

fn readme_crunch_sample() -> &'static str {
    let intro = README
        .find("The starter Preset `02-crunch.toml`:")
        .expect("README names the 02-crunch.toml sample");
    let after_intro = &README[intro..];
    let open = after_intro.find("```toml\n").expect("a toml block follows") + "```toml\n".len();
    let block = &after_intro[open..];
    let close = block.find("```").expect("the toml block is closed");
    &block[..close]
}

#[test]
fn readme_crunch_sample_equals_bank_file() {
    assert_eq!(readme_crunch_sample(), CRUNCH);
}

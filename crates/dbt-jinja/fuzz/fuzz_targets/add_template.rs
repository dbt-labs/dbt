#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &str| {
    minijinja::Environment::new()
        .add_template("fuzz.txt", input)
        .ok();
});

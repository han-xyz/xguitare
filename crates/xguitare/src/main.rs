#[cfg(feature = "rt-check")]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn main() {
    std::process::exit(xguitare::run_main(std::env::args_os()));
}

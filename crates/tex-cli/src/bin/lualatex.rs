// Keep this executable tiny: it relays to the one installed PDF engine and
// tells that engine which public personality was invoked.
#[path = "launcher.rs"]
mod driver;

fn main() -> std::process::ExitCode {
    driver::main()
}

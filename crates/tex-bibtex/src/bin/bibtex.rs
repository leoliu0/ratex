//! `bibtex` command-line entry point.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(tex_bibtex::run(&args, env!("CARGO_PKG_VERSION")));
}

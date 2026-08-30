//! tex-bibtex — BibTeX engine binary.

#[path = "../bibtex/mod.rs"]
mod bibtex;

const VERSION: &str = "1.0";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(bibtex::run(&args, VERSION));
}

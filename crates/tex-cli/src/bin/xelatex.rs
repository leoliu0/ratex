// The compatibility executable intentionally shares the PDF engine driver;
// it selects its public identity from argv[0]. Keep a distinct Cargo source
// path so Cargo does not diagnose one file as three separate targets.
#[path = "pdflatex.rs"]
mod driver;

fn main() {
    driver::main();
}

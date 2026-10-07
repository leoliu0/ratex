//! `makeindex` command-line entry point.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(tex_makeindex::run_cli(&args, &tex_makeindex::FsHost));
}

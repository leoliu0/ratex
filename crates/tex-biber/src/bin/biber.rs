fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    std::process::exit(tex_biber::cli_main(&args));
}

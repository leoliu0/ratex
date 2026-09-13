fn main() {
    let mut args = std::env::args_os().skip(1);
    let Some(output) = args.next() else {
        eprintln!("Usage: tex-index OUTPUT_DIRECTORY TEXMF_ROOT...");
        std::process::exit(2);
    };
    let mut any = false;
    for root in args {
        any = true;
        match tex_kpse::build_filename_index(
            std::path::Path::new(&root),
            std::path::Path::new(&output),
        ) {
            Ok(path) => println!("{}", path.display()),
            Err(error) => {
                eprintln!("{}: {error}", std::path::Path::new(&root).display());
                std::process::exit(1);
            }
        }
    }
    if !any {
        eprintln!("Specify at least one TEXMF root");
        std::process::exit(2);
    }
}

use std::path::Path;
use std::process::ExitCode;
use tex_bibtex::RunOpts;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // usage: bibtex [-terse] [-min-crossrefs=N] <auxfile>
    let mut aux_arg: Option<String> = None;
    let mut opts = RunOpts::default();
    for a in &args[1..] {
        if a == "-terse" || a == "--terse" {
            opts.terse = true;
        } else if let Some(n) = a
            .strip_prefix("-min-crossrefs=")
            .or_else(|| a.strip_prefix("--min-crossrefs="))
        {
            match n.parse::<usize>() {
                Ok(v) => opts.min_crossrefs = v,
                Err(_) => {
                    eprintln!("bibtex: bad -min-crossrefs value `{n}`");
                    return ExitCode::from(2);
                }
            }
        } else if a.starts_with('-') {
            // other real-bibtex flags (-8bit, -exchange-limit=N, ...) accepted
            continue;
        } else if aux_arg.is_none() {
            aux_arg = Some(a.clone());
        }
    }
    let Some(arg) = aux_arg else {
        eprintln!("Usage: bibtex [-terse] [-min-crossrefs=N] <auxfile>");
        return ExitCode::from(2);
    };
    let jobname = arg.strip_suffix(".aux").unwrap_or(&arg).to_string();
    let cwd = Path::new(".");
    let outcome = tex_bibtex::run_opts(&jobname, cwd, true, &opts);
    print!("{}", outcome.stdout);
    ExitCode::from(outcome.status.clamp(0, 255) as u8)
}

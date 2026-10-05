use std::path::{Path,PathBuf};
use tex_biber::{Options,run};
use tex_kpse::fs;

fn main(){if let Err(e)=execute(){eprintln!("ERROR - {e}");std::process::exit(2)}}
fn execute()->Result<(),String>{
    let mut input=PathBuf::from(".");let mut output=None;let mut quiet=false;let mut job=None;
    let mut args=std::env::args().skip(1);
    while let Some(arg)=args.next(){match arg.as_str(){
        "-q"|"--quiet"=>quiet=true,
        "--output-directory"=>output=Some(PathBuf::from(args.next().ok_or("Missing --output-directory value")?)),
        "--input-directory"=>input=PathBuf::from(args.next().ok_or("Missing --input-directory value")?),
        "--version"|"-V"=>{println!("TeXres Biber {} (BCF 3.11, BBL 3.3)",env!("CARGO_PKG_VERSION"));return Ok(())},
        "--help"|"-h"=>{println!("Usage: biber [--input-directory DIR] [--output-directory DIR] [-q|--quiet] <job[.bcf]>");return Ok(())},
        _ if arg.starts_with("--output-directory=")=>output=Some(PathBuf::from(&arg[19..])),
        _ if arg.starts_with("--input-directory=")=>input=PathBuf::from(&arg[18..]),
        _ if arg.starts_with('-')=>return Err(format!("Unknown option {arg}")),
        _=>{if job.replace(PathBuf::from(arg)).is_some(){return Err("Only one job can be processed at a time".into())}},
    }}
    let mut bcf=job.ok_or("Missing job name (use --help for usage)")?;if bcf.extension().is_none(){bcf.set_extension("bcf");}if !bcf.is_absolute(){bcf=input.join(bcf)}
    let out=output.map(|dir|dir.join(bcf.file_name().unwrap()).with_extension("bbl"));
    if let Some(parent)=out.as_ref().and_then(|p|p.parent()){fs::create_dir_all(parent).map_err(|e|e.to_string())?;}
    let find=|name:&str|find_file(name,&input,bcf.parent().unwrap_or(Path::new(".")));
    let result=run(&Options {bcf:bcf.clone(),output:out,find_file:&find})?;
    if !quiet {print!("{}",result.log)}
    Ok(())
}
fn find_file(name:&str,input:&Path,bcfdir:&Path)->Option<PathBuf>{
    let path=PathBuf::from(name);let readable=|p:&Path|fs::metadata(p).is_ok_and(|m|m.is_file());
    for p in [path.clone(),input.join(&path),bcfdir.join(&path)] {if readable(&p){return Some(p)}}
    if let Some(var)=std::env::var_os("BIBINPUTS"){for root in std::env::split_paths(&var){if root.as_os_str().is_empty(){continue}let recursive=root.to_string_lossy().ends_with("//");let root=if recursive{PathBuf::from(root.to_string_lossy().trim_end_matches('/'))}else{root};let candidate=root.join(&path);if readable(&candidate){return Some(candidate)}if recursive {if let Some(found)=recursive_find(&root,&path){return Some(found)}}}}
    None
}
fn recursive_find(root:&Path,name:&Path)->Option<PathBuf>{
    let mut pending=vec![root.to_path_buf()];while let Some(dir)=pending.pop(){let candidate=dir.join(name);if fs::metadata(&candidate).is_ok_and(|m|m.is_file()){return Some(candidate)}if let Ok(files)=fs::read_dir(dir){for item in files.flatten(){if item.file_type().is_ok_and(|t|t.is_dir()&&!t.is_symlink()){pending.push(item.path());}}}}
    None
}

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize,Ordering};
use tex_biber::{Options,run_configured};
static NEXT:AtomicUsize=AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {fn new()->Self {let path=std::env::temp_dir().join(format!("tex-biber-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));std::fs::create_dir_all(&path).unwrap();Self(path)}}
impl Drop for Temp {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
#[test]
fn committed_biber_222_corpus(){
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fixtures/biber");
    let mut cases=std::fs::read_dir(root).unwrap().map(|e|e.unwrap().path()).filter(|p|p.is_dir()&&p.join("main.bcf").is_file()).collect::<Vec<_>>();cases.sort();
    let temp=Temp::new();let mut failures=vec![];
    let converted=regex::Regex::new(r"^(BibTeX subsystem: ).*?\.utf8(, line \d+)").unwrap();
    for case in &cases {
        let options=std::fs::read_to_string(case.join("options.json")).map(|text|serde_json::from_str::<serde_json::Map<String,serde_json::Value>>(&text).unwrap()).unwrap_or_default();
        if options.get("output_file").and_then(|value|value.as_str())==Some("-")&&!options.get("quiet").and_then(|value|value.as_bool()).unwrap_or(false){continue;}
        let overrides=options.into_iter().map(|(key,value)|{let value=match value{serde_json::Value::String(value)=>value,_=>value.to_string()};(key,value)}).collect::<std::collections::BTreeMap<_,_>>();
        let output=temp.0.join(case.file_name().unwrap()).with_extension("bbl");
        let find=|name:&str|{let path=case.join(name);path.is_file().then_some(path)};
        let configuration=std::fs::read_to_string(case.join("biber.conf")).ok();
        match run_configured(&Options {bcf:case.join("main.bcf"),output:Some(output.clone()),output_directory:None,find_file:&find},&overrides,configuration.as_deref()) {
            Ok(outcome)=>{
                let expected=std::fs::read(case.join("expected.bbl")).unwrap();let actual=std::fs::read(output).unwrap();
                if expected!=actual {let offset=expected.iter().zip(&actual).position(|(a,b)|a!=b).unwrap_or(expected.len().min(actual.len()));failures.push(format!("{}: first differing byte {offset} (expected {}, actual {} bytes)",case.file_name().unwrap().to_string_lossy(),expected.len(),actual.len()));}
                if let Ok(expected)=std::fs::read_to_string(case.join("expected.warnings")) {
                    let mut actual=outcome.log.lines().filter_map(|line|line.strip_prefix("WARN - ")).map(|line|converted.replace(line,"${1}<converted datasource>${2}").into_owned()).collect::<Vec<_>>();actual.sort_unstable();
                    let mut expected=expected.lines().map(str::to_owned).collect::<Vec<_>>();expected.sort_unstable();
                    if expected!=actual {failures.push(format!("{}: warnings differ: expected {:?}, actual {:?}",case.file_name().unwrap().to_string_lossy(),expected,actual));}
                }
            },
            Err(e)=>failures.push(format!("{}: {e}",case.file_name().unwrap().to_string_lossy())),
        }
    }
    assert!(failures.is_empty(),"{} of {} oracle cases differ:\n{}",failures.len(),cases.len(),failures.join("\n"));
}

#[test]
fn committed_biber_222_tool_corpus(){
    let script=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/test_biber.py");
    let output=std::process::Command::new("python3").arg(script)
        .args(["--biber",env!("CARGO_BIN_EXE_biber"),"--committed-bcf","--case","tool-*"])
        .output().expect("The oracle fixture runner requires Python 3");
    assert!(output.status.success(),"tool-mode oracle fixtures differ:\n{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}

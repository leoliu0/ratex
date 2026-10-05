use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize,Ordering};
use tex_biber::{Options,run};
static NEXT:AtomicUsize=AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {fn new()->Self {let path=std::env::temp_dir().join(format!("tex-biber-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));std::fs::create_dir_all(&path).unwrap();Self(path)}}
impl Drop for Temp {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
#[test]
fn committed_biber_222_corpus(){
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fixtures/biber");
    let mut cases=std::fs::read_dir(root).unwrap().map(|e|e.unwrap().path()).filter(|p|p.is_dir()).collect::<Vec<_>>();cases.sort();
    assert!(cases.len()>=30,"oracle corpus must cover at least thirty cases");
    let temp=Temp::new();let mut failures=vec![];
    for case in &cases {
        let output=temp.0.join(case.file_name().unwrap()).with_extension("bbl");
        let find=|name:&str|{let path=case.join(name);path.is_file().then_some(path)};
        match run(&Options {bcf:case.join("main.bcf"),output:Some(output.clone()),find_file:&find}) {
            Ok(_)=>{let expected=std::fs::read(case.join("expected.bbl")).unwrap();let actual=std::fs::read(output).unwrap();if expected!=actual {let offset=expected.iter().zip(&actual).position(|(a,b)|a!=b).unwrap_or(expected.len().min(actual.len()));failures.push(format!("{}: first differing byte {offset} (expected {}, actual {} bytes)",case.file_name().unwrap().to_string_lossy(),expected.len(),actual.len()));}},
            Err(e)=>failures.push(format!("{}: {e}",case.file_name().unwrap().to_string_lossy())),
        }
    }
    assert!(failures.is_empty(),"{} of {} oracle cases differ:\n{}",failures.len(),cases.len(),failures.join("\n"));
}

use std::collections::BTreeMap;
use std::path::{Path,PathBuf};
use std::sync::atomic::{AtomicUsize,Ordering};
use tex_biber::{run_tool_configured,Options,Outcome};
static NEXT:AtomicUsize=AtomicUsize::new(0);
struct Work(PathBuf);
impl Work {
    fn fixture(name:&str)->Self {
        let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/errors").join(name);
        let path=std::env::temp_dir().join(format!("biber-maptool-errors-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        for file in std::fs::read_dir(root).unwrap(){let file=file.unwrap();if matches!(file.path().extension().and_then(|s|s.to_str()),Some("bib"|"bltxml"|"conf")){std::fs::copy(file.path(),path.join(file.file_name())).unwrap();}}
        Self(path)
    }
    fn run(&self,source:&str,overrides:&[(&str,&str)])->Result<Outcome,String>{
        let configuration=std::fs::read_to_string(self.0.join("biber.conf")).ok();
        let overrides=overrides.iter().map(|(k,v)|(k.to_string(),v.to_string())).collect::<BTreeMap<_,_>>();
        let find=|name:&str|{let path=self.0.join(name);path.is_file().then_some(path)};
        run_tool_configured(&Options {bcf:self.0.join(source),output:Some(self.0.join("converted.bib")),output_directory:None,find_file:&find},&overrides,configuration.as_deref())
    }
}
impl Drop for Work {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
#[test]
fn fatal_datamodel_preserves_output_and_returns_exit_two(){
    let work=Work::fixture("maptool-fatal-datamodel");
    let result=work.run("main.bib",&[("validate_datamodel","1"),("dieondatamodel","1")]).unwrap();
    assert_eq!(result.errors,1);
    assert_eq!(std::fs::read(&result.bbl).unwrap(),include_bytes!("fixtures/errors/maptool-fatal-datamodel/expected.oracle.bib"));
    assert!(result.log.lines().any(|line|line.starts_with("ERROR - Datamodel:")&&line.contains("title")&&line.contains("mandatory")));
    let output=work.0.join("cli.bib");
    let status=std::process::Command::new(env!("CARGO_BIN_EXE_biber")).current_dir(&work.0)
        .args(["--tool","--configfile=biber.conf","--validate-datamodel","--dieondatamodel","--output-file"])
        .arg(&output).arg("main.bib").output().unwrap();
    assert_eq!(status.status.code(),Some(2));
    assert_eq!(std::fs::read(output).unwrap(),include_bytes!("fixtures/errors/maptool-fatal-datamodel/expected.oracle.bib"));
}
#[test]
fn invalid_biblatexml_and_custom_datamodel_are_fatal_before_output(){
    for (fixture,options) in [
        ("maptool-invalid-xml",vec![("input_format","biblatexml"),("validate_bltxml","1")]),
        ("maptool-invalid-custom-rng",vec![("input_format","biblatexml"),("validate_bltxml","1"),("no_default_datamodel","1")]),
    ]{
        let work=Work::fixture(fixture);let error=work.run("main.bltxml",&options).unwrap_err();
        assert!(error.contains("failed to validate against schema"),"{error}");
        assert!(!work.0.join("converted.bib").exists());
    }
    let work=Work::fixture("maptool-invalid-xml");
    let output=std::process::Command::new(env!("CARGO_BIN_EXE_biber")).current_dir(&work.0)
        .args(["--tool","--input-format=biblatexml","--validate-bltxml","main.bltxml"]).output().unwrap();
    assert_eq!(output.status.code(),Some(2));
    assert!(!work.0.join("main_bibertool.bib").exists());
}
#[test]
fn executable_replacements_name_the_unsupported_construct(){
    // The pinned oracle evaluates these; this is the deliberate no-Perl-code
    // boundary, not a claim that the oracle rejects them.
    for (fixture,construct) in [
        ("maptool-code-array","'@{...}'"),
        ("maptool-code-scalar","scalar expression"),
        ("maptool-code-quotes","quote/operator expression"),
        ("maptool-code-subscript","computed subscript or dereference"),
    ]{
        let work=Work::fixture(fixture);let error=work.run("main.bib",&[]).unwrap_err();
        assert!(error.contains("key=a, step=1")&&error.contains(construct)&&error.contains("code evaluation is disabled"),"{error}");
        assert!(!Path::new(&work.0.join("converted.bib")).exists());
    }
}

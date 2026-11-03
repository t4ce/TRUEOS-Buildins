use hull_quickjs::{Engine,Limits};
use std::time::Duration;

#[test]
fn classic_module_cache_and_parser_admit_four_mib_and_reject_the_next_byte(){
    let mut e=Engine::with_limits(Limits{time_slice:Duration::from_secs(1),..Limits::default()}).unwrap();
    e.install_modules(|_,name|Ok(name.into())).unwrap();
    let mut source=Vec::with_capacity(4*1024*1024+1);
    source.extend_from_slice(b"/*");source.resize(4*1024*1024-2,b' ');source.extend_from_slice(b"*/");
    e.eval(&source,"classic.js",false).unwrap();
    assert!(e.module_dependencies(&source,"https://fixture/cap.js").unwrap().is_empty());
    e.add_module("https://fixture/cap.js",&source).unwrap();
    source.push(b' ');
    assert!(e.eval(&source,"classic.js",false).unwrap_err().contains("4 MiB"));
    assert!(e.module_dependencies(&source,"https://fixture/over.js").unwrap_err().contains("4 MiB"));
    assert!(e.add_module("https://fixture/over.js",&source).unwrap_err().contains("budget"));
}

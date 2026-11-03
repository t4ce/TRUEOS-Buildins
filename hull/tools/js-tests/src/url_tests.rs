//! Actual QuickJS globals backed by Hull's production URL router and Rust URL parser.
use crate::javascript_dom;
use hull_quickjs::Engine;
use std::{cell::RefCell, rc::Rc};
fn fixture() -> Engine {
    let mut e = Engine::new().unwrap();
    javascript_dom::install(&mut e, Rc::new(RefCell::new(rp5::parse("<p>before</p>")))).unwrap();
    e
}
fn run(e: &mut Engine, script: &str) {
    e.eval(script.as_bytes(), "url-fixture.js", false).unwrap();
}

#[test]
fn native_url_binding_matches_original_corejs_url_fallback() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/url-corejs.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert!(cases.len() > 100);
    let mut e = fixture();
    for (index, case) in cases.iter().enumerate() {
        let script = format!(
            r#"(()=>{{
        const c={case},args=JSON.parse(c.argsJSON);let actual;
        if(c.kind==='url'){{try{{const u=new URL(...args);for(const[k,v]of c.assign)u[k]=v;actual={{}};for(const k of ['href','origin','protocol','username','password','host','hostname','port','pathname','search','hash'])actual[k]=u[k];}}catch(e){{actual={{error:e.name}};}}}}
        else{{const p=new URLSearchParams(...args);for(const[method,...args]of c.operations)p[method](...args);actual={{serialized:p.toString(),entries:[...p],size:p.size}};}}
        function equal(a,b){{if(a===b)return true;if(!a||!b||typeof a!=='object'||typeof b!=='object')return false;const keys=Object.keys(a);return keys.length===Object.keys(b).length&&keys.every(k=>equal(a[k],b[k]));}}
        if(!equal(actual,c.expected))throw Error('case {index}: '+JSON.stringify(actual)+' expected '+JSON.stringify(c.expected));
    }})()"#
        );
        run(&mut e, &script);
    }
}
#[test]
fn url_parsing_resolution_setters_and_invalid_input() {
    let mut e = fixture();
    run(
        &mut e,
        r#"
    const u=new URL('../a b?x=1#top','https://User:pw@EXAMPLE.com:443/dir/page');
    if(u.href!=='https://User:pw@example.com/a%20b?x=1#top'||u.origin!=='https://example.com'||u.protocol!=='https:'||u.hostname!=='example.com'||u.port!==''||u.username!=='User'||u.password!=='pw')throw Error('parse/components');
    u.port='8443';u.pathname='new path';u.hash='tail';u.username='a b';
    if(u.host!=='example.com:8443'||u.pathname!=='/new%20path'||u.hash!=='#tail'||u.username!=='a%20b')throw Error('setters');
    const old=u.href;u.port='bad';if(u.href!==old)throw Error('invalid port should be ignored');
    try{u.href='not a URL';throw Error('invalid href accepted')}catch(e){if(!(e instanceof TypeError))throw e}
    if(u.href!==old)throw Error('invalid href mutated object');
    if(!URL.canParse('/x','https://example.org')||URL.canParse('/x')||URL.parse('/x')!==null||URL.parse('/x','https://example.org').href!=='https://example.org/x')throw Error('parse helpers');
    for(const value of ['http://[bad','https://x:99999','relative'])try{new URL(value);throw Error('invalid URL')}catch(e){if(!(e instanceof TypeError))throw e}
    if(new URL('https://münich.example/é').hostname!=='xn--mnich-kva.example'||new URL('http://[::1]:80').host!=='[::1]')throw Error('IDN/IPv6');
    if(new URL('data:text/plain,hi').origin!=='null'||new URL('file:///tmp/test').origin!=='null')throw Error('opaque/file origin');
    if(String(u)!==u.href||JSON.stringify(u)!==JSON.stringify(u.href)||Object.prototype.toString.call(u)!=='[object URL]')throw Error('stringifier');
    try{URL.canParse(Symbol());throw Error('symbol accepted')}catch(e){if(!(e instanceof TypeError))throw e}
    if(new URL('https://example.org/\ud800').pathname!=='/%EF%BF%BD')throw Error('USVString replacement');
    for(const make of [()=>URL(),()=>new URL(),()=>URL.canParse(),()=>URL.parse()])try{make();throw Error('missing new/argument')}catch(e){if(!(e instanceof TypeError))throw e}
"#,
    );
}
#[test]
fn search_params_are_same_object_and_live_in_both_directions() {
    let mut e = fixture();
    run(
        &mut e,
        r#"
    const u=new URL('https://example.org/?a=one%20two&b=~');const p=u.searchParams;
    if(p!==u.searchParams||p.get('a')!=='one two')throw Error('identity/parse');
    p.sort();if(u.search!=='?a=one+two&b=%7E')throw Error('form reserialization');
    p.append('a','next');if(p.getAll('a').join(',')!=='one two,next'||!u.href.endsWith('&a=next'))throw Error('append');
    const iterator=p.entries();if(iterator.next().value[0]!=='a')throw Error('iterator');
    u.search='x=1&y=2';if(p!==u.searchParams||p.get('x')!=='1'||p.has('a'))throw Error('search live update');
    if(iterator.next().value[0]!=='y')throw Error('iterator snapshot instead of live');
    u.href='https://other.test/?z=3';if(p.get('z')!=='3')throw Error('href live update');
    p.delete('z');if(u.href!=='https://other.test/'||p.size!==0)throw Error('query removal');
    p.set('x','');if(u.search!=='?x=')throw Error('set empty');
"#,
    );
}
#[test]
fn search_params_records_pairs_duplicates_iteration_and_coercion() {
    let mut e = fixture();
    run(
        &mut e,
        r#"
    const p=new URLSearchParams('?a=1&a=2&x=one+two&bad=%E0%A4%A');
    if(p.size!==4||p.get('a')!=='1'||p.getAll('a').join(',')!=='1,2'||p.get('x')!=='one two'||p.get('missing')!==null||p.get('bad')!=='�%A')throw Error('decode');
    p.delete('a','1');if(p.get('a')!=='2'||p.has('a','1')||!p.has('a','2'))throw Error('value filter');
    p.set('a','new');p.append('a','third');p.set('a','only');if(p.getAll('a').length!==1)throw Error('dedup set');
    const record=new URLSearchParams({q:'é a',n:null,u:undefined});if(String(record)!=='q=%C3%A9+a&n=null&u=undefined')throw Error('record');
    const pair=new URLSearchParams(new Map([['b','1'],['a','2']]));if([...pair].map(x=>x.join(':')).join(',')!=='b:1,a:2')throw Error('iterable');
    const seen=[];pair.forEach(function(v,k,list){if(this!==record||list!==pair)throw Error('callback context');seen.push(k+v);if(k==='b')pair.append('c','3')},record);
    if(seen.join(',')!=='b1,a2,c3')throw Error('live foreach');
    const sort=new URLSearchParams([['\ue000','x'],['😀','a'],['a','first'],['a','second']]);sort.sort();if([...sort.keys()].join(',')!=='a,a,😀,\ue000'||sort.getAll('a').join(',')!=='first,second')throw Error('UTF16 stable sort');
    if(String(new URLSearchParams(null))!=='null='||String(new URLSearchParams(true))!=='true='||String(new URLSearchParams([['\ud800','\udc00']]))!=='%EF%BF%BD=%EF%BF%BD')throw Error('coercion');
    const collision=new URLSearchParams({'\ud800x':'first','\ud801x':'last'});if(collision.size!==1||collision.get('�x')!=='last')throw Error('USV record key collisions');
    const iter=collision.entries();if(Object.prototype.toString.call(iter)!=='[object URLSearchParams Iterator]'||iter[Symbol.iterator]()!==iter)throw Error('iterator interface');
    iter.next();iter.next();collision.append('new','value');if(!iter.next().done)throw Error('exhausted iterator revived');
    for(const value of [[['a']],[['a','b','c']],['ab']])try{new URLSearchParams(value);throw Error('bad sequence accepted')}catch(e){if(!(e instanceof TypeError))throw e}
    try{URLSearchParams.prototype.get.call({},'x');throw Error('bad receiver')}catch(e){if(!(e instanceof TypeError))throw e}
    try{p.append('x');throw Error('missing value')}catch(e){if(!(e instanceof TypeError))throw e}
    Array.prototype.toJSON=()=>{throw Error('array transport hook')};Object.prototype.toJSON=()=>{throw Error('object transport hook')};
    if(new URL('https://x.test').host!=='x.test'||new URLSearchParams('a=1').toString()!=='a=1')throw Error('transport');
    delete Array.prototype.toJSON;delete Object.prototype.toJSON;
"#,
    );
}

#[test]
fn dynamic_modules_can_construct_urls_from_import_meta_and_publish_the_result() {
    let mut runtime = crate::resource_tests::runtime(
        "<p>before</p><script>import('/_app/entry/start.js').then(m=>document.querySelector('p').textContent=m.resolve())</script>",
        vec![(
            "https://fixture.test/_app/entry/start.js",
            "export function resolve(){const u=new URL('../chunks/app.js',import.meta.url);u.searchParams.set('ready','yes');return u.href}",
        )],
    );
    let (doc, reports) = crate::resource_tests::finish(&mut runtime, 1);
    assert_eq!(
        crate::resource_tests::text(&doc),
        "https://fixture.test/_app/chunks/app.js?ready=yes"
    );
    assert!(
        !reports.iter().any(|r| matches!(
            r,
            crate::javascript::Report::Evaluated { result: Err(_), .. }
                | crate::javascript::Report::JobError { .. }
        )),
        "{reports:?}"
    );
}
#[test]
fn standalone_url_realm_has_no_dom_or_io_and_observes_limits() {
    let mut e = Engine::new().unwrap();
    javascript_dom::install_url_only(&mut e).unwrap();
    run(
        &mut e,
        r#"
    if(new URL('/ok','https://example.test/').pathname!=='/ok'||new URLSearchParams('a=1').get('a')!=='1')throw Error('standalone');
    if(typeof document!=='undefined'||typeof fetch!=='undefined'||typeof require!=='undefined'||typeof __hullUrl!=='undefined')throw Error('capability leak');
    try{new URL('https://example.test/'+ 'a'.repeat(131073));throw Error('string limit')}catch(e){if(!(e instanceof RangeError))throw e}
    try{new URLSearchParams('a=1&'.repeat(4097));throw Error('pair limit')}catch(e){if(!(e instanceof RangeError))throw e}
"#,
    );
}

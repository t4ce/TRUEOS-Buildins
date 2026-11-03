use crate::javascript_dom;
use hull_quickjs::Engine;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
fn fixture(html: &str) -> (Engine, javascript_dom::SharedDocument) {
    let doc = Rc::new(RefCell::new(rp5::parse(html)));
    let mut engine = Engine::new().unwrap();
    javascript_dom::install_for_page(
        &mut engine,
        doc.clone(),
        None,
        Rc::new(Cell::new(None)),
        "https://fixture.test/path/page.html",
    )
    .unwrap();
    (engine, doc)
}
fn run(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "dom-mutation.js", false)
        .unwrap();
}
#[test]
fn creates_and_inserts_real_link_with_reflected_preload_properties() {
    let (mut engine, doc) = fixture("<p>ready</p>");
    run(
        &mut engine,
        r#"
        const links=document.getElementsByTagName('link'), link=document.createElement('LINK');
        if(!(link instanceof HTMLLinkElement)||!(link instanceof HTMLElement)||!(link instanceof Element)||link.localName!=='link'||link.tagName!=='LINK')throw Error('interfaces');
        if(link.isConnected||link.parentNode!==null||link.ownerDocument!==document||links.length)throw Error('detached identity');
        link.rel='modulepreload';link.as='script';link.crossOrigin='';link.href='../entry.js';
        if(link.rel!=='modulepreload'||link.as!=='script'||link.crossOrigin!=='anonymous'||link.href!=='https://fixture.test/entry.js')throw Error('reflected values');
        if(link.getAttribute('href')!=='../entry.js'||link.getAttribute('as')!=='script'||link.getAttribute('crossorigin')!=='')throw Error('real attributes');
        let loaded=0;link.addEventListener('load',()=>loaded++);
        if(document.head.appendChild(link)!==link||links[0]!==link||!link.isConnected||link.parentNode!==document.head)throw Error('insertion');
        if(loaded!==0)throw Error('insertion fabricated resource completion');
        link.setAttribute('href','/changed.js');if(link.href!=='https://fixture.test/changed.js')throw Error('live href');
        link.as='NOT-KNOWN';if(link.as!==''||link.getAttribute('as')!=='NOT-KNOWN')throw Error('as enum');
        link.crossOrigin=null;if(link.crossOrigin!==null||link.hasAttribute('crossorigin'))throw Error('nullable cors');
        link.crossOrigin='USE-CREDENTIALS';if(link.crossOrigin!=='use-credentials')throw Error('cors case');
        link.crossOrigin=undefined;if(link.crossOrigin!==null)throw Error('nullable undefined');
        link.href='/\ud800';if(link.href!=='https://fixture.test/%EF%BF%BD')throw Error('URL scalar conversion');
        link.href='/changed.js';
    "#,
    );
    let doc = doc.borrow();
    let id = doc
        .nodes
        .iter()
        .enumerate()
        .find_map(|(id, _)| (doc.name(id) == "link").then_some(id))
        .unwrap();
    assert_eq!(doc.name(doc.nodes[id].parent.unwrap()), "head");
    assert!(
        doc.element(id)
            .unwrap()
            .2
            .iter()
            .any(|a| a.name == "href" && a.value == "/changed.js")
    );
}
#[test]
fn document_base_uri_tracks_base_element_and_url_reflection() {
    let (mut engine, _) = fixture("<base href='/assets/'><p>ready</p>");
    run(
        &mut engine,
        r#"
        const link=document.createElement('link');link.href='app.css';
        if(document.URL!=='https://fixture.test/path/page.html'||document.documentURI!==document.URL||document.baseURI!=='https://fixture.test/assets/'||link.baseURI!==document.baseURI||link.href!=='https://fixture.test/assets/app.css')throw Error('base URL');
        document.querySelector('base').setAttribute('href','https://cdn.test/static/');
        if(link.href!=='https://cdn.test/static/app.css')throw Error('base mutation');
        link.removeAttribute('href');if(link.href!=='')throw Error('missing href');
        link.href='';if(link.href!=='https://cdn.test/static/')throw Error('empty href');
        document.querySelector('base').setAttribute('href','javascript:ignored');
        if(document.baseURI!==document.URL)throw Error('forbidden base scheme');
    "#,
    );
}
#[test]
fn insertion_moves_nodes_preserves_order_and_consumes_fragment_children() {
    let (mut engine, _) = fixture("<main><p id=old>old</p></main>");
    run(
        &mut engine,
        r#"
        const main=document.querySelector('main'),old=document.getElementById('old');
        const fragment=document.createDocumentFragment(),a=document.createElement('b'),b=document.createElement('i');
        a.appendChild(document.createTextNode('a'));b.appendChild(document.createTextNode('b'));
        fragment.appendChild(a);fragment.appendChild(b);
        if(!(fragment instanceof DocumentFragment)||fragment.querySelector('b')!==a||a.isConnected)throw Error('fragment');
        if(main.insertBefore(fragment,old)!==fragment||fragment.firstChild!==null||main.firstChild!==a||a.nextSibling!==b||b.nextSibling!==old||main.textContent!=='abold')throw Error('fragment insertion order');
        main.appendChild(a);if(main.lastChild!==a||a.previousSibling!==old||main.textContent!=='bolda')throw Error('move');
        main.insertBefore(a,a);if(main.lastChild!==a)throw Error('insert before self');
        if(main.removeChild(b)!==b||b.parentNode!==null||b.isConnected||b.textContent!=='b')throw Error('remove');
        const comment=document.createComment('marker');main.insertBefore(comment,main.firstChild);
        if(comment.nodeType!==8||comment.nodeValue!=='marker'||main.textContent!=='olda')throw Error('comment');
        const template=document.createElement('template');if(!(template instanceof HTMLTemplateElement)||!(template.content instanceof DocumentFragment)||template.content!==template.content)throw Error('template');
        template.content.appendChild(document.createTextNode('inert'));if(template.firstChild!==null||template.content.textContent!=='inert')throw Error('template content separation');
    "#,
    );
}
#[test]
fn invalid_tree_changes_are_rejected_before_detachment() {
    let (mut engine, _) = fixture("<!doctype html><main><p>old</p></main>");
    run(
        &mut engine,
        r#"
        function fails(name,fn){try{fn();throw Error('accepted invalid operation')}catch(e){if(e.name!==name)throw e}}
        const main=document.querySelector('main'),p=main.firstChild,doctype=document.firstChild;
        fails('HierarchyRequestError',()=>p.appendChild(main));
        fails('HierarchyRequestError',()=>main.appendChild(document));
        fails('HierarchyRequestError',()=>main.appendChild(doctype));
        fails('HierarchyRequestError',()=>document.appendChild(doctype));
        const extra=document.createElement('html');fails('HierarchyRequestError',()=>document.appendChild(extra));
        fails('HierarchyRequestError',()=>document.appendChild(document.createTextNode('text')));
        fails('NotFoundError',()=>document.head.removeChild(p));
        fails('NotFoundError',()=>document.head.insertBefore(extra,p));
        fails('InvalidCharacterError',()=>document.createElement('<link>'));
        fails('InvalidCharacterError',()=>document.createElement('bad name'));
        fails('InvalidCharacterError',()=>document.createElement('1bad'));
        fails('TypeError',()=>document.createElement());
        fails('TypeError',()=>p.appendChild({}));
        if(main.parentNode!==document.body||p.parentNode!==main||extra.parentNode!==null||doctype.parentNode!==document)throw Error('non-atomic rejection');
        const fragment=document.createDocumentFragment();fragment.appendChild(document.createElement('div'));fragment.appendChild(document.createElement('div'));
        fails('HierarchyRequestError',()=>document.appendChild(fragment));
        if(fragment.firstChild===null||fragment.firstChild===fragment.lastChild)throw Error('fragment changed before rejection');
        const template=document.createElement('template');
        fails('HierarchyRequestError',()=>template.content.appendChild(template));
        if(template.content.firstChild!==null||template.parentNode!==null)throw Error('template host cycle mutated tree');
    "#,
    );
}
#[test]
fn detached_node_creation_and_template_allocation_share_node_quota() {
    let (mut engine, doc) = fixture("<p>existing</p>");
    let initial = doc.borrow().nodes.len();
    for _ in 0..31 {
        run(
            &mut engine,
            "for(let i=0;i<128;i++)document.createTextNode('x')",
        );
    }
    run(
        &mut engine,
        "for(let i=0;i<127;i++)document.createTextNode('x')",
    );
    assert_eq!(doc.borrow().nodes.len(), initial + 4095);
    run(
        &mut engine,
        "try{document.createElement('template');throw Error('template exceeded quota')}catch(e){if(e.name!=='QuotaExceededError')throw e}",
    );
    assert_eq!(doc.borrow().nodes.len(), initial + 4095);
    run(
        &mut engine,
        "document.createComment('last');try{document.createElement('link');throw Error('creation exceeded quota')}catch(e){if(e.name!=='QuotaExceededError')throw e};document.body.appendChild(document.querySelector('p'))",
    );
    assert_eq!(doc.borrow().nodes.len(), initial + 4096);
}

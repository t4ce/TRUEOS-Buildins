use crate::javascript_dom::{self, SharedDocument};
use hull_quickjs::Engine;
use std::{cell::RefCell, rc::Rc};

fn fixture(html: &str) -> (Engine, SharedDocument) {
    let document = Rc::new(RefCell::new(rp5::parse(html)));
    let mut engine = Engine::new().unwrap();
    javascript_dom::install(&mut engine, document.clone()).unwrap();
    (engine, document)
}
fn evaluate(engine: &mut Engine, source: &str) {
    engine
        .eval(source.as_bytes(), "dom-fixture.js", false)
        .unwrap();
}

#[test]
fn selectors_create_lazy_identity_preserving_standard_wrappers() {
    let (mut engine, _) =
        fixture("<main id='box'><p class='note'>one</p><p class='note'>two</p></main>");
    evaluate(
        &mut engine,
        r#"
        const main=document.getElementById('box');
        if(document.currentScript!==null)throw Error('currentScript outside script execution');
        if(main!==document.querySelector('main#box'))throw Error('identity');
        if(!(main instanceof Element)||!(main instanceof Node)||!(document instanceof Document))throw Error('interfaces');
        const notes=main.querySelectorAll(':scope > p.note');
        if(!(notes instanceof NodeList)||notes.length!==2||notes.item(0)!==notes[0]||notes.item(2)!==null||notes[2]!==undefined)throw Error('NodeList');
        if([...notes].map(node=>node.textContent).join(',')!=='one,two')throw Error('order');
        notes.forEach((node,index,list)=>{if(list!==notes||list[index]!==node)throw Error('foreach');});
        if(main.querySelector('main')!==null||main.querySelector('p')!==notes[0])throw Error('element descendants');
        if(document.querySelector(':scope')!==document.documentElement)throw Error('document scope');
        if(document.getElementById('missing')!==null||document.getElementById('')!==null)throw Error('missing');
        if(main.tagName!=='MAIN'||main.nodeName!=='MAIN'||main.localName!=='main'||main.nodeType!==Node.ELEMENT_NODE)throw Error('names');
        if(main.ownerDocument!==document||document.ownerDocument!==null||!main.isConnected)throw Error('document relation');
        Array.prototype.toJSON=()=>{throw Error('inherited transport hook')};
        if(document.getElementById('box')!==main)throw Error('transport corrupted');
        delete Array.prototype.toJSON;
        if(document.head.tagName!=='HEAD'||document.body.tagName!=='BODY'||document.documentElement.tagName!=='HTML')throw Error('document roots');
        if(typeof __hullDom!=='undefined'||typeof fetch!=='undefined'||typeof window!=='undefined'||typeof document.createElement!=='undefined')throw Error('unexpected host API');
    "#,
    );
}

#[test]
fn attributes_reflect_into_rp5_and_dom_string_conversion_is_standard() {
    let (mut engine, document) = fixture("<div id='before' class='old' title='x'></div>");
    evaluate(
        &mut engine,
        r#"
        const node=document.getElementById('before');
        node.id='after';node.className='new';node.setAttribute('DATA-COUNT',123);
        node.setAttribute('style','background-color:red');node.removeAttribute('title');
        if(document.getElementById('before')!==null||document.getElementById('after')!==node||document.querySelector('.new[data-count="123"]')!==node)throw Error('reflection');
        if(node.id!=='after'||node.className!=='new'||node.getAttribute('data-count')!=='123'||node.getAttribute('title')!==null||node.hasAttribute('title'))throw Error('attributes');
        node.setAttribute('nullable',null);if(node.getAttribute('nullable')!=='null')throw Error('null DOMString');
        node.setAttribute('1@','valid');if(node.getAttribute('1@')!=='valid')throw Error('current DOM attribute name rules');
    "#,
    );
    let doc = document.borrow();
    let id = doc
        .nodes
        .iter()
        .enumerate()
        .find_map(|(id, _)| (doc.name(id) == "div").then_some(id))
        .unwrap();
    let (_, _, attrs) = doc.element(id).unwrap();
    assert!(
        attrs
            .iter()
            .any(|a| a.name == "style" && a.value == "background-color:red")
    );
    assert!(attrs.iter().any(|a| a.name == "id" && a.value == "after"));
}

#[test]
fn text_content_replaces_children_but_old_wrappers_and_static_lists_remain_valid() {
    let (mut engine, document) = fixture("<div id='box'>before<!-- comment --><b>bold</b></div>");
    evaluate(
        &mut engine,
        r#"
        const box=document.getElementById('box'),oldText=box.firstChild,comment=oldText.nextSibling;
        const old=document.querySelectorAll('b'),bold=old[0];
        if(box.textContent!=='beforebold'||comment.textContent!==' comment '||oldText.nodeValue!=='before')throw Error('text reads');
        box.textContent='after';
        if(box.textContent!=='after'||box.firstChild.textContent!=='after'||box.firstChild===oldText)throw Error('replacement');
        if(bold.parentNode!==null||bold.isConnected||bold.ownerDocument!==document||old.length!==1||old[0]!==bold||document.querySelector('b')!==null)throw Error('detached nodes');
        bold.textContent='detached';if(bold.textContent!=='detached'||box.textContent!=='after')throw Error('detached mutation');
        box.textContent=null;if(box.firstChild!==null||box.textContent!=='')throw Error('nullable text');
        box.textContent='again';box.textContent=undefined;if(box.textContent!=='')throw Error('undefined nullable text');
        oldText.nodeValue=undefined;if(oldText.nodeValue!=='')throw Error('undefined nullable nodeValue');
        document.textContent='ignored';if(document.textContent!==null||document.body===null)throw Error('Document setter');
        box.nodeValue='ignored';if(box.nodeValue!==null)throw Error('element nodeValue');
    "#,
    );
    assert!(
        !document
            .borrow()
            .text_content(document.borrow().root)
            .contains("after")
    );
}

#[test]
fn invalid_names_selectors_arguments_and_unimplemented_selectors_fail_explicitly() {
    let (mut engine, _) = fixture("<div id='box'></div><svg class='icon'></svg>");
    evaluate(
        &mut engine,
        r#"
        const box=document.getElementById('box');
        for(const name of ['', 'a b','a/b','a=b','a>b','a\0b']) {
            let failed=false;try{box.setAttribute(name,'x')}catch(error){failed=error instanceof DOMException&&error.name==='InvalidCharacterError'&&error.code===5}
            if(!failed)throw Error('invalid name accepted: '+name);
        }
        let failed=false;try{document.querySelector('[')}catch(error){failed=error instanceof DOMException&&error.name==='SyntaxError'&&error.code===12}
        if(!failed)throw Error('invalid selector accepted');
        failed=false;try{document.querySelector('div:has(p)')}catch(error){failed=error.name==='NotSupportedError'}
        if(!failed)throw Error('unsupported selector silently accepted');
        for(const call of [()=>box.setAttribute('a'),()=>box.getAttribute(),()=>document.querySelector(),()=>document.getElementById(),()=>box.id=Symbol('x'),()=>new Element(),()=>new NodeList()]) {
            failed=false;try{call()}catch(error){failed=error instanceof TypeError}if(!failed)throw Error('missing TypeError');
        }
        failed=false;try{box.textContent='\ud800'}catch(error){failed=error.name==='NotSupportedError'}
        if(!failed)throw Error('unrepresentable DOMString was silently changed');
        failed=false;try{document.querySelector('svg').className}catch(error){failed=error.name==='NotSupportedError'}
        if(!failed)throw Error('SVGAnimatedString was approximated');
    "#,
    );
}

#[test]
fn mutation_limits_reject_writes_before_changing_the_document() {
    let (mut engine, _) = fixture("<div id='box'>before</div>");
    evaluate(
        &mut engine,
        r#"
        const box=document.getElementById('box');
        let failed=false;try{box.textContent='x'.repeat(256*1024+1)}catch(error){failed=error.name==='QuotaExceededError'}
        if(!failed||box.textContent!=='before')throw Error('non-atomic quota rejection');
        box.textContent='small';if(box.textContent!=='small')throw Error('quota poisoned later work');
    "#,
    );
}

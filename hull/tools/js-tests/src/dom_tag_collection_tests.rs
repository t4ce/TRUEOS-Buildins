use crate::javascript_dom::{self, SharedDocument};
use hull_quickjs::Engine;
use std::{cell::RefCell, rc::Rc};

fn fixture(html: &str) -> Engine {
    let document: SharedDocument = Rc::new(RefCell::new(rp5::parse(html)));
    let mut engine = Engine::new().unwrap();
    javascript_dom::install(&mut engine, document).unwrap();
    engine
}

fn engine_with_manual_foreign_case() -> Engine {
    let mut document = rp5::parse(
        "<main><svg><g name='foreignName'></g></svg><p id='htmlId' name='htmlName'></p></main>",
    );
    let svg = document
        .nodes
        .iter()
        .enumerate()
        .find_map(|(id, _)| (document.name(id) == "svg").then_some(id))
        .unwrap();
    for name in ["linearGradient", "lineargradient"] {
        let id = document.add(rp5::NodeKind::Element {
            name: name.into(),
            namespace: rp5::Namespace::Svg,
            attrs: vec![],
            template_content: None,
        });
        document.append(svg, id);
    }
    let document = Rc::new(RefCell::new(document));
    let mut engine = Engine::new().unwrap();
    javascript_dom::install(&mut engine, document).unwrap();
    engine
}

#[test]
fn tag_collections_are_live_tree_ordered_and_support_html_collection_access() {
    let mut engine = fixture(
        "<main><p id='first' name='named'>one</p><section><p id='second'>two</p></section></main>",
    );
    engine.eval(br#"
        const main=document.querySelector('main'), ps=main.getElementsByTagName('P');
        if (!(ps instanceof HTMLCollection) || ps.length!==2 || ps[0]!==ps.item(0) || ps.item(9)!==null || ps[9]!==undefined) throw Error('collection access');
        if (ps.namedItem('first')!==ps[0] || ps.namedItem('named')!==ps[0] || ps.namedItem('missing')!==null || ps.namedItem('')!==null) throw Error('namedItem');
        if ([...ps].map(x=>x.id).join(',')!=='first,second') throw Error('iteration order');
        if (typeof ps.values!=='undefined' || typeof ps.keys!=='undefined' || typeof ps.entries!=='undefined') throw Error('invented collection methods');
        if (!(0 in ps) || (2 in ps) || ps.first!==ps[0] || ps.named!==ps[0] || ps.constructor!==HTMLCollection) throw Error('legacy named properties');
        if (Object.keys(ps).join(',')!=='0,1' || !Object.getOwnPropertyNames(ps).includes('first')) throw Error('property enumeration');
        const desc=Object.getOwnPropertyDescriptor(ps,'0'); if(!desc || desc.writable || !desc.enumerable || desc.value!==ps[0]) throw Error('index descriptor');
        const first=ps[0]; ps[0]=null; if(ps[0]!==first) throw Error('readonly index');
        if(delete ps[0] || delete ps.first) throw Error('readonly indexed or named property');
        let namedDefine=false; try { Object.defineProperty(ps,'first',{value:null}); } catch(e) { namedDefine=e instanceof TypeError; }
        if(!namedDefine || ps.first!==first) throw Error('named property definition');
        first.id='temporary'; ps.first='expando'; first.id='first';
        if(ps.first!=='expando' || ps.namedItem('first')!==first || Object.keys(ps).filter(k=>k==='first').length!==1) throw Error('expando shadow / ownKeys deduplication');
        delete ps.first; if(ps.first!==first) throw Error('named property restoration');
        let receiverErrors=0;
        try { Document.prototype.getElementsByTagName.call(main,'p'); } catch(e) { if(e instanceof TypeError) receiverErrors++; }
        try { Element.prototype.getElementsByTagName.call(document,'p'); } catch(e) { if(e instanceof TypeError) receiverErrors++; }
        try { Element.prototype.getElementsByTagName.call(first.firstChild,'p'); } catch(e) { if(e instanceof TypeError) receiverErrors++; }
        if(receiverErrors!==3) throw Error('interface receiver checks');
        let bad=false; try { ps.item(Symbol()); } catch(e) { bad=e instanceof TypeError; } if(!bad) throw Error('WebIDL item conversion');
        const retained=ps[0]; main.textContent='replaced';
        if(ps.length!==0 || ps.item(0)!==null || retained.parentNode!==null) throw Error('live collection');
    "#, "tag-collection.js", false).unwrap();
}

#[test]
fn tag_matching_handles_wildcards_foreign_case_and_bounded_errors() {
    let mut engine = engine_with_manual_foreign_case();
    engine.eval(br#"
        const main=document.querySelector('main');
        if(main.getElementsByTagName('*').length!==5) throw Error('wildcard');
        if(main.getElementsByTagName('linearGradient').length!==1 || main.getElementsByTagName('lineargradient').length!==1) throw Error('foreign case sensitivity');
        if(main.getElementsByTagName('P').length!==1 || document.getElementsByTagName('p').length!==1) throw Error('HTML ASCII case folding');
        if(main.getElementsByTagName('p#x').length!==0) throw Error('tag names are not CSS selectors');
        const all=main.getElementsByTagName('*'); if(all.namedItem('foreignName')!==null || all.foreignName!==undefined) throw Error('foreign name is not an HTMLCollection name');
        if(all.namedItem('htmlName')!==document.getElementById('htmlId') || all.htmlName!==document.getElementById('htmlId')) throw Error('HTML name property');
        if(all.namedItem('htmlId')!==document.getElementById('htmlId') || all.htmlId!==document.getElementById('htmlId')) throw Error('id property');
        const getAttribute=Element.prototype.getAttribute, namespace=Object.getOwnPropertyDescriptor(Element.prototype,'namespaceURI');
        Element.prototype.getAttribute=function(){return 'spoof';};
        Object.defineProperty(Element.prototype,'namespaceURI',{configurable:true,get(){return 'http://www.w3.org/1999/xhtml';}});
        if(all.namedItem('htmlId')!==document.getElementById('htmlId') || all.namedItem('foreignName')!==null) throw Error('named lookup trusts live DOM bridge');
        Element.prototype.getAttribute=getAttribute; Object.defineProperty(Element.prototype,'namespaceURI',namespace);
        let failed=false; try { main.getElementsByTagName('x'.repeat(262145)).length; } catch(e) { failed=e.name==='QuotaExceededError'; }
        if(!failed) throw Error('string bound');
    "#, "tag-collection.js", false).unwrap();
}

#[test]
fn numeric_names_do_not_override_array_index_lookup_and_item_uses_webidl_numbers() {
    let mut engine = fixture("<main><p id='9'></p></main>");
    engine.eval(br#"
        const c=document.querySelector('main').getElementsByTagName('p'), p=c.item(0);
        if(c['9']!==undefined || ('9' in c) || Object.getOwnPropertyDescriptor(c,'9')!==undefined || c.namedItem('9')!==p) throw Error('numeric array-index lookup');
        if(c.item('0')!==p || c.item(-4294967296)!==p || c.item(NaN)!==p) throw Error('unsigned long conversion');
        let big=false; try { c.item(1n); } catch(e) { big=e instanceof TypeError; } if(!big) throw Error('BigInt conversion');
    "#, "tag-collection.js", false).unwrap();
}

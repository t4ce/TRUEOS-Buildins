//! Bounded media-only subdocument inspection. No browsing contexts or rendering.
use super::{LocalPage, module_loader::{Loader, Prepared}};
use crate::javascript_dom;
use hull_quickjs::Engine;
use rp5::{Document,Namespace};
use std::{cell::{Cell,RefCell},collections::{HashSet,VecDeque},rc::Rc,time::{Duration,Instant}};
#[derive(Default)]
pub(super) struct Resolver { initialized:bool, pending:VecDeque<(String,usize)>,seen:HashSet<String> }
fn attr<'a>(doc:&'a Document,id:usize,name:&str)->Option<&'a str>{doc.element(id)?.2.iter().find(|a|a.namespace.is_none()&&a.name==name).map(|a|a.value.as_str())}
fn address(base:&str,text:&str)->Option<String>{let u=url::Url::parse(base).ok()?.join(text).ok()?;(crate::resource_url::allowed(&u)&&u.scheme()=="https").then(||u.to_string())}
impl Resolver {
    fn enqueue(&mut self,url:String,depth:usize){if depth<=3 && self.seen.len()<9 && self.seen.insert(url.clone()){self.pending.push_back((url,depth));}}
    fn discover(&mut self,doc:&Document,base:&str,depth:usize,page:&LocalPage)->bool {
        let mut pending=vec![doc.root];
        while let Some(id)=pending.pop(){
            pending.extend(doc.nodes[id].children.iter().rev().copied());
            let Some((name,Namespace::Html,_))=doc.element(id) else {continue};
            if name=="video" {
                let source=attr(doc,id,"src").or_else(||doc.nodes[id].children.iter().find_map(|&n|(doc.name(n)=="source").then(||attr(doc,n,"src")).flatten()));
                if let Some(url)=source.and_then(|s|address(base,s)) {page.media.borrow_mut().request(id,url);return true;}
            }
            if name=="iframe" {
                let label=format!("{} {} {}",attr(doc,id,"title").unwrap_or(""),attr(doc,id,"id").unwrap_or(""),attr(doc,id,"name").unwrap_or("")).to_ascii_lowercase();
                if label.contains("video")||label.contains("player") {
                    if let Some(url)=attr(doc,id,"src").and_then(|s|address(base,s)){self.enqueue(url,depth+1);}
                }
            }
        }
        false
    }
    pub(super) fn poll(&mut self,loader:&mut Loader,parent:&mut Engine,page:&LocalPage){
        if !self.initialized {
            self.initialized=true;
            if let Some(doc)=&page.document {if self.discover(&doc.borrow(),&page.url,0,page){return;}}
        }
        let Some((url,depth))=self.pending.front().cloned() else {return};
        let bytes=match loader.source(&url){Prepared::Pending=>return,Prepared::Failed(_)=>{self.pending.pop_front();return},Prepared::Ready(b)=>b};
        self.pending.pop_front();
        if bytes.is_empty()||bytes.len()>256*1024 {return;}
        let doc=rp5::parse(&String::from_utf8_lossy(&bytes));
        if self.discover(&doc,&url,depth,page){self.pending.clear();return;}
        // Execute only this tiny page's inline control code in a disposable
        // realm. Location writes are captured, never applied. No external JS,
        // timers, frames, network exports, player UI, or new worker are created.
        let remaining=parent.execution_budget_remaining().unwrap_or(Duration::ZERO);
        if remaining.is_zero(){return;}
        let start=Instant::now();
        let _=self.inspect(doc,&url,depth,page,remaining.min(Duration::from_millis(25)));
        parent.set_execution_budget(remaining.saturating_sub(start.elapsed()));
    }
    fn inspect(&mut self,doc:Document,url:&str,depth:usize,page:&LocalPage,budget:Duration)->Result<(),String>{
        let mut scripts=crate::scripts::Scripts::default();scripts.extract_quiet(&doc,url);
        let click=doc.nodes.iter().enumerate().find_map(|(id,_)|{
            if doc.name(id)!="button" {return None;}
            let label=format!("{} {}",attr(&doc,id,"id").unwrap_or(""),doc.text_content(id)).to_ascii_lowercase();
            if label.contains("play") {attr(&doc,id,"onclick").map(str::to_owned)}else{None}
        });
        let mut engine=Engine::new()?;engine.set_execution_budget(budget);engine.install_console()?;
        let state:javascript_dom::SharedScriptState=Default::default();state.borrow_mut().media_navigation=true;
        javascript_dom::install_for_page_with_cookies(&mut engine,Rc::new(RefCell::new(doc)),page.window.clone(),Rc::new(Cell::new(None)),url,page.cookies.clone(),None,Some(page.media.clone()),state)?;
        engine.eval(include_bytes!("javascript_player_kt.js"),"hull:media-player-adapter",false)?;
        for source in scripts.sources.iter().filter_map(|s|s.bytes.as_ref()).filter(|b|b.len()<=64*1024).take(8){
            if engine.execution_budget_exhausted(){break;}
            let _=engine.eval(source,url,false);
        }
        if let Some(click)=click {let _=engine.eval(click.as_bytes(),"hull:media-play-control",false);}
        let base=url::Url::parse(url).map_err(|e|e.to_string())?;
        let navigations=std::mem::take(&mut page.media.borrow_mut().inspection_navigation);
        for target in navigations {
            if let Some(target)=address(url,&target).filter(|s|url::Url::parse(s).is_ok_and(|u|u.origin()==base.origin())){self.enqueue(target,depth+1);}
        }
        Ok(())
    }
}

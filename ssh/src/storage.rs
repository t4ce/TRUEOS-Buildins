//! Identity creation and durable TOFU learning, using checked record publication.
use alloc::{format,string::{String,ToString},vec::Vec};
use sha2::{Digest,Sha256};
use sunset::{KeyType,PubKey,SignKey};
use zeroize::Zeroizing;
use crate::{options::KNOWN_HOSTS,Result};

pub struct Snapshot{pub bytes:Zeroizing<Vec<u8>>}
impl Snapshot{pub fn digest(&self)->[u8;32]{Sha256::digest(self.bytes.as_slice()).into()}}
#[derive(Debug)]pub enum StoreError{Conflict,Other(String)}
impl core::fmt::Display for StoreError{fn fmt(&self,f:&mut core::fmt::Formatter<'_>)->core::fmt::Result{match self{Self::Conflict=>f.write_str("configuration changed concurrently"),Self::Other(s)=>f.write_str(s)}}}
pub trait Store{
    async fn snapshot(&mut self,path:&str)->core::result::Result<Option<Snapshot>,StoreError>;
    async fn ensure_parent(&mut self,path:&str)->core::result::Result<(),StoreError>;
    async fn publish(&mut self,path:&str,bytes:&[u8],expected:Option<[u8;32]>)->core::result::Result<(),StoreError>;
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]pub enum KnownHostStatus{Match,Changed,Unknown}
pub fn known_host_status(snapshot:Option<&Snapshot>,host:&str,key:&PubKey<'_>)->Result<KnownHostStatus>{
    let Some(snapshot)=snapshot else{return Ok(KnownHostStatus::Unknown)};
    let text=core::str::from_utf8(&snapshot.bytes).map_err(|_|"ssh: known_hosts is not UTF8")?;
    let mut found=false;
    for line in text.lines().map(str::trim){if line.is_empty()||line.starts_with('#'){continue}
        let mut fields=line.split_whitespace();let Some(hosts)=fields.next()else{continue};
        if !hosts.split(',').any(|candidate|candidate==host){continue}found=true;
        let(Some(algorithm),Some(base64))=(fields.next(),fields.next())else{continue};
        if key.matches_openssh(&format!("{algorithm} {base64}")).unwrap_or(false){return Ok(KnownHostStatus::Match)}
    }
    Ok(if found{KnownHostStatus::Changed}else{KnownHostStatus::Unknown})
}
/// Called only after explicit confirmation or --accept-new. Reload at every CAS
/// attempt so a racing changed key can never be appended over and accepted.
pub async fn learn_known_host<S:Store>(store:&mut S,host:&str,key:&PubKey<'_>)->Result<()>{
    let public=key.to_openssh().map_err(|_|"ssh: cannot encode server public key")?;
    store.ensure_parent(KNOWN_HOSTS).await.map_err(|e|e.to_string())?;
    for _ in 0..4{
        let snapshot=store.snapshot(KNOWN_HOSTS).await.map_err(|e|e.to_string())?;
        match known_host_status(snapshot.as_ref(),host,key)?{
            KnownHostStatus::Match=>return Ok(()),
            KnownHostStatus::Changed=>return Err("ssh: HOST KEY CHANGED while saving trust; connection refused".into()),
            KnownHostStatus::Unknown=>{},
        }
        let expected=snapshot.as_ref().map(Snapshot::digest);
        let mut text=String::new();
        if let Some(snapshot)=snapshot.as_ref(){text.push_str(core::str::from_utf8(&snapshot.bytes).map_err(|_|"ssh: known_hosts is not UTF8")?);if !text.is_empty()&&!text.ends_with('\n'){text.push('\n')}}
        text.push_str(&format!("{host} {public}\n"));
        match store.publish(KNOWN_HOSTS,text.as_bytes(),expected).await{
            Ok(())=>return Ok(()),Err(StoreError::Conflict)=>continue,Err(e)=>return Err(format!("ssh: cannot durably save host trust: {e}")),
        }
    }
    Err("ssh: known_hosts kept changing; connection refused".into())
}
pub struct Identity{pub key:SignKey,pub created:bool}
pub async fn load_or_create_identity<S:Store>(store:&mut S,path:&str)->Result<Identity>{
    for _ in 0..4{
        if let Some(snapshot)=store.snapshot(path).await.map_err(|e|e.to_string())?{
            let key=SignKey::import_private(&snapshot.bytes).map_err(|_|"ssh: identity is unsupported, encrypted, or invalid")?;
            return Ok(Identity{key,created:false})
        }
        store.ensure_parent(path).await.map_err(|e|e.to_string())?;
        let key=SignKey::generate(KeyType::Ed25519,None).map_err(|_|"ssh: cannot generate Ed25519 identity")?;
        let private=key.to_openssh_private().map_err(|_|"ssh: cannot encode private identity")?;
        match store.publish(path,private.as_bytes(),None).await{
            Ok(())=>return Ok(Identity{key,created:true}),
            Err(StoreError::Conflict)=>continue, // Load the durable winning identity.
            Err(e)=>return Err(format!("ssh: cannot durably save new identity: {e}")),
        }
    }
    Err("ssh: identity kept changing; authentication refused".into())
}

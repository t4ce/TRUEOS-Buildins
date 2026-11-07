//! Fixed-capacity byte staging with exact accepted-prefix consumption.
use alloc::{string::String,vec::Vec};
use crate::Result;
pub struct Queue{bytes:Vec<u8>,start:usize,end:usize}
impl Queue{
    pub fn new(capacity:usize)->Result<Self>{let mut bytes=Vec::new();bytes.try_reserve_exact(capacity).map_err(|_|String::from("ssh: cannot allocate I/O buffer"))?;bytes.resize(capacity,0);Ok(Self{bytes,start:0,end:0})}
    pub fn len(&self)->usize{self.end-self.start}
    pub fn is_empty(&self)->bool{self.start==self.end}
    pub fn room(&self)->usize{self.bytes.len()-self.len()}
    pub fn bytes(&self)->&[u8]{&self.bytes[self.start..self.end]}
    pub fn consume(&mut self,n:usize){assert!(n<=self.len());self.start+=n;if self.start==self.end{self.start=0;self.end=0}}
    pub fn spare(&mut self)->&mut[u8]{if self.start!=0{self.bytes.copy_within(self.start..self.end,0);self.end-=self.start;self.start=0;} &mut self.bytes[self.end..]}
    pub fn produced(&mut self,n:usize){assert!(n<=self.bytes.len()-self.end);self.end+=n}
    pub fn append(&mut self,bytes:&[u8])->Result<()>{if bytes.len()>self.room(){return Err("ssh: I/O staging buffer full".into())}self.spare()[..bytes.len()].copy_from_slice(bytes);self.produced(bytes.len());Ok(())}
}
#[cfg(test)]mod tests{use super::*;#[test]fn partial_prefix_is_retained_across_compaction(){let mut q=Queue::new(7).unwrap();q.append(b"abcdef").unwrap();q.consume(2);q.append(b"ghi").unwrap();assert_eq!(q.bytes(),b"cdefghi");assert!(q.append(b"x").is_err());q.consume(7);assert!(q.is_empty());assert_eq!(q.room(),7);}}

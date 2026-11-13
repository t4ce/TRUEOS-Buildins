extern crate alloc;
extern crate self as trueos;
use std::{cell::RefCell, future::Future, task::{Context, Poll, Waker}};

pub mod content_identity {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct ContentTypeId(u8);
    impl ContentTypeId {
        pub const TRUEOS_BLUEPRINT: Self = Self(1);
        pub const BLOB: Self = Self(2);
    }
}
pub mod async_fs {
    use super::content_identity::ContentTypeId;
    pub const ERR_NOT_FOUND: i32 = -8;
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum NodeKind { File, Directory }
    #[derive(Clone)]
    pub struct Entry { pub name: String, pub kind: NodeKind, pub content_type: ContentTypeId }
    #[derive(Clone, Default)]
    pub struct Listing { pub entries: Vec<Entry>, pub truncated: bool }
    pub async fn list_dir_typed(path: &[u8]) -> Result<Listing,i32> {
        assert_eq!(path,b"common/dl/apps");
        super::STATE.with_borrow(|result| result.clone())
    }
}
thread_local! { static STATE: RefCell<Result<async_fs::Listing,i32>> = RefCell::new(Ok(async_fs::Listing::default())); }
mod download { pub const DIRECTORY: &str = "common/dl/apps"; }
#[path = "../src/fetched.rs"]
mod fetched;

fn run<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("unexpected fixture wait"),
    }
}

#[test]
fn opening_again_restores_fetched_from_durable_blueprint_files_and_tracks_deletions() {
    use async_fs::{Entry,Listing,NodeKind};
    use content_identity::ContentTypeId as Type;
    STATE.set(Ok(Listing { entries: vec![
        Entry { name:"codex.bp".into(),kind:NodeKind::File,content_type:Type::TRUEOS_BLUEPRINT },
        Entry { name:"sqlite.bp".into(),kind:NodeKind::File,content_type:Type::TRUEOS_BLUEPRINT },
        Entry { name:"folder.bp".into(),kind:NodeKind::Directory,content_type:Type::BLOB },
        Entry { name:"unverified.bp".into(),kind:NodeKind::File,content_type:Type::BLOB },
    ],truncated:false }));
    let first = run(fetched::load()).unwrap();
    assert_eq!(first,["codex.bp".into(),"sqlite.bp".into()].into_iter().collect());
    assert_eq!(run(fetched::load()).unwrap(),first);
    STATE.set(Ok(Listing::default()));
    assert!(run(fetched::load()).unwrap().is_empty());
}

#[test]
fn missing_directory_is_empty_but_read_failures_and_truncation_are_reported() {
    STATE.set(Err(async_fs::ERR_NOT_FOUND));
    assert!(run(fetched::load()).unwrap().is_empty());
    STATE.set(Err(-2));
    assert!(run(fetched::load()).is_err());
    STATE.set(Ok(async_fs::Listing { truncated:true,..Default::default() }));
    assert!(run(fetched::load()).is_err());
}

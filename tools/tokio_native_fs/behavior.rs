//! Real native Tokio FS implementation, with only the kernel broker mocked.
#[path = "../../../TRUEOS/tools/testpy/native_tokio/boundary.rs"]
mod runtime_boundary;

use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Mutex, OnceLock},
    task::{Context, Poll, Waker},
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, ErrorKind, SeekFrom},
};

#[derive(Default)]
struct Broker {
    files: BTreeMap<String, Vec<u8>>,
    operations: BTreeMap<u32, Operation>,
    next: u32,
    calls: Vec<String>,
    delay: usize,
    discarded: usize,
    malformed_stat: bool,
    malformed_list: bool,
}
struct Operation {
    path: String,
    bytes: Vec<u8>,
    delay: usize,
}
static BROKER: OnceLock<Mutex<Broker>> = OnceLock::new();
fn broker() -> std::sync::MutexGuard<'static, Broker> {
    BROKER.get_or_init(Default::default).lock().unwrap()
}
fn operation(b: &mut Broker, path: String, bytes: Vec<u8>) -> i32 {
    b.next += 1;
    b.calls.push(path.clone());
    let id = b.next;
    b.operations.insert(
        id,
        Operation {
            path,
            bytes,
            delay: b.delay,
        },
    );
    id as i32
}
unsafe fn text(p: *const u8, n: usize) -> String {
    unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(p, n))
            .unwrap()
            .to_owned()
    }
}
fn complete<F: Future>(future: F) -> F::Output {
    let mut f = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..10000 {
        if let Poll::Ready(result) = f.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("native filesystem future did not complete");
}

#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_read_start(p: *const u8, n: usize) -> i32 {
    let path = unsafe { text(p, n) };
    let mut b = broker();
    match b.files.get(&path).cloned() {
        Some(bytes) => operation(&mut b, path, bytes),
        None => -5,
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_write_begin(p: *const u8, n: usize, total: usize) -> i32 {
    let path = unsafe { text(p, n) };
    operation(&mut broker(), path, vec![0; total])
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_write_chunk(
    id: u32,
    offset: usize,
    p: *const u8,
    n: usize,
) -> i32 {
    let mut b = broker();
    let op = b.operations.get_mut(&id).unwrap();
    let end = offset.checked_add(n).unwrap();
    assert!(end <= op.bytes.len());
    op.bytes[offset..end].copy_from_slice(unsafe { std::slice::from_raw_parts(p, n) });
    0
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_async_fs_write_commit(id: u32) -> i32 {
    let mut b = broker();
    let op = b.operations.get(&id).unwrap();
    let (path, bytes) = (op.path.clone(), op.bytes.clone());
    b.files.insert(path, bytes);
    0
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_metadata_start_v2(p: *const u8, n: usize) -> i32 {
    let path = unsafe { text(p, n) };
    let mut b = broker();
    let kind = if path.is_empty() || path == "dir" {
        2u32
    } else if b.files.contains_key(&path) {
        1
    } else {
        return -5;
    };
    let len = b.files.get(&path).map_or(0, Vec::len) as u64;
    let mut bytes = Vec::new();
    bytes.extend(kind.to_le_bytes());
    bytes.extend(len.to_le_bytes());
    bytes.extend(77u64.to_le_bytes());
    bytes.extend(3u32.to_le_bytes());
    bytes.extend(23u64.to_le_bytes());
    if b.malformed_stat {
        bytes.pop();
    }
    operation(&mut b, path, bytes)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_list_dir_start(p: *const u8, n: usize) -> i32 {
    let path = unsafe { text(p, n) };
    let mut b = broker();
    let names: Vec<String> = b
        .files
        .keys()
        .filter(|s| !s.contains('/'))
        .cloned()
        .collect();
    let mut bytes = b"TDL1\0\0\0\0".to_vec();
    bytes.extend((names.len() as u32).to_le_bytes());
    for name in names {
        bytes.extend([1, 0]);
        bytes.extend((name.len() as u16).to_le_bytes());
        bytes.extend(name.as_bytes());
    }
    if b.malformed_list {
        bytes.push(9);
    }
    operation(&mut b, path, bytes)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_create_dir_all_start(p: *const u8, n: usize) -> i32 {
    operation(&mut broker(), unsafe { text(p, n) }, Vec::new())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_remove_start(p: *const u8, n: usize) -> i32 {
    let path = unsafe { text(p, n) };
    let mut b = broker();
    if b.files.remove(&path).is_none() {
        return -5;
    }
    operation(&mut b, path, Vec::new())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_rename_start(
    p: *const u8,
    n: usize,
    q: *const u8,
    m: usize,
) -> i32 {
    let (path, dest) = unsafe { (text(p, n), text(q, m)) };
    let mut b = broker();
    if b.files.contains_key(&dest) {
        return -9;
    }
    let Some(bytes) = b.files.remove(&path) else {
        return -5;
    };
    b.files.insert(dest, bytes);
    operation(&mut b, path, Vec::new())
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_async_fs_status(id: u32) -> i32 {
    let mut b = broker();
    let op = b.operations.get_mut(&id).unwrap();
    if op.delay > 0 {
        op.delay -= 1;
        0
    } else {
        1
    }
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_async_fs_result_len(id: u32) -> isize {
    broker().operations[&id].bytes.len() as isize
}
#[unsafe(no_mangle)]
unsafe extern "C" fn trueos_cabi_async_fs_result_read(
    id: u32,
    offset: usize,
    p: *mut u8,
    n: usize,
) -> isize {
    let b = broker();
    let bytes = &b.operations[&id].bytes;
    let count = n.min(bytes.len().saturating_sub(offset));
    unsafe { std::ptr::copy_nonoverlapping(bytes[offset..].as_ptr(), p, count) };
    count as isize
}
#[unsafe(no_mangle)]
extern "C" fn trueos_cabi_async_fs_discard(id: u32) -> i32 {
    let mut b = broker();
    assert!(b.operations.remove(&id).is_some());
    b.discarded += 1;
    0
}

fn main() {
    let mut cases = 0;
    complete(async {
        let payload: Vec<u8> = (0..200_001).map(|v| (v % 251) as u8).collect();
        fs::write("/./large.bin", &payload).await.unwrap();
        assert_eq!(fs::read("large.bin").await.unwrap(), payload);
        cases += 1;
        let calls = broker().calls.len();
        for bad in ["../escape", "a/../escape", "bad\0path", ""] {
            assert_eq!(
                fs::read(bad).await.unwrap_err().kind(),
                ErrorKind::InvalidInput
            );
        }
        assert_eq!(broker().calls.len(), calls);
        cases += 1;
        assert_eq!(
            fs::canonicalize("//./large.bin").await.unwrap().as_str(),
            "/large.bin"
        );
        assert_eq!(fs::canonicalize("/").await.unwrap().as_str(), "/");
        assert!(!fs::try_exists("missing").await.unwrap());
        cases += 1;
        let md = fs::metadata("large.bin").await.unwrap();
        assert_eq!(
            (md.file_id(), md.root_id(), md.revision(), md.len()),
            (Some(77), 3, Some(23), 200_001)
        );
        assert!(md.created().is_err() && md.modified().is_err());
        let mut perms = md.permissions();
        assert!(!perms.readonly());
        perms.set_readonly(true);
        fs::set_permissions("large.bin", perms).await.unwrap();
        assert_eq!(
            fs::set_permissions("missing", md.permissions())
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
        cases += 1;
        let mut f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("large.bin")
            .await
            .unwrap();
        f.seek(SeekFrom::Start(3)).await.unwrap();
        f.write_all(b"xyz").await.unwrap();
        f.flush().await.unwrap();
        assert_eq!(&fs::read("large.bin").await.unwrap()[3..6], b"xyz");
        cases += 1;
        let t = fs::FileTimestamp::from_unix_duration(std::time::Duration::from_secs(23));
        assert_eq!(t.as_unix_duration().as_secs(), 23);
        let err = f
            .set_times(fs::FileTimes::new().set_accessed(t).set_modified(t))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("does not expose writable file timestamps")
        );
        cases += 1;
        let native = f.into_native().await;
        let mut f = fs::File::from_native(native);
        f.seek(SeekFrom::Start(0)).await.unwrap();
        let mut bytes = [0; 3];
        f.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, &payload[..3]);
        cases += 1;
        assert_eq!(
            fs::OpenOptions::new()
                .read(true)
                .create_new(true)
                .open("new")
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidInput
        );
        assert_eq!(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open("large.bin")
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::AlreadyExists
        );
        cases += 1;
        fs::write("name\nwith unicode ü", b"hello").await.unwrap();
        let mut dir = fs::read_dir("/").await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = dir.next_entry().await.unwrap() {
            names.push(entry.file_name());
            assert!(entry.metadata().await.unwrap().file_id().is_some());
        }
        assert!(names.contains(&"name\nwith unicode ü".to_owned()));
        cases += 1;
        fs::rename("name\nwith unicode ü", "renamed").await.unwrap();
        assert_eq!(fs::read("renamed").await.unwrap(), b"hello");
        assert_eq!(
            fs::rename("renamed", "large.bin").await.unwrap_err().kind(),
            ErrorKind::AlreadyExists
        );
        fs::remove_file("renamed").await.unwrap();
        assert!(!fs::try_exists("renamed").await.unwrap());
        cases += 1;
        broker().malformed_stat = true;
        assert!(fs::metadata("large.bin").await.is_err());
        broker().malformed_stat = false;
        broker().malformed_list = true;
        assert_eq!(
            fs::read_dir("/").await.unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        broker().malformed_list = false;
        cases += 1;
        broker().delay = 5;
        {
            let mut read = std::pin::pin!(fs::read("large.bin"));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(read.as_mut().poll(&mut cx).is_pending());
        }
        assert!(broker().operations.is_empty());
        broker().delay = 0;
        cases += 1;
        assert_eq!(
            fs::read_to_string("large.bin").await.unwrap_err().kind(),
            ErrorKind::InvalidData
        );
        cases += 1;
        let path = fs::Path::new("/dir");
        assert_eq!(path.join("file").as_str(), "/dir/file");
        assert_eq!(path.join("/replacement").as_str(), "/replacement");
        cases += 1;
        fs::write("clone", b"abcdef").await.unwrap();
        let mut original = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("clone")
            .await
            .unwrap();
        let mut cloned = original.try_clone().await.unwrap();
        original.seek(SeekFrom::Start(2)).await.unwrap();
        let mut byte = [0];
        cloned.read_exact(&mut byte).await.unwrap();
        assert_eq!(byte, [b'c']);
        cloned.write_all(b"!").await.unwrap();
        assert_eq!(original.stream_position().await.unwrap(), 4);
        original.flush().await.unwrap();
        assert_eq!(fs::read("clone").await.unwrap(), b"abc!ef");
        cases += 1;
        original.seek(SeekFrom::Start(9)).await.unwrap();
        original.set_len(2).await.unwrap();
        assert_eq!(cloned.stream_position().await.unwrap(), 9);
        original.write_all(b"z").await.unwrap();
        original.flush().await.unwrap();
        assert_eq!(fs::read("clone").await.unwrap(), b"ab\0\0\0\0\0\0\0z");
        cases += 1;
    });
    assert!(broker().operations.is_empty());
    println!("{cases} native filesystem boundary cases passed");
}

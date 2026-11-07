//! One best-effort local type association after a completed transfer.
use sha2::{Digest, Sha256};
const MAX_ASSOCIATION_BYTES: u64 = 16 * 1024 * 1024;
trait Backend {
    type Kind: Copy + PartialEq;
    fn blob(&self) -> Self::Kind;
    async fn metadata(&mut self, path: &str) -> Result<(u64, Self::Kind), i32>;
    async fn read(&mut self, path: &str) -> Result<alloc::vec::Vec<u8>, i32>;
    fn detect(&self, path: &str, bytes: &[u8]) -> Option<Self::Kind>;
    async fn promote(
        &mut self,
        path: &str,
        bytes: &[u8],
        kind: Self::Kind,
        digest: [u8; 32],
    ) -> Result<(), i32>;
}
async fn attempt<B: Backend>(backend: &mut B, path: &str) -> Result<Option<B::Kind>, i32> {
    let (size, kind) = backend.metadata(path).await?;
    // The existing guarded API snapshots and rewrites locally. Bound that
    // allocation; large/unknown files remain valid Blob downloads.
    if kind != backend.blob() || size > MAX_ASSOCIATION_BYTES {
        return Ok(None);
    }
    let bytes = backend.read(path).await?;
    if bytes.len() as u64 != size {
        return Err(-17);
    }
    let Some(kind) = backend
        .detect(path, &bytes)
        .filter(|kind| *kind != backend.blob())
    else {
        return Ok(None);
    };
    let digest = Sha256::digest(&bytes).into();
    backend.promote(path, &bytes, kind, digest).await?;
    Ok(Some(kind))
}
#[cfg(any(target_os = "trueos", target_os = "zkvm"))]
pub(crate) async fn completed(path: &crate::path::Path) {
    use trueos::async_fs;
    struct Native;
    impl Backend for Native {
        type Kind = async_fs::ContentTypeId;
        fn blob(&self) -> Self::Kind {
            Self::Kind::BLOB
        }
        async fn metadata(&mut self, path: &str) -> Result<(u64, Self::Kind), i32> {
            let info = async_fs::typed_metadata(path.as_bytes()).await?;
            Ok((info.len, info.content_type))
        }
        async fn read(&mut self, path: &str) -> Result<alloc::vec::Vec<u8>, i32> {
            async_fs::read_file(path.as_bytes()).await
        }
        fn detect(&self, path: &str, bytes: &[u8]) -> Option<Self::Kind> {
            async_fs::infer_named_bytes(path, bytes)
        }
        async fn promote(
            &mut self,
            path: &str,
            bytes: &[u8],
            kind: Self::Kind,
            digest: [u8; 32],
        ) -> Result<(), i32> {
            async_fs::infer_blob_checked(path.as_bytes(), bytes, kind, digest).await
        }
    }
    let path = path.as_str();
    match attempt(&mut Native, path).await {
        Ok(Some(kind)) => println!("Trauma: associated {path} as content type {}", kind.raw()),
        Ok(None) => {}
        Err(code) => {
            eprintln!("Trauma: type association unavailable ({code}); saved bytes retained: {path}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        bytes: alloc::vec::Vec<u8>,
        size: u64,
        kind: u32,
        detected: Option<u32>,
        reads: usize,
        promotions: usize,
        fail: bool,
    }
    impl Backend for Fake {
        type Kind = u32;
        fn blob(&self) -> u32 {
            0
        }
        async fn metadata(&mut self, _: &str) -> Result<(u64, u32), i32> {
            Ok((self.size, self.kind))
        }
        async fn read(&mut self, _: &str) -> Result<alloc::vec::Vec<u8>, i32> {
            self.reads += 1;
            Ok(self.bytes.clone())
        }
        fn detect(&self, _: &str, _: &[u8]) -> Option<u32> {
            self.detected
        }
        async fn promote(
            &mut self,
            _: &str,
            bytes: &[u8],
            kind: u32,
            digest: [u8; 32],
        ) -> Result<(), i32> {
            self.promotions += 1;
            assert_eq!(digest, <[u8; 32]>::from(Sha256::digest(bytes)));
            if self.fail {
                return Err(-17);
            }
            self.kind = kind;
            Ok(())
        }
    }
    #[test]
    fn associates_once_and_keeps_fallback_bytes_on_unknown_conflict_or_budget() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut file = Fake {
                bytes: b"saved bytes".to_vec(),
                size: 11,
                kind: 0,
                detected: Some(7),
                reads: 0,
                promotions: 0,
                fail: false,
            };
            assert_eq!(attempt(&mut file, "file.png").await, Ok(Some(7)));
            assert_eq!(attempt(&mut file, "file.png").await, Ok(None));
            assert_eq!((file.reads, file.promotions), (1, 1));
            file.kind = 0;
            file.detected = None;
            assert_eq!(attempt(&mut file, "unknown.bin").await, Ok(None));
            assert_eq!(file.promotions, 1);
            file.detected = Some(7);
            file.fail = true;
            assert_eq!(attempt(&mut file, "file.png").await, Err(-17));
            assert_eq!(file.kind, 0);
            assert_eq!(file.bytes, b"saved bytes");
            file.size = MAX_ASSOCIATION_BYTES + 1;
            let reads = file.reads;
            assert_eq!(attempt(&mut file, "large.zip").await, Ok(None));
            assert_eq!(file.reads, reads);
        });
    }
}

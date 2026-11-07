//! Identity creation and durable TOFU learning, using checked record publication.
use crate::{Result, options::KNOWN_HOSTS};
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use sha2::{Digest, Sha256};
use sunset::{KeyType, PubKey, SignKey};
use zeroize::Zeroizing;

pub struct Snapshot {
    pub bytes: Zeroizing<Vec<u8>>,
}
impl Snapshot {
    pub fn digest(&self) -> [u8; 32] {
        Sha256::digest(self.bytes.as_slice()).into()
    }
}
#[derive(Debug)]
pub enum StoreError {
    Conflict,
    Other(String),
}
impl core::fmt::Display for StoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Conflict => f.write_str("configuration changed concurrently"),
            Self::Other(s) => f.write_str(s),
        }
    }
}
pub trait Store {
    async fn snapshot(&mut self, path: &str) -> core::result::Result<Option<Snapshot>, StoreError>;
    async fn ensure_parent(&mut self, path: &str) -> core::result::Result<(), StoreError>;
    async fn publish(
        &mut self,
        path: &str,
        bytes: &[u8],
        expected: Option<[u8; 32]>,
    ) -> core::result::Result<(), StoreError>;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnownHostStatus {
    Match,
    Changed,
    Unknown,
}
pub fn known_host_status(
    snapshot: Option<&Snapshot>,
    host: &str,
    key: &PubKey<'_>,
) -> Result<KnownHostStatus> {
    let Some(snapshot) = snapshot else {
        return Ok(KnownHostStatus::Unknown);
    };
    let text = core::str::from_utf8(&snapshot.bytes).map_err(|_| "ssh: known_hosts is not UTF8")?;
    let mut found = false;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(hosts) = fields.next() else { continue };
        if !hosts.split(',').any(|candidate| candidate == host) {
            continue;
        }
        found = true;
        let (Some(algorithm), Some(base64)) = (fields.next(), fields.next()) else {
            continue;
        };
        if key
            .matches_openssh(&format!("{algorithm} {base64}"))
            .unwrap_or(false)
        {
            return Ok(KnownHostStatus::Match);
        }
    }
    Ok(if found {
        KnownHostStatus::Changed
    } else {
        KnownHostStatus::Unknown
    })
}
/// Called only after explicit confirmation or --accept-new. Reload at every CAS
/// attempt so a racing changed key can never be appended over and accepted.
pub async fn learn_known_host<S: Store>(store: &mut S, host: &str, key: &PubKey<'_>) -> Result<()> {
    let public = key
        .to_openssh()
        .map_err(|_| "ssh: cannot encode server public key")?;
    store
        .ensure_parent(KNOWN_HOSTS)
        .await
        .map_err(|e| e.to_string())?;
    for _ in 0..4 {
        let snapshot = store
            .snapshot(KNOWN_HOSTS)
            .await
            .map_err(|e| e.to_string())?;
        match known_host_status(snapshot.as_ref(), host, key)? {
            KnownHostStatus::Match => return Ok(()),
            KnownHostStatus::Changed => {
                return Err("ssh: HOST KEY CHANGED while saving trust; connection refused".into());
            }
            KnownHostStatus::Unknown => {}
        }
        let expected = snapshot.as_ref().map(Snapshot::digest);
        let mut text = String::new();
        if let Some(snapshot) = snapshot.as_ref() {
            text.push_str(
                core::str::from_utf8(&snapshot.bytes)
                    .map_err(|_| "ssh: known_hosts is not UTF8")?,
            );
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n')
            }
        }
        text.push_str(&format!("{host} {public}\n"));
        match store.publish(KNOWN_HOSTS, text.as_bytes(), expected).await {
            Ok(()) => return Ok(()),
            Err(StoreError::Conflict) => continue,
            Err(e) => return Err(format!("ssh: cannot durably save host trust: {e}")),
        }
    }
    Err("ssh: known_hosts kept changing; connection refused".into())
}
pub struct Identity {
    pub key: SignKey,
    pub created: bool,
}
pub async fn load_or_create_identity<S: Store>(store: &mut S, path: &str) -> Result<Identity> {
    for _ in 0..4 {
        if let Some(snapshot) = store.snapshot(path).await.map_err(|e| e.to_string())? {
            let key = SignKey::import_private(&snapshot.bytes)
                .map_err(|_| "ssh: identity is unsupported, encrypted, or invalid")?;
            return Ok(Identity {
                key,
                created: false,
            });
        }
        store.ensure_parent(path).await.map_err(|e| e.to_string())?;
        let key = SignKey::generate(KeyType::Ed25519, None)
            .map_err(|_| "ssh: cannot generate Ed25519 identity")?;
        let private = key
            .to_openssh_private()
            .map_err(|_| "ssh: cannot encode private identity")?;
        match store.publish(path, private.as_bytes(), None).await {
            Ok(()) => return Ok(Identity { key, created: true }),
            Err(StoreError::Conflict) => continue, // Load the durable winning identity.
            Err(e) => return Err(format!("ssh: cannot durably save new identity: {e}")),
        }
    }
    Err("ssh: identity kept changing; authentication refused".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    enum Race {
        Put(String, Vec<u8>),
        Fail,
    }
    #[derive(Default)]
    struct MemoryStore {
        files: BTreeMap<String, Vec<u8>>,
        races: VecDeque<Race>,
        writes: usize,
        fail_read: bool,
    }
    impl Store for MemoryStore {
        async fn snapshot(
            &mut self,
            path: &str,
        ) -> core::result::Result<Option<Snapshot>, StoreError> {
            if self.fail_read {
                return Err(StoreError::Other("injected read failure".into()));
            }
            Ok(self.files.get(path).map(|b| Snapshot {
                bytes: Zeroizing::new(b.clone()),
            }))
        }
        async fn ensure_parent(&mut self, _: &str) -> core::result::Result<(), StoreError> {
            Ok(())
        }
        async fn publish(
            &mut self,
            path: &str,
            bytes: &[u8],
            expected: Option<[u8; 32]>,
        ) -> core::result::Result<(), StoreError> {
            self.writes += 1;
            if let Some(race) = self.races.pop_front() {
                match race {
                    Race::Put(p, b) => {
                        self.files.insert(p, b);
                    }
                    Race::Fail => return Err(StoreError::Other("injected write failure".into())),
                }
            }
            match (self.files.get(path), expected) {
                (None, None) => {}
                (Some(old), Some(expected))
                    if <[u8; 32]>::from(Sha256::digest(old)) == expected => {}
                _ => return Err(StoreError::Conflict),
            }
            self.files.insert(path.into(), bytes.into());
            Ok(())
        }
    }
    fn key() -> SignKey {
        SignKey::generate(KeyType::Ed25519, None).unwrap()
    }
    fn entry(host: &str, key: &SignKey) -> Vec<u8> {
        format!("{host} {}\n", key.pubkey().to_openssh().unwrap()).into_bytes()
    }
    #[tokio::test]
    async fn known_hosts_accepts_match_and_refuses_changed_or_malformed_matching_entry() {
        let key = key();
        let foreign = key();
        let mut bytes = b"# keep comment\n".to_vec();
        bytes.extend(entry("other,host", &key));
        let snapshot = Snapshot {
            bytes: Zeroizing::new(bytes),
        };
        assert_eq!(
            known_host_status(Some(&snapshot), "host", &key.pubkey()).unwrap(),
            KnownHostStatus::Match
        );
        assert_eq!(
            known_host_status(Some(&snapshot), "host", &foreign.pubkey()).unwrap(),
            KnownHostStatus::Changed
        );
        assert_eq!(
            known_host_status(Some(&snapshot), "new", &key.pubkey()).unwrap(),
            KnownHostStatus::Unknown
        );
        let malformed = Snapshot {
            bytes: Zeroizing::new(b"host malformed key\n".to_vec()),
        };
        assert_eq!(
            known_host_status(Some(&malformed), "host", &key.pubkey()).unwrap(),
            KnownHostStatus::Changed
        );
    }
    #[tokio::test]
    async fn concurrent_unrelated_trust_append_retries_without_losing_existing_lines() {
        let key = key();
        let other = key();
        let original = entry("old", &other);
        let mut raced = original.clone();
        raced.extend(entry("racing", &other));
        let mut store = MemoryStore::default();
        store.files.insert(KNOWN_HOSTS.into(), original);
        store
            .races
            .push_back(Race::Put(KNOWN_HOSTS.into(), raced.clone()));
        learn_known_host(&mut store, "new", &key.pubkey())
            .await
            .unwrap();
        assert_eq!(store.writes, 2);
        let result = &store.files[KNOWN_HOSTS];
        assert!(result.starts_with(&raced));
        assert!(result.ends_with(&entry("new", &key)));
    }
    #[tokio::test]
    async fn racing_changed_host_is_not_overwritten_or_accepted() {
        let key = key();
        let other = key();
        let foreign = entry("host", &other);
        let mut store = MemoryStore::default();
        store
            .races
            .push_back(Race::Put(KNOWN_HOSTS.into(), foreign.clone()));
        assert!(
            learn_known_host(&mut store, "host", &key.pubkey())
                .await
                .is_err()
        );
        assert_eq!(store.files[KNOWN_HOSTS], foreign);
        assert_eq!(store.writes, 1);
    }
    #[tokio::test]
    async fn failed_trust_publication_and_read_error_fail_closed() {
        let key = key();
        let mut store = MemoryStore::default();
        store.races.push_back(Race::Fail);
        assert!(
            learn_known_host(&mut store, "host", &key.pubkey())
                .await
                .is_err()
        );
        assert!(!store.files.contains_key(KNOWN_HOSTS));
        store.fail_read = true;
        assert!(
            learn_known_host(&mut store, "host", &key.pubkey())
                .await
                .is_err()
        );
        assert_eq!(store.writes, 1);
    }
    #[tokio::test]
    async fn identity_create_race_loads_the_durable_winner() {
        let winner = key();
        let private = winner.to_openssh_private().unwrap();
        let mut store = MemoryStore::default();
        store
            .races
            .push_back(Race::Put("identity".into(), private.as_bytes().to_vec()));
        let identity = load_or_create_identity(&mut store, "identity")
            .await
            .unwrap();
        assert!(!identity.created);
        assert_eq!(
            identity.key.pubkey().to_openssh().unwrap(),
            winner.pubkey().to_openssh().unwrap()
        );
        assert_eq!(store.files["identity"], private.as_bytes());
    }
    #[tokio::test]
    async fn identity_write_failure_or_invalid_existing_file_never_overwrites_it() {
        let mut store = MemoryStore::default();
        store.races.push_back(Race::Fail);
        assert!(
            load_or_create_identity(&mut store, "identity")
                .await
                .is_err()
        );
        assert!(!store.files.contains_key("identity"));
        store
            .files
            .insert("identity".into(), b"invalid private key".to_vec());
        assert!(
            load_or_create_identity(&mut store, "identity")
                .await
                .is_err()
        );
        assert_eq!(store.files["identity"], b"invalid private key");
        assert_eq!(store.writes, 1);
    }
}

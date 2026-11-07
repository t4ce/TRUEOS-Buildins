//! Collections used by CSS parsing and transformation.
#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
pub use std::collections::{HashMap, HashSet};
#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub use hashbrown::{HashMap, HashSet};
#[cfg(not(all(feature = "trueos-native", target_os = "trueos")))]
/// Insertion-ordered map using the hosted process hasher.
pub type IndexMap<K, V> = indexmap::IndexMap<K, V, std::collections::hash_map::RandomState>;
/// Insertion-ordered map with an explicit native hasher.
#[cfg(all(feature = "trueos-native", target_os = "trueos"))]
pub type IndexMap<K, V> = indexmap::IndexMap<K, V, core::hash::BuildHasherDefault<rustc_hash::FxHasher>>;

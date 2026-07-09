use std::collections::{HashMap, HashSet};

pub(crate) type FastHashMap<K, V> = HashMap<K, V, ahash::RandomState>;
pub(crate) type FastHashSet<T> = HashSet<T, ahash::RandomState>;

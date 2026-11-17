pub(crate) fn ascii_case_hash(value: &str) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;

    value.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte.to_ascii_lowercase())).wrapping_mul(PRIME)
    })
}

pub(crate) fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let needle = needle.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle));
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

use std::{
    collections::{HashMap, HashSet},
    hash::{BuildHasherDefault, Hasher},
};

#[derive(Default)]
pub(crate) struct FastIntHasher(u64);

impl Hasher for FastIntHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        const OFFSET: u64 = 0xcbf29ce484222325;
        const PRIME: u64 = 0x100000001b3;
        self.0 = bytes.iter().fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        });
    }

    fn write_u32(&mut self, value: u32) {
        self.0 = mix_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = mix_u64(value);
    }
}

fn mix_u64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

pub(crate) type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FastIntHasher>>;
pub(crate) type FastHashSet<K> = HashSet<K, BuildHasherDefault<FastIntHasher>>;

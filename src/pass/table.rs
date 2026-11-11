//! A per-pass memo table that is cleared in O(1) and never frees.
//!
//! Open addressing with linear probing over `(node address, constraint key)`. Every slot is
//! stamped with the generation it was written in; clearing bumps the generation, which turns
//! every slot stale without touching it. After warm-up the table is a fixed allocation that a
//! pass only reads and writes, so the pass does not allocate.

use crate::state::ConstraintKey;
use alloc::vec::Vec;

#[derive(Copy, Clone)]
struct Slot<V: Copy> {
    generation: u32,
    addr: usize,
    key: ConstraintKey,
    value: V,
}

/// A memo of `V`s by node address and constraint key, valid for one pass.
pub(crate) struct PassTable<V: Copy> {
    slots: Vec<Slot<V>>,
    /// Live slots carry this generation; `0` is never live.
    generation: u32,
    len: usize,
}

impl<V: Copy + Default> Default for PassTable<V> {
    fn default() -> Self {
        PassTable {
            slots: Vec::new(),
            generation: 1,
            len: 0,
        }
    }
}

impl<V: Copy + Default> PassTable<V> {
    /// Forgets every entry without freeing anything.
    pub fn clear(&mut self) {
        self.len = 0;
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Wrapped: stamps from four billion passes ago could collide, so reset them all.
            for s in self.slots.iter_mut() {
                s.generation = 0;
            }
            self.generation = 1;
        }
    }

    #[inline]
    fn hash(addr: usize, key: &ConstraintKey) -> usize {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for w in [
            addr as u64,
            key.word(0) as u64,
            key.word(1) as u64,
            key.word(2) as u64,
            key.word(3) as u64,
            key.word(4) as u64,
        ] {
            h ^= w;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        (h ^ (h >> 32)) as usize
    }

    /// The value stored for `(addr, key)` in this pass, if any.
    pub fn get(&self, addr: usize, key: &ConstraintKey) -> Option<V> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut i = Self::hash(addr, key) & mask;
        loop {
            let s = &self.slots[i];
            if s.generation != self.generation {
                return None;
            }
            if s.addr == addr && s.key == *key {
                return Some(s.value);
            }
            i = (i + 1) & mask;
        }
    }

    /// Stores `value` for `(addr, key)`, growing (and so allocating) only when more than
    /// three quarters full.
    pub fn insert(&mut self, addr: usize, key: ConstraintKey, value: V) {
        if (self.len + 1) * 4 > self.slots.len() * 3 {
            self.grow();
        }
        let mask = self.slots.len() - 1;
        let mut i = Self::hash(addr, &key) & mask;
        loop {
            let s = &mut self.slots[i];
            if s.generation != self.generation {
                *s = Slot {
                    generation: self.generation,
                    addr,
                    key,
                    value,
                };
                self.len += 1;
                return;
            }
            if s.addr == addr && s.key == key {
                s.value = value;
                return;
            }
            i = (i + 1) & mask;
        }
    }

    fn grow(&mut self) {
        let new_len = (self.slots.len() * 2).max(64);
        let old = core::mem::replace(
            &mut self.slots,
            alloc::vec![Slot { generation: 0, addr: 0, key: ConstraintKey::default(), value: V::default() }; new_len],
        );
        let generation = self.generation;
        self.len = 0;
        for s in old.into_iter().filter(|s| s.generation == generation) {
            self.insert(s.addr, s.key, s.value);
        }
    }

    /// Slots allocated; for tests.
    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_hides_old_entries_and_insert_overwrites() {
        let mut t: PassTable<u32> = PassTable::default();
        let k = |i: u32| ConstraintKey::new([i, 0], [0, 0], 0);
        for i in 0..200 {
            t.insert(i as usize, k(i), i);
        }
        for i in 0..200 {
            assert_eq!(t.get(i as usize, &k(i)), Some(i));
        }
        t.insert(5, k(5), 500);
        assert_eq!(t.get(5, &k(5)), Some(500));
        let cap = t.capacity();
        t.clear();
        assert_eq!(t.get(5, &k(5)), None);
        assert_eq!(t.capacity(), cap, "clearing frees nothing");
        t.insert(7, k(7), 7);
        assert_eq!(t.get(7, &k(7)), Some(7));
        assert_eq!(t.get(5, &k(5)), None);
    }
}

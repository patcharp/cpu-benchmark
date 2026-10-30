//! Server-style workloads: hash map traffic, JSON (de)serialisation and
//! allocator churn.

use super::{CAT_DATA, Kernel, Metric, Worker, always_ok, no_backend};
use crate::stats::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::BuildHasherDefault;
use std::hint::black_box;

pub fn kernels() -> Vec<Kernel> {
    vec![
        Kernel {
            id: "data.hashmap",
            category: CAT_DATA,
            desc: "HashMap<u64,u64> (SipHash-1-3): 200k inserts with growth, 400k lookups (half miss)",
            unit: "Mop/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(MapOps { n: 200_000 }),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "data.json",
            category: CAT_DATA,
            desc: "serde_json parse + serialise of a ~1 MB array of nested records",
            unit: "MB/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: true,
            heavy: false,
            make: |_| Box::new(Json::new(4000)),
            check: always_ok,
            backend: no_backend,
        },
        Kernel {
            id: "data.alloc",
            category: CAT_DATA,
            desc: "Binary-tree build/free (malloc+free churn). Single-thread only: libc allocator locking would dominate a multi-thread run",
            unit: "Malloc/s",
            scale: 1e-6,
            metric: Metric::Rate,
            multi: false,
            heavy: false,
            make: |_| Box::new(Trees),
            check: always_ok,
            backend: no_backend,
        },
    ]
}

type Map = HashMap<u64, u64, BuildHasherDefault<DefaultHasher>>;

struct MapOps {
    n: usize,
}

impl Worker for MapOps {
    fn run(&mut self) -> u64 {
        let mut rng = Rng::new(0x4D41_5001);
        let mut keys = Vec::with_capacity(self.n);
        let mut map = Map::default();
        for i in 0..self.n {
            let k = rng.next_u64();
            keys.push(k);
            map.insert(k, i as u64);
        }
        let mut sum = 0u64;
        for &k in &keys {
            sum = sum.wrapping_add(*map.get(&black_box(k)).unwrap_or(&0));
            // A key that was (almost surely) never inserted: a miss.
            sum = sum.wrapping_add(*map.get(&black_box(!k)).unwrap_or(&1));
        }
        black_box(sum)
    }
    fn work(&self) -> f64 {
        (self.n * 3) as f64
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Geo {
    lat: f64,
    lon: f64,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Record {
    id: u64,
    name: String,
    email: String,
    active: bool,
    score: f64,
    tags: Vec<String>,
    location: Geo,
}

fn records(n: usize) -> Vec<Record> {
    let mut rng = Rng::new(0x4A53_0001);
    (0..n)
        .map(|i| Record {
            id: i as u64 * 7919 + rng.below(100),
            name: format!("user-{:08x}", rng.next_u64() as u32),
            email: format!("u{}@example-{}.test", rng.below(1_000_000), rng.below(500)),
            active: rng.below(2) == 0,
            score: rng.below(1_000_000) as f64 / 1000.0,
            tags: (0..rng.below(6)).map(|_| format!("tag{}", rng.below(300))).collect(),
            location: Geo {
                lat: rng.below(180_000) as f64 / 1000.0 - 90.0,
                lon: rng.below(360_000) as f64 / 1000.0 - 180.0,
            },
        })
        .collect()
}

struct Json {
    doc: Vec<u8>,
}

impl Json {
    fn new(n: usize) -> Self {
        Json { doc: serde_json::to_vec(&records(n)).unwrap() }
    }
}

impl Worker for Json {
    fn run(&mut self) -> u64 {
        let parsed: Vec<Record> = serde_json::from_slice(black_box(&self.doc)).unwrap();
        let out = serde_json::to_vec(black_box(&parsed)).unwrap();
        (parsed.len() as u64) << 32 ^ out.len() as u64
    }
    fn work(&self) -> f64 {
        (self.doc.len() * 2) as f64
    }
}

struct Node {
    l: Option<Box<Node>>,
    r: Option<Box<Node>>,
}

fn build(depth: u32) -> Box<Node> {
    if depth == 0 {
        Box::new(Node { l: None, r: None })
    } else {
        Box::new(Node { l: Some(build(depth - 1)), r: Some(build(depth - 1)) })
    }
}

fn count(n: &Node) -> u64 {
    1 + n.l.as_deref().map_or(0, count) + n.r.as_deref().map_or(0, count)
}

const DEPTHS: [u32; 3] = [4, 8, 12];

struct Trees;

impl Worker for Trees {
    fn run(&mut self) -> u64 {
        let mut total = 0;
        for d in DEPTHS {
            for _ in 0..(1usize << (17 - d)) {
                let tree = build(black_box(d));
                total += count(black_box(&tree));
            }
        }
        total
    }
    fn work(&self) -> f64 {
        DEPTHS.iter().map(|&d| (1usize << (17 - d)) * ((1usize << (d + 1)) - 1)).sum::<usize>() as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_is_lossless() {
        let recs = records(300);
        let doc = serde_json::to_vec(&recs).unwrap();
        let back: Vec<Record> = serde_json::from_slice(&doc).unwrap();
        assert_eq!(recs.len(), back.len());
        // serde_json's default float parser is not guaranteed bit-exact (1 ulp),
        // so compare floats with a tolerance and everything else exactly.
        for (a, b) in recs.iter().zip(&back) {
            assert_eq!((a.id, &a.name, &a.email, a.active, &a.tags), (b.id, &b.name, &b.email, b.active, &b.tags));
            assert!((a.score - b.score).abs() < 1e-9);
            assert!((a.location.lat - b.location.lat).abs() < 1e-9 && (a.location.lon - b.location.lon).abs() < 1e-9);
        }
        assert!(Json::new(4000).doc.len() > 700_000, "document should be ~1 MB");
    }

    #[test]
    fn trees_count_matches_work() {
        let mut t = Trees;
        assert_eq!(t.run() as f64, t.work());
    }

    #[test]
    fn hashmap_checksum_is_stable() {
        assert_eq!(MapOps { n: 5000 }.run(), MapOps { n: 5000 }.run());
    }
}

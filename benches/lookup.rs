use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use simonkv::KVStore;
use tempfile::NamedTempFile;

fn make_store() -> (NamedTempFile, KVStore) {
    let file = NamedTempFile::new().unwrap();
    let store = KVStore::open(file.path()).unwrap();
    (file, store)
}

fn make_populated_store() -> (NamedTempFile, KVStore) {
    let (file, mut store) = make_store();
    store.set("key".into(), "v".repeat(100)).unwrap();
    (file, store)
}

fn benchmark_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("operations");
    group.throughput(Throughput::Elements(1));

    group.bench_function("set", |b| {
        let (_file, mut store) = make_store();
        b.iter_batched(
            || (String::from("key"), "v".repeat(100)),
            |(key, value)| store.set(key, value).unwrap(),
            BatchSize::SmallInput,
        )
    });

    group.bench_function("get", |b| {
        let (_file, mut store) = make_populated_store();
        b.iter(|| black_box(store.get("key").unwrap()))
    });

    group.bench_function("delete", |b| {
        b.iter_batched(
            make_populated_store,
            |(file, mut store)| {
                let result = store.delete("key").unwrap();
                (file, store, result)
            },
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

criterion_group!(benches, benchmark_operations);
criterion_main!(benches);

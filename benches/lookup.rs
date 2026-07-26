use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use simonkv::KVStore;
use tempfile::NamedTempFile;

struct KVPair {
    key: String,
    value: String,
}

fn create_dataset(num_entries: usize) -> Vec<KVPair> {
    let mut dataset = Vec::with_capacity(num_entries);
    for i in 0..num_entries {
        let key = format!("{i}");
        let value = format!("String from {i}");
        dataset.push(KVPair { key, value })
    }
    dataset
}

fn kvstore_set_vector(pairs: Vec<KVPair>, kvstore: &mut KVStore) {
    for kvpair in pairs {
        kvstore.set(kvpair.key, kvpair.value).unwrap();
    }
}

fn create_benchmark_inputs(num_entries: usize) -> (NamedTempFile, KVStore, Vec<KVPair>) {
    let file = NamedTempFile::new().unwrap();
    let kvstore = KVStore::open(file.path()).unwrap();
    let dataset = create_dataset(num_entries);

    (file, kvstore, dataset)
}

fn benchmark_sets(c: &mut Criterion) {
    c.bench_function("10,000 sets", |b| {
        b.iter_batched(
            || create_benchmark_inputs(10000),
            |(_file, mut kvstore, dataset)| kvstore_set_vector(dataset, &mut kvstore),
            BatchSize::PerIteration,
        )
    });
}

criterion_group!(benches, benchmark_sets);
criterion_main!(benches);

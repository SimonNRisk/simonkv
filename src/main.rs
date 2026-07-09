use simonkv::KVStore;

fn main() {
    let mut kvstore = KVStore::new();
    kvstore.set(String::from("Test"), String::from("1"));
    let fetched_val = kvstore.get(String::from("Test"));
    println!("{fetched_val}");
}

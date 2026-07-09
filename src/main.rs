use simonkv::KVStore;

fn main() {
    let mut kvstore = KVStore::new();
    kvstore.set(String::from("Test"), String::from("1"));
    let fetched_val = kvstore.get("Test");
    match fetched_val {
        Some(val) => println!("Got {val}"),
        None => println!("No value")
    }
    let fetched_fake_val = kvstore.get("Fake");
    match fetched_fake_val {
        Some(val) => println!("Got {val}"),
        None => println!("No value")
    }
}

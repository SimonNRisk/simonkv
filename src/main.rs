use simonkv::KVStore;

fn main() {
    let mut kvstore = KVStore::open("simonkv.log").unwrap();

    kvstore.set(String::from("K"), String::from("V")).unwrap();

    let fetched_val = kvstore.get("K");
    match fetched_val {
        Some(val) => println!("Got {val}"),
        None => println!("No value"),
    }

    let fetched_fake_val = kvstore.get("Fake");
    match fetched_fake_val {
        Some(val) => println!("Got {val}"),
        None => println!("No value"),
    }

    let deleted_fetch_val = kvstore.delete("Test").unwrap();
    match deleted_fetch_val {
        Some(val) => println!("Successfully deleted {val}"),
        None => println!("Could not delete"),
    }
}

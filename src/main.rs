use simonkv::KVStore;

fn main() {
    /*
       Remember when inspecting logs, byte offsets include header and key
       Header = Operation (1 byte) + Key Length (2 bytes) + Value Length (4 bytes) = 7 bytes
    */

    let mut kvstore = KVStore::open("simonkv.log").unwrap();

    let k = String::from("K");
    let v = String::from("V");

    kvstore.set(k.clone(), v.clone()).unwrap();

    println!("Set {k} to {v} in keydir: {kvstore}");

    let a = String::from("A");
    let b = String::from("B");

    kvstore.set(a.clone(), b.clone()).unwrap();

    println!("Set {a} to {b} in keydir: {kvstore}");

    drop(kvstore);

    println!("Dropped kvstore");

    let mut kvstore = KVStore::open("simonkv.log").unwrap();

    let fetched_val = kvstore.get(&k).unwrap();
    match fetched_val {
        Some(val) => println!("Got {val} for {k}"),
        None => println!("No value"),
    }

    let fake = String::from("Fake");

    let fetched_fake_val = kvstore.get(&fake).unwrap();
    match fetched_fake_val {
        Some(val) => println!("Got {val}"), // wont run
        None => println!("No value for {fake} found"),
    }

    println!("Compacting...");

    kvstore.compact().unwrap();

    println!("Compacted kvstore: {kvstore}");
}

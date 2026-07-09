use std::collections::HashMap;

pub struct KVStore {
    map: HashMap<String, String>,
}

impl KVStore {
    pub fn new() -> Self {
        KVStore { map: HashMap::new() }
    }
    pub fn set(&mut self, key: String, value: String) {
        self.map.insert(key, value);
    }
    pub fn get(&self, key:&str) -> Option<&str> {
        self.map.get(key).map(|v| v.as_str())
    }
    pub fn delete(&mut self, key:&str) -> Option<String> {
        self.map.remove(key)
    }
}
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
    pub fn get(&self, key:String) -> String {
        match self.map.get(&key) {
            Some(val) => return val.clone(),
            None => return String::from("")
        }
    }
    pub fn delete(&mut self, key:String) {
        self.map.remove(&key);
    }
}
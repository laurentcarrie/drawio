use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub original: String,
    pub derived: Vec<Derived>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Derived {
    pub output: String,
    pub from: String,
    pub transforms: Vec<Transform>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Transform {
    Color { id: String, color: String },
}

//! JSON models reconstructed from shipped frontend consumers and Rust serde-name evidence.
//! Field types/default behavior are a new implementation; original Rust source is unavailable.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Material {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub size: u64,
    pub url: String,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Subtask {
    pub id: String,
    pub title: String,
    pub completed: bool,
    pub materials: Vec<Material>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: i64,
    pub area_id: Option<i64>,
    pub project_id: Option<i64>,
    pub title: String,
    pub description: String,
    pub description_doc: Option<Value>,
    pub url: String,
    pub due_date: Option<String>,
    pub completed: bool,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub subtasks: Vec<Subtask>,
    pub materials: Vec<Material>,
    pub sort_order: Option<i64>,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Area {
    pub id: i64,
    pub name: String,
    pub icon: String,
    pub color: String,
    pub created_at: String,
    pub materials: Vec<Material>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub id: i64,
    pub area_id: i64,
    pub name: String,
    pub created_at: String,
    pub materials: Vec<Material>,
}

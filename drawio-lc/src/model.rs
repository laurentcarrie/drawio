use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub original: String,
    pub derived: Vec<Derived>,
    /// Optional: if present, the generated slides are pushed to Confluence.
    pub confluence: Option<ConfluenceConfig>,
}

/// Confluence Cloud target.
/// Authentication is read from the `CONFLUENCE_USER` and `CONFLUENCE_TOKEN`
/// environment variables (user = Atlassian account email, token = API token).
#[derive(Debug, Serialize, Deserialize)]
pub struct ConfluenceConfig {
    /// Base URL of your Confluence Cloud instance, e.g. https://mycompany.atlassian.net
    pub url: String,
    /// Space key, e.g. "TEAM"
    pub space_key: String,
    /// Title of the page to create or update.
    pub page_title: String,
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
    Color { ids: Vec<String>, color: String },
    LayerVisibility {
        #[serde(default)]
        show: Vec<String>,
        #[serde(default)]
        hide: Vec<String>,
    },
    /// Recolor all edges that are NOT connected to any node in `exclude`.
    ColorEdges {
        exclude: Vec<String>,
        color: String,
    },
    /// Replace the display text of a cell identified by `id`.
    ReplaceText { id: String, text: String },
    /// Generate a title slide with centered text, same dimensions as other outputs.
    /// Must be the only transform in the list.
    TitleSlide { text: String },
}

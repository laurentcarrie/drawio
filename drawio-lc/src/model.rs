use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub derived: Vec<Derived>,
    /// Delay between slides in the animated GIF and MP4, in milliseconds.
    /// Defaults to 1000 ms (1 second) if not specified.
    #[serde(default = "default_delay_between_slides")]
    pub delay_between_slides: u32,
    /// Bottom margin applied to heading tags (h1–h6) in Markdown-rendered text,
    /// in pixels. Controls the gap between a title and the content that follows.
    /// Defaults to 4 px.
    #[serde(default = "default_heading_margin_bottom")]
    pub heading_margin_bottom: u32,
    /// Number of spacer units inserted between list items. Each unit adds
    /// ~10-12px of vertical gap. draw.io ignores all CSS on list elements,
    /// so spacing is achieved by replacing <ul>/<li> with plain bullet lines
    /// and injecting tiny <font> spacers. Defaults to 0 (tight list).
    #[serde(default = "default_list_item_spacing")]
    pub list_item_spacing: u32,
    /// Number of non-breaking spaces prepended before each bullet/number in
    /// list items, creating a left indent so items are not flush against the
    /// box edge. Defaults to 0 (no indent).
    #[serde(default = "default_list_item_indent")]
    pub list_item_indent: u32,
    /// Optional: if present, the generated slides are pushed to Confluence.
    pub confluence: Option<ConfluenceConfig>,
}

fn default_delay_between_slides() -> u32 {
    1000
}

fn default_heading_margin_bottom() -> u32 {
    4
}

fn default_list_item_spacing() -> u32 {
    0
}

fn default_list_item_indent() -> u32 {
    0
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
    /// Optional delay (in milliseconds) to display this slide in the animated
    /// GIF/MP4.  Overrides `delay_between_slides` for this specific slide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Transform {
    /// Recolor cells. Each entry in `tags` is a draw.io tag.
    Color { tags: Vec<String>, color: String },
    /// Recolor all edges that are NOT connected to any node whose tag is in `exclude`.
    ColorEdges {
        exclude: Vec<String>,
        color: String,
    },
    /// Replace the display text of a cell. `tag` is a draw.io tag.
    ReplaceText {
        tag: String,
        text: String,
    },
    /// Show or hide cells by draw.io tag.
    ElementVisibility {
        #[serde(default)]
        show: Vec<String>,
        #[serde(default)]
        hide: Vec<String>,
    },
    /// Generate a title slide with centered text, same dimensions as other outputs.
    /// Must be the only transform in the list.
    TitleSlide { text: String },
    /// Replace the display text of a cell with the contents of a Markdown file.
    /// `tag` is a draw.io tag. `file` is the path to the `.md` file,
    /// relative to the directory containing the config file.
    ImportMarkdown {
        tag: String,
        file: String,
    },
    /// Animation marker: splits the transform list into sections.
    /// Each section produces an animated GIF (`stepX.sectionN.gif`) with one
    /// frame per intermediate state (state before section + one frame per
    /// transform in the section).  Does not modify the draw.io XML.
    Animation,
    /// Show or hide the begin/end arrows of edge cells by draw.io tag.
    /// `tags` selects the target edges. `begin` and `end` control whether
    /// the corresponding arrowhead is shown (`true`) or hidden (`false`).
    /// Omitting a field leaves the corresponding arrowhead unchanged.
    ArrowVisibility {
        tags: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        begin: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end: Option<bool>,
    },
}

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
    /// Transition effect applied between slides in the MP4 output.
    /// Supported values: "dissolve". Omit for a hard cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<String>,
    /// Duration of the transition effect in milliseconds. Only used when
    /// `transition` is set. Defaults to 500 ms.
    #[serde(default = "default_transition_duration_ms")]
    pub transition_duration_ms: u32,
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

fn default_transition_duration_ms() -> u32 {
    500
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
    /// Source file or the output name of a previous step to use as input.
    /// If omitted, defaults to the `output` of the immediately preceding
    /// non-TitleSlide step.  The first step must always provide `from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub transforms: Vec<Transform>,
    /// If present, the exported PNG is cropped to the bounding box of the
    /// draw.io cell whose tag matches this value (instead of the full canvas).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounding_box_tag: Option<String>,
    /// Optional delay (in milliseconds) to display this slide in the animated
    /// GIF/MP4.  Overrides `delay_between_slides` for this specific slide.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<u32>,
}

/// Border/stroke line style for shape cells.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StrokeStyle {
    /// Continuous line (draw.io: `dashed=0`).
    Solid,
    /// Dashed line (draw.io: `dashed=1`).
    Dashed,
    /// Dotted line (draw.io: `dashed=1; dashPattern=1 4`).
    Dotted,
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
    /// Change visual attributes of shape (non-edge) cells selected by tag.
    /// All fields except `tags` are optional; omitted fields are left unchanged.
    ShapeAttributes {
        tags: Vec<String>,
        /// draw.io shape name, e.g. "rhombus", "ellipse", "hexagon".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shape: Option<String>,
        /// Fill color as a CSS hex string, e.g. "#DDDDDD".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill_color: Option<String>,
        /// Stroke (border) color as a CSS hex string.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke_color: Option<String>,
        /// Border line style: "solid", "dashed", or "dotted".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stroke_style: Option<StrokeStyle>,
        /// Replace the cell label with this text (Markdown supported).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Font size in points.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        font_size: Option<u32>,
    },
    /// Change visual attributes of edge cells selected by tag.
    /// All fields except `tags` are optional; omitted fields are left unchanged.
    EdgeAttributes {
        tags: Vec<String>,
        /// Replace the edge label with this text (Markdown supported).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Stroke color as a CSS hex string or named color, e.g. "red".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Line style: "dashed", "dotted", or "solid".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_style: Option<String>,
        /// Stroke thickness in pixels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thickness: Option<f64>,
        /// Font color for the label text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        font_color: Option<String>,
        /// Font size in points.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        font_size: Option<u32>,
        /// Background color of the label box.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_background: Option<String>,
        /// Border color of the label box.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text_border_color: Option<String>,
        /// Start arrowhead type, e.g. "none", "classic", "open".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start_arrow: Option<String>,
        /// End arrowhead type, e.g. "none", "classic", "open".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_arrow: Option<String>,
    },
    /// Embed an external image file into an image-placeholder cell.
    /// The cell is selected by `tag`; its style is updated to use a base64-
    /// encoded data URI, and the geometry is resized to `width` × `height`.
    EmbedImage {
        /// draw.io tag of the target image cell.
        tag: String,
        /// Path to the image file, relative to the config directory.
        file: String,
        /// Desired display width in pixels.  Height is scaled proportionally.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f64>,
        /// Desired display height in pixels.  Width is scaled proportionally.
        /// If both `width` and `height` are given, both are used as-is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<f64>,
    },
}

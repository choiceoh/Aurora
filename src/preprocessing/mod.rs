pub mod html_preprocess;
pub mod telegram;
pub mod web_content;

pub use html_preprocess::{preprocess_html, PreprocessResult, HtmlMetadata, QualitySignals};
pub use telegram::format_tables_for_telegram;
pub use web_content::{process_web_content, WebContentConfig, WebContentResult};

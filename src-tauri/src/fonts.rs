use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    pub name: String,
    pub family_name: String,
    pub post_script_name: String,
    pub weight: String,
    pub style: String,
    pub width: String,
    pub monospace: bool,
}

pub fn list_system_fonts() -> Vec<FontInfo> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let mut seen = HashSet::new();
    let mut fonts = Vec::new();

    for face in db.faces() {
        let Some((family_name, _)) = face.families.first() else {
            continue;
        };
        let family_trimmed = family_name.trim();
        if family_trimmed.is_empty() || family_trimmed.starts_with('.') {
            continue;
        }
        let lower = family_trimmed.to_ascii_lowercase();
        if !seen.insert(lower) {
            continue;
        }
        let style_str = match face.style {
            fontdb::Style::Normal => "normal",
            fontdb::Style::Italic => "italic",
            fontdb::Style::Oblique => "oblique",
        };
        fonts.push(FontInfo {
            name: family_trimmed.to_string(),
            family_name: family_trimmed.to_string(),
            post_script_name: face.post_script_name.clone(),
            weight: face.weight.0.to_string(),
            style: style_str.to_string(),
            width: format!("{:?}", face.stretch).to_ascii_lowercase(),
            monospace: face.monospaced,
        });
    }

    fonts.sort_by(|a, b| {
        a.family_name
            .to_ascii_lowercase()
            .cmp(&b.family_name.to_ascii_lowercase())
    });
    fonts
}

#[tauri::command]
pub async fn get_fonts() -> Result<Vec<FontInfo>, String> {
    tokio::task::spawn_blocking(list_system_fonts)
        .await
        .map_err(|e| e.to_string())
}

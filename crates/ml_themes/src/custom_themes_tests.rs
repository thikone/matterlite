// SPDX-License-Identifier: AGPL-3.0-only

use std::fs;

use super::*;

const THEME_BODY: &str = "\
accent: '#01a0e4'
background: '#090300'
details: darker
foreground: '#a5a2a2'
terminal_colors:
  bright:
    black: '#5c5855'
    blue: '#807d7c'
    cyan: '#cdab53'
    green: '#3a3432'
    magenta: '#d6d5d4'
    red: '#e8bbd0'
    white: '#f7f7f7'
    yellow: '#4a4543'
  normal:
    black: '#090300'
    blue: '#01a0e4'
    cyan: '#b5e4f4'
    green: '#01a252'
    magenta: '#a16a94'
    red: '#db2d20'
    white: '#a5a2a2'
    yellow: '#fded02'
";

fn names(themes: &[(ThemeKind, WarpTheme)]) -> Vec<String> {
    let mut names: Vec<String> = themes.iter().map(|(kind, _)| kind.to_string()).collect();
    names.sort();
    names
}

#[test]
fn file_names_become_title_case_names() {
    assert_eq!(
        file_name_to_theme_name("solarized_dark.yaml"),
        "Solarized Dark"
    );
    assert_eq!(
        file_name_to_theme_name("SOLARIZED_DARK.yml"),
        "Solarized Dark"
    );
    assert_eq!(file_name_to_theme_name("Nord.yaml"), "Nord");
}

#[test]
fn missing_directory_has_no_custom_themes() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_custom_themes(&dir.path().join("themes")).is_empty());
}

#[test]
fn loads_yaml_themes_and_skips_everything_else() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("named.yaml"),
        format!("{THEME_BODY}name: My Theme\n"),
    )
    .unwrap();
    fs::create_dir(dir.path().join("more")).unwrap();
    fs::write(dir.path().join("more").join("deep_sea.yml"), THEME_BODY).unwrap();
    fs::write(dir.path().join("broken.yaml"), "accent: [not a colour").unwrap();
    fs::write(dir.path().join("notes.txt"), THEME_BODY).unwrap();
    fs::write(dir.path().join(".yaml"), THEME_BODY).unwrap();

    let themes = load_custom_themes(dir.path());

    assert_eq!(names(&themes), ["Deep Sea", "My Theme"]);
    for (kind, theme) in &themes {
        assert!(matches!(kind, ThemeKind::Custom(_)));
        assert_eq!(theme.name(), Some(kind.to_string()));
    }
}

#[test]
fn custom_themes_join_the_built_in_ones() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("base16_ocean.yaml"), THEME_BODY).unwrap();

    let config = load_theme_configs(dir.path());
    let custom: Vec<_> = config
        .theme_items()
        .filter(|(kind, _)| matches!(kind, ThemeKind::CustomBase16(_)))
        .collect();

    assert_eq!(custom.len(), 1);
    assert_eq!(custom[0].0.to_string(), "Base16 Ocean");
    assert!(
        config
            .theme_items()
            .any(|(kind, _)| *kind == ThemeKind::Dracula)
    );
}

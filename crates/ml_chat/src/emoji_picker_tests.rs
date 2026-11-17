// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

fn names(section: &Section) -> Vec<&str> {
    section
        .emojis
        .iter()
        .map(|emoji| emoji.name.as_str())
        .collect()
}

#[test]
fn without_a_query_recent_categories_and_custom() {
    let recent = [
        "+1".to_owned(),
        "party_parrot".to_owned(),
        "gone".to_owned(),
    ];
    let custom = ["party_parrot".to_owned()];
    let shown = sections("", &recent, &custom, None);
    let labels: Vec<&str> = shown.iter().map(|section| section.label).collect();
    assert_eq!(
        labels,
        [
            "Recently Used",
            "Smileys & Emotion",
            "People & Body",
            "Animals & Nature",
            "Food & Drink",
            "Travel & Places",
            "Activities",
            "Objects",
            "Symbols",
            "Flags",
            "Custom",
        ]
    );
    // Unknown names are left out.
    assert_eq!(names(&shown[0]), ["+1", "party_parrot"]);
    assert_eq!(names(&shown[1])[0], "grinning");
    // No skin-tone variants or components.
    assert!(
        shown
            .iter()
            .flat_map(|section| names(section))
            .all(|name| !name.contains("skin_tone"))
    );
    assert!(names(&shown[2]).contains(&"wave"));
    // Without recent emoji there is no such section.
    assert_eq!(sections("", &[], &[], None)[0].label, "Smileys & Emotion");
}

#[test]
fn a_skin_tone_replaces_emoji_that_have_one() {
    let shown = sections("", &["wave".to_owned()], &[], Some("1F3FD"));
    assert_eq!(names(&shown[0]), ["wave_medium_skin_tone"]);
    let people = names(&shown[2]);
    assert!(people.contains(&"point_up_medium_skin_tone"));
    assert!(!people.contains(&"wave"));
    // Emoji without variants stay.
    assert!(names(&shown[1]).contains(&"grinning"));
}

#[test]
fn search_puts_recent_then_prefix_matches_first() {
    let custom = ["catjam".to_owned(), "Smile_Big".to_owned()];
    let recent = ["joy_cat".to_owned()];
    let shown = sections(":cat", &recent, &custom, None);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].label, "Search Results");
    let found = names(&shown[0]);
    assert_eq!(found[0], "joy_cat");
    // Then names starting with the query, in name order, then the others.
    assert_eq!(&found[1..4], ["cat", "cat2", "catjam"]);
    assert!(found[4..].iter().all(|name| !name.starts_with("cat")));
    assert!(found.contains(&"black_cat"));
    // Case-insensitive, custom emoji too.
    assert_eq!(
        names(&sections("SMILE_B", &[], &custom, None)[0]),
        ["Smile_Big"]
    );
    assert!(sections("zzzz", &[], &[], None)[0].emojis.is_empty());
}

#[test]
fn the_grid_has_a_header_and_rows_of_nine_per_section() {
    let emoji = |name: &str| PickerEmoji {
        name: name.to_owned(),
        emoji: EmojiRef::Custom {
            name: name.to_owned(),
        },
    };
    let sections = [
        Section {
            label: "A",
            icon: None,
            emojis: (0..10).map(|i| emoji(&format!("a{i}"))).collect(),
        },
        Section {
            label: "B",
            icon: None,
            emojis: vec![emoji("b")],
        },
    ];
    let (flat, rows, headers) = grid(&sections);
    assert_eq!(flat.len(), 11);
    assert_eq!(
        rows,
        [
            GridRow::Header("A"),
            GridRow::Emojis(0..9),
            GridRow::Emojis(9..10),
            GridRow::Header("B"),
            GridRow::Emojis(10..11),
        ]
    );
    assert_eq!(headers, [0, 3]);
    assert_eq!(row_of(&rows, 9), Some(2));
    assert_eq!(row_of(&rows, 10), Some(4));
}

#[test]
fn up_and_down_keep_the_column_across_sections() {
    let rows = [
        GridRow::Header("Recent"),
        GridRow::Emojis(0..2),
        GridRow::Header("A"),
        GridRow::Emojis(2..11),
        GridRow::Emojis(11..14),
    ];
    // Column 1 of the recent row to column 1 of the next section.
    assert_eq!(vertical_step(&rows, 1, true), 3);
    // Column 7 to a shorter row: its last emoji.
    assert_eq!(vertical_step(&rows, 9, true), 13);
    assert_eq!(vertical_step(&rows, 13, false), 4);
    assert_eq!(vertical_step(&rows, 8, false), 1);
    // At the ends nothing moves.
    assert_eq!(vertical_step(&rows, 0, false), 0);
    assert_eq!(vertical_step(&rows, 12, true), 12);
}

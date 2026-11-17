// SPDX-License-Identifier: AGPL-3.0-only

//! The emoji picker (the webapp's `components/emoji_picker`): a search field, the categories
//! as tabs, the emoji in a grid (Recently Used, the system categories, then the server's
//! custom emoji) and the selected emoji's name. Up/Down move through the grid from the search
//! field, Enter picks, Esc clears the search and then closes. Picking emits the emoji's name;
//! the view that opened the picker does the reacting.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::LazyLock;

use ml_editor::NavigationKey;
use ml_workspace::{SearchBarEvent, SearchBarOptions, SearchBarView};
use mm_emoji::Emoji;
use mm_model::EmojiId;
use mm_render::EmojiRef;
use mm_sync::{Command, ImageKey};
use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Empty,
    Flex, Hoverable, MainAxisSize, MouseStateHandle, ParentElement, Radius, ScrollStateHandle,
    Scrollable, ScrollableElement as _, Text, UniformList, UniformListState,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent as _;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::chats_view::fade;
use crate::message::{CustomEmoji, emoji_image};
use crate::session::{Session, SessionEvent};

/// Emoji per grid row (`EMOJI_PER_ROW`).
pub const PER_ROW: usize = 9;
/// A grid cell, and the height of every grid row (headers too).
const CELL: f32 = 36.;
const EMOJI_SIZE: f32 = 24.;
const VISIBLE_ROWS: usize = 8;
const PADDING: f32 = 8.;
const TAB: f32 = 28.;
/// Room for the grid's scrollbar beside the last column.
const SCROLLBAR: f32 = 14.;
const WIDTH: f32 = PER_ROW as f32 * CELL + SCROLLBAR + 2. * PADDING;

/// An emoji as the picker offers it: the name it is reacted with and how to draw it.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerEmoji {
    pub name: String,
    pub emoji: EmojiRef,
}

/// A group of the grid: a category, Recently Used, Custom, or the search results.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub label: &'static str,
    /// The tab's emoji.
    pub icon: Option<EmojiRef>,
    pub emojis: Vec<PickerEmoji>,
}

/// The system categories in the webapp's order, with the emoji drawn on their tabs.
const CATEGORIES: [(&str, &str, &str); 9] = [
    ("smileys-emotion", "Smileys & Emotion", "grinning"),
    ("people-body", "People & Body", "wave"),
    ("animals-nature", "Animals & Nature", "dog"),
    ("food-drink", "Food & Drink", "apple"),
    ("travel-places", "Travel & Places", "airplane"),
    ("activities", "Activities", "soccer"),
    ("objects", "Objects", "bulb"),
    ("symbols", "Symbols", "heart"),
    ("flags", "Flags", "checkered_flag"),
];

static BY_UNIFIED: LazyLock<HashMap<&'static str, &'static Emoji>> = LazyLock::new(|| {
    mm_emoji::all()
        .iter()
        .map(|emoji| (emoji.unified.as_str(), emoji))
        .collect()
});

fn system(emoji: &'static Emoji) -> PickerEmoji {
    PickerEmoji {
        name: emoji.name().to_owned(),
        emoji: EmojiRef::System {
            name: emoji.name().to_owned(),
            image: emoji.image(),
        },
    }
}

fn custom(name: &str) -> PickerEmoji {
    PickerEmoji {
        name: name.to_owned(),
        emoji: EmojiRef::Custom {
            name: name.to_owned(),
        },
    }
}

fn icon(name: &str) -> Option<EmojiRef> {
    mm_emoji::by_name(name).map(|emoji| system(emoji).emoji)
}

/// An emoji in my skin tone, if it has a variant in it (`convertEmojiSkinTone`).
fn in_tone(emoji: &'static Emoji, tone: Option<&str>) -> &'static Emoji {
    tone.filter(|_| emoji.skin_tone.is_none())
        .and_then(|tone| {
            let base = emoji.unified.trim_end_matches("-FE0F");
            BY_UNIFIED.get(format!("{base}-{tone}").as_str()).copied()
        })
        .unwrap_or(emoji)
}

/// The system emoji offered, in the webapp's order: no skin-tone variants or components,
/// and, with a tone, an emoji's variant in it instead of the emoji (`emojiMatchesSkin`).
pub fn system_emojis(tone: Option<&str>) -> Vec<&'static Emoji> {
    mm_emoji::all()
        .iter()
        .filter(|emoji| emoji.skin_tone.is_none() && emoji.category != "component")
        .map(|emoji| in_tone(emoji, tone))
        .collect()
}

/// The sections to show: without a query, Recently Used (when there are any), the
/// categories and Custom; with one, the emoji with a name containing it (`getFilteredEmojis`),
/// recently used ones first and in each part those starting with it first (`compareEmojis`).
/// `custom` is the server's custom emoji, by name.
pub fn sections(
    query: &str,
    recent: &[String],
    custom: &[String],
    tone: Option<&str>,
) -> Vec<Section> {
    let query = query.trim().trim_matches(':').to_lowercase();
    let known_custom: HashSet<&str> = custom.iter().map(String::as_str).collect();
    let resolve = |name: &str| -> Option<PickerEmoji> {
        match mm_emoji::by_name(name) {
            Some(emoji) => Some(system(in_tone(emoji, tone))),
            None => known_custom.contains(name).then(|| self::custom(name)),
        }
    };
    if query.is_empty() {
        let mut sections = Vec::new();
        let mut seen = HashSet::new();
        let recent: Vec<PickerEmoji> = recent
            .iter()
            .filter_map(|name| resolve(name))
            .filter(|emoji| seen.insert(emoji.name.clone()))
            .collect();
        if !recent.is_empty() {
            sections.push(Section {
                label: "Recently Used",
                icon: icon("clock3"),
                emojis: recent,
            });
        }
        let all = system_emojis(tone);
        for (id, label, tab) in CATEGORIES {
            sections.push(Section {
                label,
                icon: icon(tab),
                emojis: all
                    .iter()
                    .filter(|emoji| emoji.category == id)
                    .map(|emoji| system(emoji))
                    .collect(),
            });
        }
        if !custom.is_empty() {
            sections.push(Section {
                label: "Custom",
                icon: Some(EmojiRef::Custom {
                    name: custom[0].clone(),
                }),
                emojis: custom.iter().map(|name| self::custom(name)).collect(),
            });
        }
        return sections;
    }
    // (matched name, emoji) of everything containing the query.
    let mut found: Vec<(String, PickerEmoji)> = Vec::new();
    for emoji in system_emojis(tone) {
        if let Some(alias) = emoji
            .short_names
            .iter()
            .find(|alias| alias.contains(&query))
        {
            found.push((alias.clone(), system(emoji)));
        }
    }
    for name in custom {
        if name.to_lowercase().contains(&query) {
            found.push((name.to_lowercase(), self::custom(name)));
        }
    }
    let recent: HashSet<String> = recent
        .iter()
        .filter_map(|name| resolve(name))
        .map(|emoji| emoji.name)
        .collect();
    found.sort_by(|(a_name, a), (b_name, b)| {
        let key = |name: &String, emoji: &PickerEmoji| {
            (
                !recent.contains(&emoji.name),
                !name.starts_with(&query),
                name.clone(),
            )
        };
        key(a_name, a).cmp(&key(b_name, b))
    });
    vec![Section {
        label: "Search Results",
        icon: None,
        emojis: found.into_iter().map(|(_, emoji)| emoji).collect(),
    }]
}

/// One row of the grid.
#[derive(Clone, Debug, PartialEq)]
pub enum GridRow {
    Header(&'static str),
    /// Indices into the flat list of emoji.
    Emojis(Range<usize>),
}

/// The grid of sections: each a header, then rows of [`PER_ROW`]. Also the flat list of
/// emoji and the row of each section's header.
pub fn grid(sections: &[Section]) -> (Vec<PickerEmoji>, Vec<GridRow>, Vec<usize>) {
    let mut flat = Vec::new();
    let mut rows = Vec::new();
    let mut headers = Vec::new();
    for section in sections {
        headers.push(rows.len());
        rows.push(GridRow::Header(section.label));
        for chunk in section.emojis.chunks(PER_ROW) {
            let start = flat.len();
            flat.extend(chunk.iter().cloned());
            rows.push(GridRow::Emojis(start..flat.len()));
        }
    }
    (flat, rows, headers)
}

/// The row showing emoji `index`.
fn row_of(rows: &[GridRow], index: usize) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, GridRow::Emojis(range) if range.contains(&index)))
}

/// The emoji above or below `index` in the grid: the same column of the next row of emoji
/// (across section headers), or the last of a shorter row; `index` at either end.
pub fn vertical_step(rows: &[GridRow], index: usize, down: bool) -> usize {
    let Some(row) = row_of(rows, index) else {
        return index;
    };
    let GridRow::Emojis(current) = &rows[row] else {
        return index;
    };
    let column = index - current.start;
    let mut others: Box<dyn Iterator<Item = &GridRow>> = if down {
        Box::new(rows[row + 1..].iter())
    } else {
        Box::new(rows[..row].iter().rev())
    };
    others
        .find_map(|row| match row {
            GridRow::Emojis(range) => Some((range.start + column).min(range.end - 1)),
            GridRow::Header(_) => None,
        })
        .unwrap_or(index)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerEvent {
    Picked(String),
    /// Esc on an empty search.
    Closed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PickerAction {
    Pick(usize),
    Hover(usize),
    /// A category tab: scroll to its section.
    Section(usize),
}

pub struct EmojiPickerView {
    search: ViewHandle<SearchBarView>,
    sections: Vec<Section>,
    flat: Rc<Vec<PickerEmoji>>,
    rows: Rc<Vec<GridRow>>,
    headers: Vec<usize>,
    selected: usize,
    searching: bool,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    cell_states: Rc<RefCell<HashMap<usize, MouseStateHandle>>>,
    tab_states: Vec<MouseStateHandle>,
    recent: Vec<String>,
    custom: Vec<String>,
    tone: Option<String>,
    /// Custom emoji whose images are on disk, and the ids of all of them.
    custom_paths: Rc<CustomEmoji>,
    custom_ids: HashMap<EmojiId, String>,
    account_dir: Option<PathBuf>,
}

impl EmojiPickerView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let search = ctx.add_typed_action_view(|ctx| {
            SearchBarView::new(
                SearchBarOptions {
                    placeholder: "Search emojis".into(),
                    query_options: false,
                    toggle_groups: Vec::new(),
                },
                ctx,
            )
        });
        ctx.subscribe_to_view(&search, |me, _, event, ctx| match event {
            SearchBarEvent::QueryChanged | SearchBarEvent::Cleared => me.rebuild(ctx),
            SearchBarEvent::Submit => me.pick(me.selected, ctx),
            SearchBarEvent::Dismissed => ctx.emit(PickerEvent::Closed),
            SearchBarEvent::Navigate(NavigationKey::Up) => me.step(false, ctx),
            SearchBarEvent::Navigate(NavigationKey::Down) => me.step(true, ctx),
            _ => {}
        });
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| {
            if let SessionEvent::ImageReady(ImageKey::Emoji(id)) = event
                && let (Some(name), Some(dir)) = (me.custom_ids.get(id), &me.account_dir)
            {
                let path = ImageKey::Emoji(id.clone()).path(dir);
                Rc::make_mut(&mut me.custom_paths).insert(name.clone(), path);
                ctx.notify();
            }
        });
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        Self {
            search,
            sections: Vec::new(),
            flat: Rc::new(Vec::new()),
            rows: Rc::new(Vec::new()),
            headers: Vec::new(),
            selected: 0,
            searching: false,
            list_state: UniformListState::new(),
            scroll_state: ScrollStateHandle::default(),
            cell_states: Rc::new(RefCell::new(HashMap::new())),
            tab_states: Vec::new(),
            recent: Vec::new(),
            custom: Vec::new(),
            tone: None,
            custom_paths: Rc::new(CustomEmoji::new()),
            custom_ids: HashMap::new(),
            account_dir: None,
        }
    }

    /// Readies the picker to show: an empty search, my recent emoji and skin tone, the
    /// server's custom emoji (asking for the images not on disk yet).
    pub fn open(&mut self, ctx: &mut ViewContext<Self>) {
        let session = Session::as_ref(ctx);
        self.account_dir = session
            .active()
            .map(|active| active.account_dir().to_owned());
        let Some(store) = session.store().cloned() else {
            return;
        };
        let mut wanted = Vec::new();
        {
            let Ok(store) = store.read() else { return };
            self.recent = store.recent_emojis();
            self.tone = store.skin_tone().map(str::to_owned);
            let mut custom: Vec<(&String, &EmojiId)> = store
                .emoji
                .values()
                .filter(|emoji| emoji.delete_at == 0)
                .map(|emoji| (&emoji.name, &emoji.id))
                .collect();
            custom.sort();
            self.custom = custom.iter().map(|(name, _)| (*name).clone()).collect();
            self.custom_ids = custom
                .iter()
                .map(|(name, id)| ((*id).clone(), (*name).clone()))
                .collect();
            let mut paths = CustomEmoji::new();
            for (name, id) in custom {
                let key = ImageKey::Emoji(id.clone());
                match &self.account_dir {
                    Some(dir) if key.path(dir).is_file() => {
                        paths.insert(name.clone(), key.path(dir));
                    }
                    Some(_) => wanted.push(key),
                    None => {}
                }
            }
            self.custom_paths = Rc::new(paths);
        }
        for key in wanted {
            Session::as_ref(ctx).send(Command::FetchImage(key));
        }
        self.search.update(ctx, |search, ctx| search.clear(ctx));
        self.rebuild(ctx);
    }

    fn rebuild(&mut self, ctx: &mut ViewContext<Self>) {
        let query = self.search.as_ref(ctx).query(ctx);
        self.searching = !query.trim().trim_matches(':').is_empty();
        self.sections = sections(&query, &self.recent, &self.custom, self.tone.as_deref());
        let (flat, rows, headers) = grid(&self.sections);
        self.flat = Rc::new(flat);
        self.rows = Rc::new(rows);
        self.headers = headers;
        self.tab_states = self
            .sections
            .iter()
            .map(|_| MouseStateHandle::default())
            .collect();
        self.cell_states.borrow_mut().clear();
        self.selected = 0;
        self.scroll_to_row(0);
        ctx.notify();
    }

    /// Puts row `row` at the top.
    fn scroll_to_row(&self, row: usize) {
        let top = self.list_state.scroll_top().as_f64() as f32;
        self.list_state.add_scroll_top(row as f32 - top);
    }

    fn step(&mut self, down: bool, ctx: &mut ViewContext<Self>) {
        if self.flat.is_empty() {
            return;
        }
        self.selected = vertical_step(&self.rows, self.selected, down);
        if let Some(row) = row_of(&self.rows, self.selected) {
            // The header above the first row comes into view with it.
            let first = if row == 1 { 0 } else { row };
            self.list_state.scroll_to(first);
        }
        ctx.notify();
    }

    fn pick(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if let Some(emoji) = self.flat.get(index) {
            ctx.emit(PickerEvent::Picked(emoji.name.clone()));
        }
    }

    fn render_tabs(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = theme.main_text_color(theme.surface_1()).into_solid();
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_spacing(2.);
        for (index, section) in self.sections.iter().enumerate() {
            let Some(icon) = section.icon.clone() else {
                continue;
            };
            let Some(state) = self.tab_states.get(index).cloned() else {
                continue;
            };
            let custom = self.custom_paths.clone();
            let label = section.label;
            let tooltip = appearance
                .ui_builder()
                .tool_tip(label.to_owned())
                .build()
                .finish();
            let mut tooltip = Some(tooltip);
            row.add_child(
                Hoverable::new(state, move |state| {
                    let image = emoji_image(&icon, &custom)
                        .map(|image| {
                            ConstrainedBox::new(image)
                                .with_width(18.)
                                .with_height(18.)
                                .finish()
                        })
                        .unwrap_or_else(|| Empty::new().finish());
                    let mut cell = Container::new(Align::new(image).finish())
                        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)));
                    if state.is_hovered() {
                        cell = cell.with_background_color(fade(text, 0.1));
                    }
                    let cell = ConstrainedBox::new(cell.finish())
                        .with_width(TAB)
                        .with_height(TAB)
                        .finish();
                    match (state.is_hovered(), tooltip.take()) {
                        (true, Some(tooltip)) => {
                            let mut stack = warpui::elements::Stack::new().with_child(cell);
                            stack.add_positioned_overlay_child(
                                tooltip,
                                warpui::elements::OffsetPositioning::offset_from_parent(
                                    pathfinder_geometry::vector::vec2f(0., -4.),
                                    warpui::elements::ParentOffsetBounds::WindowByPosition,
                                    warpui::elements::ParentAnchor::TopMiddle,
                                    warpui::elements::ChildAnchor::BottomMiddle,
                                ),
                            );
                            stack.finish()
                        }
                        _ => cell,
                    }
                })
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| ctx.dispatch_typed_action(PickerAction::Section(index)))
                .finish(),
            );
        }
        row.finish()
    }

    fn render_grid(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = theme.main_text_color(theme.surface_1()).into_solid();
        let muted = theme.sub_text_color(theme.surface_1()).into_solid();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let flat = self.flat.clone();
        let rows = self.rows.clone();
        let states = self.cell_states.clone();
        let custom = self.custom_paths.clone();
        let selected = self.selected;
        let list = UniformList::new(self.list_state.clone(), rows.len(), move |range, _| {
            let mut elements: Vec<Box<dyn Element>> = Vec::with_capacity(range.len());
            for row in &rows[range] {
                let element = match row {
                    GridRow::Header(label) => Container::new(
                        Text::new_inline(*label, font, size - 2.)
                            .with_color(muted)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .with_padding_left(4.)
                    .with_padding_top(14.)
                    .finish(),
                    GridRow::Emojis(cells) => {
                        let mut line = Flex::row();
                        for index in cells.clone() {
                            let state = states.borrow_mut().entry(index).or_default().clone();
                            let emoji = flat[index].emoji.clone();
                            let custom = custom.clone();
                            line.add_child(
                                Hoverable::new(state, move |state| {
                                    let image = emoji_image(&emoji, &custom)
                                        .map(|image| {
                                            ConstrainedBox::new(image)
                                                .with_width(EMOJI_SIZE)
                                                .with_height(EMOJI_SIZE)
                                                .finish()
                                        })
                                        .unwrap_or_else(|| Empty::new().finish());
                                    let mut cell = Container::new(Align::new(image).finish())
                                        .with_corner_radius(CornerRadius::with_all(
                                            Radius::Pixels(4.),
                                        ));
                                    if index == selected || state.is_hovered() {
                                        cell = cell.with_background_color(fade(text, 0.1));
                                    }
                                    ConstrainedBox::new(cell.finish())
                                        .with_width(CELL)
                                        .with_height(CELL)
                                        .finish()
                                })
                                .with_cursor(Cursor::PointingHand)
                                .on_hover(move |hovered, ctx, _, _| {
                                    if hovered {
                                        ctx.dispatch_typed_action(PickerAction::Hover(index));
                                    }
                                })
                                .on_click(move |ctx, _, _| {
                                    ctx.dispatch_typed_action(PickerAction::Pick(index))
                                })
                                .finish(),
                            );
                        }
                        line.finish()
                    }
                };
                elements.push(ConstrainedBox::new(element).with_height(CELL).finish());
            }
            elements.into_iter()
        });
        Scrollable::vertical(
            self.scroll_state.clone(),
            list.finish_scrollable(),
            warpui::elements::ScrollbarWidth::Auto,
            fade(text, 0.25).into(),
            fade(text, 0.45).into(),
            warpui::elements::Fill::None,
        )
        .finish()
    }

    fn render_preview(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = theme.main_text_color(theme.surface_1()).into_solid();
        let muted = theme.sub_text_color(theme.surface_1()).into_solid();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let content = match self.flat.get(self.selected) {
            Some(emoji) => {
                let image = emoji_image(&emoji.emoji, &self.custom_paths)
                    .map(|image| {
                        ConstrainedBox::new(image)
                            .with_width(28.)
                            .with_height(28.)
                            .finish()
                    })
                    .unwrap_or_else(|| Empty::new().finish());
                Flex::row()
                    .with_spacing(8.)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(image)
                    .with_child(
                        Text::new_inline(format!(":{}:", emoji.name), font, size)
                            .with_color(text)
                            .finish(),
                    )
                    .finish()
            }
            None => Text::new_inline("No emoji found", font, size)
                .with_color(muted)
                .finish(),
        };
        ConstrainedBox::new(Container::new(content).with_padding_top(6.).finish())
            .with_height(40.)
            .finish()
    }
}

impl Entity for EmojiPickerView {
    type Event = PickerEvent;
}

impl View for EmojiPickerView {
    fn ui_name() -> &'static str {
        "EmojiPickerView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.search.update(ctx, |search, ctx| search.focus(ctx));
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let line = theme.surface_3();
        let rule = || {
            Container::new(
                ConstrainedBox::new(Empty::new().finish())
                    .with_height(1.)
                    .finish(),
            )
            .with_background(line)
            .finish()
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.)
            .with_child(ChildView::new(&self.search).finish());
        if !self.searching {
            column.add_child(self.render_tabs(appearance));
        }
        column.add_child(rule());
        column.add_child(
            ConstrainedBox::new(self.render_grid(appearance))
                .with_height(VISIBLE_ROWS as f32 * CELL)
                .finish(),
        );
        column.add_child(rule());
        column.add_child(self.render_preview(appearance));
        ConstrainedBox::new(
            Container::new(column.finish())
                .with_uniform_padding(PADDING)
                .with_background(theme.surface_1())
                .with_border(Border::all(1.).with_border_fill(line))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
                .finish(),
        )
        .with_width(WIDTH)
        .finish()
    }
}

impl TypedActionView for EmojiPickerView {
    type Action = PickerAction;

    fn handle_action(&mut self, action: &PickerAction, ctx: &mut ViewContext<Self>) {
        match action {
            PickerAction::Pick(index) => self.pick(*index, ctx),
            PickerAction::Hover(index) => {
                if self.selected != *index {
                    self.selected = *index;
                    ctx.notify();
                }
            }
            PickerAction::Section(index) => {
                if let Some(row) = self.headers.get(*index) {
                    self.scroll_to_row(*row);
                    if let Some(GridRow::Emojis(cells)) = self.rows.get(row + 1) {
                        self.selected = cells.start;
                    }
                    ctx.notify();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "emoji_picker_tests.rs"]
mod tests;

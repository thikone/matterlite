// SPDX-License-Identifier: AGPL-3.0-only
// Derived from warp-terminal `app/src/themes/theme_chooser.rs` @ 5bb7bcc6a — adapted for MatterLite.
// Original: Copyright (C) 2020-2026 Denver Technologies, Inc.

//! The theme chooser panel (MatterLite.md §11.1): a searchable list of theme previews. Moving
//! through the list applies and saves each theme at once; Esc or the close button closes the
//! panel. Following the system theme, it picks the theme for the current light/dark mode.
//!
//! The host shows the panel, calls [`ThemeChooser::open`] and focuses it, and hides it again on
//! [`ThemeChooserEvent::Close`].

use pathfinder_color::ColorU;
use settings::Setting as _;
use warp_core::paths::themes_dir;
use warpui::accessibility::{AccessibilityContent, WarpA11yRole};
use warpui::elements::{
    Align, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, DispatchEventResult,
    Element, Empty, EventHandler, Fill, Flex, Hoverable, Icon, MainAxisAlignment, MainAxisSize,
    MouseStateHandle, ParentElement, Radius, ScrollStateHandle, Scrollable, ScrollableElement,
    ScrollbarWidth, Shrinkable, Text, UniformList, UniformListState,
};
use warpui::fonts::{FamilyId, Weight};
use warpui::keymap::FixedBinding;
use warpui::platform::{Cursor, SystemTheme};
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, FocusContext, SingletonEntity, Tracked, TypedActionView, View, ViewContext,
    ViewHandle,
};

use ml_editor::{
    EditorView, Event as EditorEvent, NavigationKey, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextOptions,
};

use crate::appearance::{Appearance, AppearanceManager, AppearanceManagerEvent};
use crate::preview::render_preview;
use crate::settings::{ThemeSettings, respect_system_theme};
use crate::theme::{RespectSystemTheme, SelectedSystemThemes, ThemeKind, WarpTheme};

// All units in px
const THEME_CHOOSER_TITLE: &str = "Themes";
/// Matches the workspace's panel headers.
const HEADER_HEIGHT: f32 = 34.;
const CLOSE_BUTTON_SIZE: f32 = 20.;
const CLOSE_BUTTON_MARGIN_RIGHT: f32 = 6.;
const TITLE_FONT_SIZE: f32 = 16.;
const TITLE_MARGIN: f32 = 12.;
const SCROLLBAR_WIDTH: ScrollbarWidth = ScrollbarWidth::Auto;
const THEME_NAME_FONT_SIZE: f32 = 14.;
const THEME_NAME_MARGIN_LEFT: f32 = 16.;
const THEME_CHOOSER_ITEM_PADDING: f32 = 16.;
const SEARCH_PLACEHOLDER: &str = "Search themes";

#[derive(Default)]
struct MouseStateHandles {
    reload_button_mouse_state: MouseStateHandle,
    close_button_mouse_state: MouseStateHandle,
}

#[derive(Clone, Copy, Debug)]
pub enum ThemeChooserEvent {
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum ThemeChooserMode {
    /// Select a single fixed theme, independent of whether the system is using
    /// a light or dark theme.
    SystemAgnostic,
    /// Select a theme to use when the system is using a light theme.
    SystemLight,
    /// Select a theme to use when the system is using a dark theme.
    SystemDark,
}

impl ThemeChooserMode {
    /// Returns the mode the theme chooser should use if the aim is to change
    /// the active theme, as opposed to changing a specific theme option.
    pub fn for_active_theme(app: &AppContext) -> Self {
        match respect_system_theme(ThemeSettings::as_ref(app)) {
            RespectSystemTheme::On(_) => match app.system_theme() {
                SystemTheme::Dark => ThemeChooserMode::SystemDark,
                SystemTheme::Light => ThemeChooserMode::SystemLight,
            },
            RespectSystemTheme::Off => ThemeChooserMode::SystemAgnostic,
        }
    }

    pub fn into_theme_kind(self, ctx: &AppContext) -> ThemeKind {
        let theme_settings = ThemeSettings::as_ref(ctx);
        let theme_kind = theme_settings.theme_kind.value();
        match (self, &respect_system_theme(theme_settings)) {
            (ThemeChooserMode::SystemAgnostic, _) => theme_kind.clone(),
            (ThemeChooserMode::SystemLight, RespectSystemTheme::On(system_themes)) => {
                system_themes.light.clone()
            }
            (ThemeChooserMode::SystemDark, RespectSystemTheme::On(system_themes)) => {
                system_themes.dark.clone()
            }
            (_, _) => ThemeKind::default(),
        }
    }

    fn render_hint_text(&self, appearance: &Appearance) -> Box<dyn Element> {
        let hint_text = match self {
            ThemeChooserMode::SystemAgnostic => appearance
                .ui_builder()
                .paragraph("Change your current theme.".to_string()),
            ThemeChooserMode::SystemLight => appearance
                .ui_builder()
                .paragraph("Pick a theme for when your system is in light mode.".to_string()),
            ThemeChooserMode::SystemDark => appearance
                .ui_builder()
                .paragraph("Pick a theme for when your system is in dark mode.".to_string()),
        };
        hint_text
            .build()
            .with_margin_left(TITLE_MARGIN)
            .with_margin_right(TITLE_MARGIN)
            .finish()
    }
}

pub struct ThemeChooser {
    button_mouse_states: MouseStateHandles,
    list_state: UniformListState,
    scroll_state: ScrollStateHandle,
    selected_theme: Tracked<Option<ThemeKind>>,
    themes: Tracked<Vec<ThemeChooserItem>>,
    filtered_themes: Tracked<Option<Vec<ThemeChooserItem>>>,
    mode: ThemeChooserMode,
    search_editor: ViewHandle<EditorView>,
}

#[derive(Debug)]
pub enum ThemeChooserAction {
    Close,
    Enter,
    Click(ThemeKind),
    Up,
    Down,
    ReloadThemes,
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings(vec![
        FixedBinding::new("up", ThemeChooserAction::Up, id!("ThemeChooser")),
        FixedBinding::new("down", ThemeChooserAction::Down, id!("ThemeChooser")),
        FixedBinding::new("escape", ThemeChooserAction::Close, id!("ThemeChooser")),
        FixedBinding::new("enter", ThemeChooserAction::Enter, id!("ThemeChooser")),
    ]);
}

fn theme_chooser_items(app: &AppContext) -> Vec<ThemeChooserItem> {
    let mut theme_items: Vec<ThemeChooserItem> = AppearanceManager::as_ref(app)
        .themes()
        .theme_items()
        .map(|(key, theme)| ThemeChooserItem::new(key.clone(), theme.clone()))
        .collect();
    theme_items.sort_by(|a, b| a.kind.cmp(&b.kind));
    theme_items
}

impl ThemeChooser {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let search_editor = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(appearance.ui_font_size()), appearance),
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            EditorView::single_line(options, ctx)
        });
        search_editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text(SEARCH_PLACEHOLDER, ctx);
        });

        ctx.subscribe_to_view(&search_editor, move |me, _, event, ctx| {
            me.handle_editor_event(event, ctx);
        });

        ctx.subscribe_to_model(
            &AppearanceManager::handle(ctx),
            |me, _, event, ctx| match event {
                AppearanceManagerEvent::ThemesChanged => {
                    me.update_themes(ctx);
                    ctx.notify();
                }
            },
        );
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());

        Self {
            themes: Tracked::new(theme_chooser_items(ctx)),
            button_mouse_states: Default::default(),
            list_state: Default::default(),
            scroll_state: Default::default(),
            selected_theme: Tracked::new(None),
            filtered_themes: Tracked::new(None),
            mode: ThemeChooserMode::for_active_theme(ctx),
            search_editor,
        }
    }

    /// Prepares the chooser for showing: picks the mode for the active theme and selects
    /// that mode's theme.
    pub fn open(&mut self, ctx: &mut ViewContext<Self>) {
        let mode = ThemeChooserMode::for_active_theme(ctx);
        self.set_mode(mode);
        self.select_theme(mode.into_theme_kind(ctx), ctx);
        ctx.notify();
    }

    pub fn handle_theme_change(&mut self, ctx: &mut ViewContext<Self>) {
        // Ensure that we are still showing the right mode and have the correct theme selected.
        // This can get out of sync when the settings file changes while the chooser is open.
        // Note that we intentionally read from the settings model, not appearance here, as
        // the appearance will give us the derived theme, but we are trying to stay in sync
        // with the actual theme settings.
        let theme_settings = ThemeSettings::as_ref(ctx);
        let respect_system_theme = respect_system_theme(theme_settings);
        let system_theme = ctx.system_theme();
        match (respect_system_theme, self.mode, system_theme) {
            (
                RespectSystemTheme::On(selected_system_themes),
                ThemeChooserMode::SystemLight,
                SystemTheme::Light,
            )
            | (
                RespectSystemTheme::On(selected_system_themes),
                ThemeChooserMode::SystemDark,
                SystemTheme::Dark,
            ) => {
                // If we are choosing the theme for the current mode, ensure that we update the chooser state to match the
                // model state.
                let theme = match system_theme {
                    SystemTheme::Light => selected_system_themes.light.clone(),
                    SystemTheme::Dark => selected_system_themes.dark.clone(),
                };
                self.select_theme(theme, ctx);
            }
            (RespectSystemTheme::Off, ThemeChooserMode::SystemAgnostic, _) => {
                // If we are choosing the global theme, ensure that we update the chooser state to match the
                // model state
                let theme = ThemeSettings::as_ref(ctx).theme_kind.value().clone();
                self.select_theme(theme, ctx);
            }
            _ => {
                // Otherwise, we don't need to update anything, as we are in a state where we are
                // choosing a theme for an inactive mode.
            }
        }
    }

    /// Re-reads the custom themes from the themes directory and keeps the selection.
    fn reload_themes(&mut self, ctx: &mut ViewContext<Self>) {
        AppearanceManager::handle(ctx).update(ctx, |manager, ctx| manager.reload_themes(ctx));
        if let Some(selected) = self.selected_theme.clone() {
            self.select_theme(selected, ctx);
        }
        self.focus_search(ctx);
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(_) => {
                self.refilter(ctx);
                // Finding the position of the selected theme to adjust the scroll position of the
                // list of visible themes.
                let index = self.theme_position(self.selected_theme.clone().unwrap_or_default());
                self.list_state.scroll_to(index.unwrap_or_default());
            }
            EditorEvent::Navigate(NavigationKey::Up) => self.up(ctx),
            EditorEvent::Navigate(NavigationKey::Down) => self.down(ctx),
            EditorEvent::Enter => self.enter(ctx),
            EditorEvent::Escape => self.close(ctx),
            _ => {}
        }
    }

    fn refilter(&mut self, ctx: &AppContext) {
        let search_term = self.search_editor.as_ref(ctx).buffer_text(ctx);
        *self.filtered_themes = if search_term.is_empty() {
            None
        } else {
            Some(
                self.themes
                    .iter()
                    .filter(|item| item.kind.matches(&search_term))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
    }

    pub fn set_mode(&mut self, mode: ThemeChooserMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> ThemeChooserMode {
        self.mode
    }

    pub fn themes(&self) -> impl Iterator<Item = &ThemeKind> {
        self.themes
            .iter()
            .map(|theme_chooser_item| &theme_chooser_item.kind)
    }

    pub fn selected(&self) -> Option<&ThemeKind> {
        self.selected_theme.as_ref()
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        *self.selected_theme = None;
        *self.filtered_themes = None;
        AppearanceManager::handle(ctx).update(ctx, |appearance_manager, ctx| {
            appearance_manager.clear_transient_theme(ctx);
        });
        self.search_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx);
        });
        ctx.emit(ThemeChooserEvent::Close);
    }

    fn enter(&mut self, ctx: &mut ViewContext<Self>) {
        // "Enter" should close the theme picker is there is a theme that's visibly selected
        // If the user has entered a search term, but no theme is selected, we should not close
        if self.is_selected_theme_visible() {
            self.close(ctx);
        } else {
            log::info!("Handled enter key in theme chooser, but no theme is visibly selected.")
        }
    }

    pub fn select_and_save_theme(
        &mut self,
        selected_kind: &ThemeKind,
        ctx: &mut ViewContext<Self>,
    ) {
        self.select_theme(selected_kind.clone(), ctx);
        let theme_settings = ThemeSettings::handle(ctx);

        let selected_themes = respect_system_theme(theme_settings.as_ref(ctx))
            .selected_system_themes()
            .cloned()
            .unwrap_or_default();
        let result = theme_settings.update(ctx, |theme_settings, ctx| match self.mode {
            ThemeChooserMode::SystemAgnostic => theme_settings
                .theme_kind
                .set_value(selected_kind.clone(), ctx),
            ThemeChooserMode::SystemLight => theme_settings.selected_system_themes.set_value(
                SelectedSystemThemes {
                    light: selected_kind.clone(),
                    dark: selected_themes.dark,
                },
                ctx,
            ),
            ThemeChooserMode::SystemDark => theme_settings.selected_system_themes.set_value(
                SelectedSystemThemes {
                    light: selected_themes.light,
                    dark: selected_kind.clone(),
                },
                ctx,
            ),
        });
        if let Err(err) = result {
            log::error!("Couldn't save the theme {selected_kind}: {err:#}");
        }
    }

    fn theme_position(&self, kind: ThemeKind) -> Option<usize> {
        self.filtered_themes
            .as_ref()
            .unwrap_or_else(|| self.themes.as_ref())
            .iter()
            .position(|item| item.kind == kind)
    }

    pub fn select_theme(&mut self, kind: ThemeKind, ctx: &mut ViewContext<Self>) {
        let index = self.theme_position(kind.clone()).unwrap_or_default();

        self.list_state.scroll_to(index);

        *self.selected_theme = Some(kind.clone());

        AppearanceManager::handle(ctx).update(ctx, |appearance_manager, ctx| {
            appearance_manager.set_transient_theme(&kind, ctx);
        });
    }

    fn update_themes(&mut self, ctx: &mut ViewContext<Self>) {
        *self.themes = theme_chooser_items(ctx);
        if self.filtered_themes.is_some() {
            self.refilter(ctx);
        }
    }

    fn up(&mut self, ctx: &mut ViewContext<Self>) {
        if self.visible_theme_count() == 0 {
            return;
        }

        let index = match &*self.selected_theme {
            None => 0,
            Some(selected_kind) => self
                .theme_position(selected_kind.clone())
                .unwrap_or_default()
                .saturating_sub(1),
        };
        self.list_state.scroll_to(index);
        self.select_and_save_theme(&self.selected_theme(index), ctx);
    }

    fn down(&mut self, ctx: &mut ViewContext<Self>) {
        if self.visible_theme_count() == 0 {
            return;
        }

        let index = match &*self.selected_theme {
            None => 0,
            Some(selected_kind) => {
                match self.theme_position(selected_kind.clone()) {
                    None => 0, // selected element is not visible
                    Some(index) => (index + 1).min(self.visible_theme_count() - 1),
                }
            }
        };
        self.list_state.scroll_to(index);
        self.select_and_save_theme(&self.selected_theme(index), ctx);
    }

    fn visible_theme_count(&self) -> usize {
        match &*self.filtered_themes {
            None => self.themes.len(),
            Some(themes) => themes.len(),
        }
    }

    fn selected_theme(&self, index: usize) -> ThemeKind {
        match &*self.filtered_themes {
            None => self.themes[index].kind.clone(),
            Some(themes) => themes[index].kind.clone(),
        }
    }

    fn is_selected_theme_visible(&self) -> bool {
        self.selected_theme
            .as_ref()
            .map(|selected_kind| self.theme_position(selected_kind.clone()).is_some())
            .unwrap_or(false)
    }

    fn click(&mut self, kind: ThemeKind, ctx: &mut ViewContext<Self>) {
        self.select_and_save_theme(&kind, ctx);
        // Keep the keyboard in the chooser so Up/Down continue from the clicked theme.
        self.focus_search(ctx);
    }

    fn focus_search(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.search_editor);
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let close_button = appearance
            .ui_builder()
            .close_button(
                CLOSE_BUTTON_SIZE,
                self.button_mouse_states.close_button_mouse_state.clone(),
            )
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(ThemeChooserAction::Close))
            .finish();

        ConstrainedBox::new(
            Flex::row()
                .with_child(
                    Container::new(Align::new(close_button).finish())
                        .with_margin_right(CLOSE_BUTTON_MARGIN_RIGHT)
                        .finish(),
                )
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .finish(),
        )
        .with_height(HEADER_HEIGHT)
        .finish()
    }

    fn render_title_row(&self, appearance: &Appearance) -> Box<dyn Element> {
        let reload_button = appearance
            .ui_builder()
            .retry_button(
                CLOSE_BUTTON_SIZE,
                self.button_mouse_states.reload_button_mouse_state.clone(),
            )
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(ThemeChooserAction::ReloadThemes))
            .with_cursor(Cursor::PointingHand)
            .finish();

        let title_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Shrinkable::new(
                    1.0,
                    Align::new(
                        appearance
                            .ui_builder()
                            .span(THEME_CHOOSER_TITLE.to_string())
                            .with_style(UiComponentStyles {
                                font_family_id: Some(appearance.ui_font_family()),
                                font_size: Some(TITLE_FONT_SIZE),
                                font_weight: Some(Weight::Semibold),
                                ..Default::default()
                            })
                            .build()
                            .finish(),
                    )
                    .left()
                    .finish(),
                )
                .finish(),
            )
            .with_child(reload_button);

        Container::new(title_row.finish())
            .with_margin_bottom(6.)
            .with_margin_left(TITLE_MARGIN)
            .with_margin_right(TITLE_MARGIN)
            .finish()
    }

    fn render_search_bar(&self, appearance: &Appearance) -> Box<dyn Element> {
        Container::new(
            Flex::row()
                .with_child(
                    Container::new(
                        ConstrainedBox::new(
                            Icon::new(
                                "bundled/svg/find.svg",
                                appearance.theme().active_ui_detail(),
                            )
                            .finish(),
                        )
                        .with_height(10.)
                        .with_width(10.)
                        .finish(),
                    )
                    .with_margin_right(3.)
                    .with_padding_top(12.)
                    .finish(),
                )
                .with_child(
                    Shrinkable::new(
                        1.,
                        appearance
                            .ui_builder()
                            .text_input(self.search_editor.clone())
                            .with_style(UiComponentStyles {
                                border_radius: Some(CornerRadius::with_all(Radius::Pixels(0.))),
                                background: Some(Fill::None),
                                border_width: Some(0.),
                                ..Default::default()
                            })
                            .build()
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        )
        .with_margin_left(TITLE_MARGIN)
        .finish()
    }

    fn render_list(&self, appearance: &Appearance) -> Box<dyn Element> {
        let themes = self
            .filtered_themes
            .as_ref()
            .unwrap_or_else(|| self.themes.as_ref())
            .to_vec();

        let selected_kind = self.selected_theme.clone();

        let list_len = themes.len();
        let element = if list_len == 0 {
            // renders a text & an empty rectangle that expands over the panel
            // without it, the theme picker panel would be shorter than the window
            Flex::column()
                .with_child(
                    appearance
                        .ui_builder()
                        .span("No matching themes!".to_string())
                        .build()
                        .finish(),
                )
                .with_child(Empty::new().finish())
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .finish()
        } else {
            let list = UniformList::new(self.list_state.clone(), list_len, move |range, ctx| {
                let appearance = Appearance::as_ref(ctx);
                let font_family = appearance.ui_font_family();
                let text_color = appearance.theme().active_ui_text_color().into();
                let selected_background_color = appearance.theme().surface_2();

                themes
                    .clone()
                    .into_iter()
                    .skip(range.start)
                    .take(range.end - range.start)
                    .map(|item| {
                        let selected = match &selected_kind {
                            Some(selected_kind) => selected_kind == &item.kind,
                            None => false,
                        };
                        let element = item.render(
                            selected,
                            font_family,
                            text_color,
                            selected_background_color.into(),
                        );
                        EventHandler::new(element)
                            .on_left_mouse_down(move |ctx, _, _| {
                                ctx.dispatch_typed_action(ThemeChooserAction::Click(
                                    item.kind.clone(),
                                ));
                                DispatchEventResult::StopPropagation
                            })
                            .finish()
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
            });
            let warp_theme = appearance.theme();

            Scrollable::vertical(
                self.scroll_state.clone(),
                list.finish_scrollable(),
                SCROLLBAR_WIDTH,
                warp_theme
                    .disabled_text_color(warp_theme.surface_2())
                    .into(),
                warp_theme.main_text_color(warp_theme.surface_2()).into(),
                Fill::None,
            )
            .finish()
        };

        Shrinkable::new(
            1.,
            ConstrainedBox::new(element).with_height(f32::MAX).finish(),
        )
        .finish()
    }

    /// Where custom themes go; the reload button picks up new files.
    fn render_footer(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        Container::new(
            Text::new(
                format!(
                    "Custom themes: YAML files in {}. The reload button above picks up new and edited files.",
                    themes_dir().display()
                ),
                appearance.ui_font_family(),
                appearance.ui_font_size() - 2.,
            )
            .with_color(theme.hint_text_color(theme.background()).into_solid())
            .finish(),
        )
        .with_uniform_padding(TITLE_MARGIN)
        .finish()
    }
}

impl Entity for ThemeChooser {
    type Event = ThemeChooserEvent;
}

impl TypedActionView for ThemeChooser {
    type Action = ThemeChooserAction;

    fn handle_action(&mut self, action: &ThemeChooserAction, ctx: &mut ViewContext<Self>) {
        use ThemeChooserAction::*;

        match action {
            Up => self.up(ctx),
            Down => self.down(ctx),
            Click(kind) => self.click(kind.clone(), ctx),
            Close => self.close(ctx),
            Enter => self.enter(ctx),
            ReloadThemes => self.reload_themes(ctx),
        }
    }
}

impl View for ThemeChooser {
    fn ui_name() -> &'static str {
        "ThemeChooser"
    }

    fn accessibility_contents(&self, _: &AppContext) -> Option<AccessibilityContent> {
        Some(AccessibilityContent::new(
            "Theme chooser. Type to search themes; up and down apply the previous or next theme.",
            "Press escape to close.",
            WarpA11yRole::WindowRole,
        ))
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        Container::new(
            Flex::column()
                .with_child(self.render_header(appearance))
                .with_child(self.render_title_row(appearance))
                .with_child(self.mode.render_hint_text(appearance))
                .with_child(self.render_search_bar(appearance))
                .with_child(self.render_list(appearance))
                .with_child(self.render_footer(appearance))
                .finish(),
        )
        .finish()
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.search_editor);
        }
    }
}

#[derive(Clone)]
struct ThemeChooserItem {
    pub kind: ThemeKind,
    warp_theme: WarpTheme,
    mouse_state: MouseStateHandle,
}

impl ThemeChooserItem {
    pub fn new(kind: ThemeKind, warp_theme: WarpTheme) -> Self {
        Self {
            kind,
            warp_theme,
            mouse_state: MouseStateHandle::default(),
        }
    }

    pub fn render(
        &self,
        is_selected: bool,
        font_family: FamilyId,
        text_color: ColorU,
        selected_background_color: ColorU,
    ) -> Box<dyn Element> {
        Hoverable::new(self.mouse_state.clone(), |_| {
            let thumbnail = render_preview(&self.warp_theme, font_family);

            let name_text = Container::new(
                ConstrainedBox::new(
                    Text::new_inline(self.kind.to_string(), font_family, THEME_NAME_FONT_SIZE)
                        .with_color(text_color)
                        .finish(),
                )
                .with_max_width(190.)
                .finish(),
            )
            .with_margin_left(THEME_NAME_MARGIN_LEFT)
            .finish();

            let mut container = Container::new(
                Flex::column()
                    .with_child(thumbnail)
                    .with_child(Container::new(name_text).with_margin_top(8.).finish())
                    .finish(),
            )
            .with_padding_top(THEME_CHOOSER_ITEM_PADDING)
            .with_padding_bottom(THEME_CHOOSER_ITEM_PADDING);

            if is_selected {
                container = container.with_background_color(selected_background_color);
            }

            container.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
    }
}

#[cfg(test)]
#[path = "theme_chooser_tests.rs"]
mod tests;

// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/workspace/view/left_panel.rs` @ 5bb7bcc6a (toolbelt header,
// button styling, resizable panel); the code is new.

//! The tool panel on the left: a toolbelt of six tabs (MatterLite.md §10.6) above the active
//! tab's content.

use enum_iterator::{Sequence, all};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, ConstrainedBox, Container, CrossAxisAlignment, DragBarSide, Flex, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, Resizable, ResizableStateHandle, Shrinkable,
    Text, resizable_state_handle,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use crate::buttons::icon_button_with_color;
use crate::{PANEL_HEADER_HEIGHT, WorkspaceAction};

const DEFAULT_WIDTH: f32 = 300.;
const MIN_WIDTH: f32 = 220.;
const MAX_WIDTH_RATIO: f32 = 0.6;
const TOOLBELT_BUTTON_SIZE: f32 = 24.;
const HEADER_PADDING_LEFT: f32 = 10.;
const HEADER_PADDING_RIGHT: f32 = 4.;

/// The tool panel's tabs, in toolbelt order. `Alt+1…6` selects them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Sequence)]
pub enum ToolPanelTab {
    Chats,
    Activity,
    Channel,
    History,
    Files,
    Search,
}

impl ToolPanelTab {
    pub fn title(self) -> &'static str {
        match self {
            Self::Chats => "Chats",
            Self::Activity => "Activity",
            Self::Channel => "Channel",
            Self::History => "History",
            Self::Files => "Files",
            Self::Search => "Search",
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Chats => Icon::MessageChatSquare,
            Self::Activity => Icon::Bell,
            Self::Channel => Icon::Info,
            Self::History => Icon::History,
            Self::Files => Icon::Folder,
            Self::Search => Icon::Search,
        }
    }

    /// The 1-based position used by the `Alt+N` shortcut.
    pub fn number(self) -> usize {
        all::<Self>()
            .position(|tab| tab == self)
            .expect("tab is in the sequence")
            + 1
    }

    /// What the tab will hold; shown until the real pane is built.
    fn placeholder(self) -> &'static str {
        match self {
            Self::Chats => "Teams, channel categories, direct and group messages.",
            Self::Activity => "Threads, mentions, saved messages, drafts and scheduled posts.",
            Self::Channel => "Info, members, pinned messages and bookmarks of the active chat.",
            Self::History => "Messages of the active chat, grouped by date, with search.",
            Self::Files => "Every sent and received file, grouped by chat.",
            Self::Search => "Server-wide search in messages and files.",
        }
    }
}

pub struct ToolPanel {
    active_tab: ToolPanelTab,
    button_mouse_states: Vec<(ToolPanelTab, MouseStateHandle)>,
    close_mouse_state: MouseStateHandle,
    resizable_state: ResizableStateHandle,
}

impl ToolPanel {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        Self {
            active_tab: ToolPanelTab::Chats,
            button_mouse_states: all::<ToolPanelTab>()
                .map(|tab| (tab, MouseStateHandle::default()))
                .collect(),
            close_mouse_state: MouseStateHandle::default(),
            resizable_state: resizable_state_handle(DEFAULT_WIDTH),
        }
    }

    pub fn active_tab(&self) -> ToolPanelTab {
        self.active_tab
    }

    pub fn set_active_tab(&mut self, tab: ToolPanelTab, ctx: &mut ViewContext<Self>) {
        if self.active_tab != tab {
            self.active_tab = tab;
            ctx.notify();
        }
    }

    fn render_toolbelt_button(
        &self,
        tab: ToolPanelTab,
        mouse_state: MouseStateHandle,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let active = tab == self.active_tab;
        let icon_color = if active {
            theme.foreground()
        } else {
            theme.sub_text_color(theme.background())
        };
        let tooltip = appearance
            .ui_builder()
            .tool_tip_with_sublabel(tab.title().to_string(), format!("Alt+{}", tab.number()))
            .build()
            .finish();
        let size = |styles: UiComponentStyles| UiComponentStyles {
            height: Some(TOOLBELT_BUTTON_SIZE),
            width: Some(TOOLBELT_BUTTON_SIZE),
            padding: Some(Coords::uniform(4.)),
            ..styles
        };
        icon_button_with_color(appearance, tab.icon(), active, mouse_state, icon_color)
            .with_tooltip(move || tooltip)
            .with_style(size(UiComponentStyles {
                font_color: Some(icon_color.into_solid()),
                ..Default::default()
            }))
            .with_active_styles(size(UiComponentStyles {
                font_color: Some(icon_color.into_solid()),
                background: Some(internal_colors::fg_overlay_3(theme).into()),
                ..Default::default()
            }))
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::ShowToolPanelTab(tab));
            })
            .with_cursor(Cursor::PointingHand)
            .finish()
    }

    fn render_close_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let tooltip = appearance
            .ui_builder()
            .tool_tip("Close panel".to_string())
            .build()
            .finish();
        icon_button_with_color(
            appearance,
            Icon::X,
            false,
            self.close_mouse_state.clone(),
            theme.sub_text_color(theme.background()),
        )
        .with_tooltip(move || tooltip)
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::ToggleToolPanel))
        .with_cursor(Cursor::PointingHand)
        .finish()
    }

    fn render_placeholder(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        Container::new(
            Flex::column()
                .with_child(
                    Container::new(
                        Text::new_inline(self.active_tab.title().to_uppercase(), font, size - 1.)
                            .with_color(theme.sub_text_color(theme.background()).into_solid())
                            .finish(),
                    )
                    .with_margin_bottom(8.)
                    .finish(),
                )
                .with_child(
                    Text::new(self.active_tab.placeholder().to_string(), font, size)
                        .with_color(theme.hint_text_color(theme.background()).into_solid())
                        .finish(),
                )
                .finish(),
        )
        .with_uniform_padding(12.)
        .finish()
    }
}

impl Entity for ToolPanel {
    type Event = ();
}

impl View for ToolPanel {
    fn ui_name() -> &'static str {
        "ToolPanel"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);

        let toolbelt =
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(4.)
                .with_main_axis_size(MainAxisSize::Min)
                .with_children(self.button_mouse_states.iter().map(|(tab, state)| {
                    self.render_toolbelt_button(*tab, state.clone(), appearance)
                }))
                .finish();

        let header = Container::new(
            ConstrainedBox::new(
                Flex::row()
                    .with_main_axis_size(MainAxisSize::Max)
                    .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(Shrinkable::new(1., toolbelt).finish())
                    .with_child(self.render_close_button(appearance))
                    .finish(),
            )
            .with_height(PANEL_HEADER_HEIGHT)
            .finish(),
        )
        .with_padding_left(HEADER_PADDING_LEFT)
        .with_padding_right(HEADER_PADDING_RIGHT)
        .finish();

        let content = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(header)
            .with_child(
                Shrinkable::new(
                    1.,
                    Align::new(self.render_placeholder(appearance))
                        .top_left()
                        .finish(),
                )
                .finish(),
            )
            .finish();

        Resizable::new(self.resizable_state.clone(), content)
            .with_dragbar_side(DragBarSide::Right)
            .on_resize(|ctx, _| ctx.notify())
            .with_bounds_callback(Box::new(|window_size| {
                (
                    MIN_WIDTH,
                    (window_size.x() * MAX_WIDTH_RATIO).max(MIN_WIDTH),
                )
            }))
            .finish()
    }
}

impl TypedActionView for ToolPanel {
    type Action = ();
}

// SPDX-License-Identifier: AGPL-3.0-only
// Modelled on warp-terminal `app/src/workspace/view.rs` @ 5bb7bcc6a (tab bar over a row of tool
// panel, tab content and right panel; the theme chooser as a panel left of them); the code is
// new.

//! The window's root view: tab bar, theme chooser (when open), tool panel, chat tab content and
//! right panel.

use ml_themes::theme_chooser::ThemeChooserAction;
use ml_themes::{ThemeChooser, ThemeChooserEvent};
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CrossAxisAlignment, Flex, MainAxisSize,
    MouseStateHandle, ParentElement, Rect, Shrinkable, Stack, Text,
};
use warpui::keymap::{Context, FixedBinding};
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::right_panel::RightPanel;
use crate::tab_bar::{AccountBadge, ChatTab, TabMenu, TabMenuItem, TabSpec, render_tab_bar};
use crate::tool_panel::{ToolPanel, ToolPanelEvent, ToolPanelTab};

/// Keymap context set while the right panel is open (Esc closes it).
const RIGHT_PANEL_OPEN: &str = "Workspace_RightPanelOpen";
/// Warp's width for the theme chooser panel; fits the 190-px previews.
const THEME_CHOOSER_WIDTH: f32 = 240.;
/// Opens or closes the theme chooser. Warp has no default key for it; this one is free on all
/// platforms and leaves Ctrl/Cmd+T for new tabs.
const THEME_CHOOSER_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-ctrl-t"
} else {
    "ctrl-alt-shift-T"
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Shows the tab in the tool panel, or hides the panel if that tab is already showing.
    ShowToolPanelTab(ToolPanelTab),
    ToggleToolPanel,
    ToggleRightPanel,
    CloseRightPanel,
    NewTab,
    ActivateTab(usize),
    /// 1-based, like the `Ctrl/Cmd+N` shortcut; 9 is always the last tab.
    ActivateTabByNumber(usize),
    ActivateNextTab,
    ActivatePrevTab,
    CloseTab(usize),
    CloseActiveTab,
    ReopenClosedTab,
    OpenTabMenu(usize),
    CloseTabMenu,
    TabMenu {
        index: usize,
        item: TabMenuItem,
    },
    /// Opens the theme chooser as a panel left of the tool panel, or closes it.
    ToggleThemeChooser,
    /// The account button in the tab bar was clicked.
    OpenAccountMenu,
    BannerAction,
}

/// A bar under the tab bar about the whole window (the connection), with an optional link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub text: String,
    /// Reported as [`WorkspaceEvent::BannerAction`] when clicked.
    pub action: Option<String>,
    /// A problem (drawn in the error colour) or information.
    pub warning: bool,
}

/// What the user did with the tabs or the account button; the owner (which holds the tabs'
/// chats) acts and calls [`Workspace::set_tabs`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// The account button was clicked; the owner shows the account menu.
    AccountMenu,
    ActivateTab(usize),
    CloseTab(usize),
    CloseOtherTabs(usize),
    TogglePinTab(usize),
    ReopenClosedTab,
    /// Ctrl/Cmd+T or the + button: open a chat.
    NewTab,
    /// The banner's link was clicked.
    BannerAction,
}

/// Registers the workspace keybindings (MatterLite.md §10.5).
pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    let workspace = || id!("Workspace");
    let mut bindings = Vec::new();
    for tab in enum_iterator::all::<ToolPanelTab>() {
        bindings.push(FixedBinding::new(
            format!("alt-{}", tab.number()),
            WorkspaceAction::ShowToolPanelTab(tab),
            workspace(),
        ));
    }
    for number in 1..=9 {
        bindings.push(FixedBinding::new(
            format!("cmdorctrl-{number}"),
            WorkspaceAction::ActivateTabByNumber(number),
            workspace(),
        ));
    }
    bindings.extend([
        FixedBinding::new("ctrl-tab", WorkspaceAction::ActivateNextTab, workspace()),
        FixedBinding::new(
            "ctrl-shift-tab",
            WorkspaceAction::ActivatePrevTab,
            workspace(),
        ),
        FixedBinding::new("cmdorctrl-t", WorkspaceAction::NewTab, workspace()),
        FixedBinding::new("cmdorctrl-w", WorkspaceAction::CloseActiveTab, workspace()),
        FixedBinding::new(
            "cmdorctrl-shift-T",
            WorkspaceAction::ReopenClosedTab,
            workspace(),
        ),
        FixedBinding::new(
            THEME_CHOOSER_KEY,
            WorkspaceAction::ToggleThemeChooser,
            workspace(),
        ),
        FixedBinding::new(
            "escape",
            WorkspaceAction::CloseRightPanel,
            workspace() & id!(RIGHT_PANEL_OPEN),
        ),
    ]);
    app.register_fixed_bindings(bindings);
}

pub struct Workspace {
    tabs: Vec<ChatTab>,
    active_tab: Option<usize>,
    tab_menu: Option<TabMenu>,
    new_tab_mouse_state: MouseStateHandle,
    tool_panel: ViewHandle<ToolPanel>,
    tool_panel_open: bool,
    right_panel: ViewHandle<RightPanel>,
    right_panel_open: bool,
    theme_chooser: ViewHandle<ThemeChooser>,
    theme_chooser_open: bool,
    account: Option<AccountBadge>,
    account_mouse_state: MouseStateHandle,
    /// Lines shown while no chat is open.
    placeholder: Vec<String>,
    /// The active tab's content, supplied by the app.
    content: Option<Box<dyn crate::ToolPane>>,
    banner: Option<Banner>,
    banner_mouse_state: MouseStateHandle,
}

impl Workspace {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        let tool_panel = ctx.add_typed_action_view(ToolPanel::new);
        ctx.subscribe_to_view(&tool_panel, |me, _, event, ctx| match event {
            ToolPanelEvent::ReturnFocus => me.focus_content(ctx),
        });
        let right_panel = ctx.add_typed_action_view(RightPanel::new);
        let theme_chooser = ctx.add_typed_action_view(ThemeChooser::new);
        ctx.subscribe_to_view(&theme_chooser, |me, _, event, ctx| match event {
            // Closed from inside (Esc, Enter, the close button). When the workspace closes it,
            // the flag is already cleared and the focus already moved on.
            ThemeChooserEvent::Close => {
                if me.theme_chooser_open {
                    me.theme_chooser_open = false;
                    ctx.focus_self();
                    ctx.notify();
                }
            }
        });
        ctx.focus_self();
        Self {
            tabs: Vec::new(),
            active_tab: None,
            tab_menu: None,
            new_tab_mouse_state: MouseStateHandle::default(),
            tool_panel,
            tool_panel_open: true,
            right_panel,
            right_panel_open: false,
            theme_chooser,
            theme_chooser_open: false,
            account: None,
            account_mouse_state: MouseStateHandle::default(),
            placeholder: Vec::new(),
            content: None,
            banner: None,
            banner_mouse_state: MouseStateHandle::default(),
        }
    }

    /// Shows a bar under the tab bar, or removes it.
    pub fn set_banner(&mut self, banner: Option<Banner>, ctx: &mut ViewContext<Self>) {
        if banner != self.banner {
            self.banner = banner;
            ctx.notify();
        }
    }

    fn render_banner(&self, banner: &Banner, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let chat = theme.chat();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let text_color = theme.main_text_color(theme.background()).into_solid();
        let accent = if banner.warning {
            chat.error_text
        } else {
            chat.button_bg
        };
        let mut background = accent;
        background.a = 36;
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(warpui::elements::MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(12.)
            .with_child(
                Text::new_inline(banner.text.clone(), font, size)
                    .with_color(text_color)
                    .finish(),
            );
        if let Some(action) = &banner.action {
            let action = action.clone();
            let link = chat.link;
            row.add_child(
                warpui::elements::Hoverable::new(self.banner_mouse_state.clone(), move |state| {
                    let mut properties = warpui::fonts::Properties::default();
                    if state.is_hovered() {
                        properties = properties.weight(warpui::fonts::Weight::Bold);
                    }
                    Text::new_inline(action.clone(), font, size)
                        .with_color(link)
                        .with_style(properties)
                        .finish()
                })
                .with_cursor(warpui::platform::Cursor::PointingHand)
                .on_click(|ctx, _, _| ctx.dispatch_typed_action(WorkspaceAction::BannerAction))
                .finish(),
            );
        }
        Container::new(row.finish())
            .with_vertical_padding(6.)
            .with_background_color(background)
            .with_border(Border::bottom(1.).with_border_color(accent))
            .finish()
    }

    /// What the active tab shows; `None` shows the placeholder.
    pub fn set_content(
        &mut self,
        content: Option<Box<dyn crate::ToolPane>>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.content = content;
        ctx.notify();
    }

    /// Moves the focus into the tab content (or onto the workspace without one).
    pub fn focus_content(&mut self, ctx: &mut ViewContext<Self>) {
        match &self.content {
            Some(content) if self.active_tab.is_some() => content.focus_from(ctx),
            _ => ctx.focus_self(),
        }
    }

    /// The account button at the right end of the tab bar.
    pub fn set_account(&mut self, account: Option<AccountBadge>, ctx: &mut ViewContext<Self>) {
        if self.account != account {
            self.account = account;
            ctx.notify();
        }
    }

    /// Shows `pane` in a tool panel tab instead of its placeholder.
    pub fn set_tool_pane(
        &mut self,
        tab: ToolPanelTab,
        pane: Option<Box<dyn crate::ToolPane>>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.tool_panel
            .update(ctx, |panel, ctx| panel.set_pane(tab, pane, ctx));
    }

    /// The tabs to show (the owner keeps their chats) and the active one.
    pub fn set_tabs(
        &mut self,
        specs: Vec<TabSpec>,
        active: Option<usize>,
        ctx: &mut ViewContext<Self>,
    ) {
        let unchanged = active == self.active_tab
            && specs.len() == self.tabs.len()
            && specs
                .iter()
                .zip(&self.tabs)
                .all(|(spec, tab)| *spec == tab.spec);
        if unchanged {
            return;
        }
        // Mouse states stay with their position, so hovering survives relabelling.
        self.tabs.truncate(specs.len());
        for (index, spec) in specs.into_iter().enumerate() {
            match self.tabs.get_mut(index) {
                Some(tab) => tab.spec = spec,
                None => self.tabs.push(ChatTab::new(spec)),
            }
        }
        self.active_tab = active.filter(|index| *index < self.tabs.len());
        if self
            .tab_menu
            .as_ref()
            .is_some_and(|menu| menu.index >= self.tabs.len())
        {
            self.tab_menu = None;
        }
        ctx.notify();
    }

    /// The text of the placeholder chat content.
    pub fn set_placeholder(&mut self, lines: Vec<String>, ctx: &mut ViewContext<Self>) {
        if self.placeholder != lines {
            self.placeholder = lines;
            ctx.notify();
        }
    }

    fn toggle_theme_chooser(&mut self, ctx: &mut ViewContext<Self>) {
        if self.theme_chooser_open {
            self.close_theme_chooser(ctx);
            ctx.focus_self();
        } else {
            self.theme_chooser_open = true;
            self.theme_chooser
                .update(ctx, |chooser, ctx| chooser.open(ctx));
            ctx.focus(&self.theme_chooser);
        }
        ctx.notify();
    }

    /// Closes the chooser as its own Close does (ends the preview, clears the search).
    fn close_theme_chooser(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.theme_chooser_open {
            return;
        }
        self.theme_chooser_open = false;
        self.theme_chooser.update(ctx, |chooser, ctx| {
            chooser.handle_action(&ThemeChooserAction::Close, ctx)
        });
        ctx.notify();
    }

    /// Shows the tab and focuses its search field, if it has one; showing the tab that is
    /// already showing hides the panel.
    fn show_tool_panel_tab(&mut self, tab: ToolPanelTab, ctx: &mut ViewContext<Self>) {
        let showing = self.tool_panel.as_ref(ctx).active_tab();
        if self.tool_panel_open && showing == tab {
            self.set_tool_panel_open(false, ctx);
            return;
        }
        self.tool_panel_open = true;
        let focused_search = self.tool_panel.update(ctx, |panel, ctx| {
            panel.set_active_tab(tab, ctx);
            panel.focus_search(ctx)
        });
        // Don't leave the focus in the search field of a tab that is no longer showing.
        if !focused_search && self.tool_panel.as_ref(ctx).has_focus(ctx) {
            ctx.focus_self();
        }
        ctx.notify();
    }

    fn set_tool_panel_open(&mut self, open: bool, ctx: &mut ViewContext<Self>) {
        self.tool_panel_open = open;
        if !open && self.tool_panel.as_ref(ctx).has_focus(ctx) {
            ctx.focus_self();
        }
        ctx.notify();
    }

    fn activate_tab(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if index < self.tabs.len() {
            ctx.emit(WorkspaceEvent::ActivateTab(index));
        }
    }

    fn step_tab(&mut self, forward: bool, ctx: &mut ViewContext<Self>) {
        let len = self.tabs.len();
        if len == 0 {
            return;
        }
        let index = match (self.active_tab, forward) {
            (Some(active), true) => (active + 1) % len,
            (Some(active), false) => (active + len - 1) % len,
            (None, true) => 0,
            (None, false) => len - 1,
        };
        self.activate_tab(index, ctx);
    }

    fn render_tab_content(&self, appearance: &Appearance) -> Box<dyn Element> {
        if let Some(content) = &self.content
            && self.active_tab.is_some()
        {
            return content.render();
        }
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let title = self
            .active_tab
            .and_then(|index| self.tabs.get(index))
            .map_or_else(|| "No chat open".to_owned(), |tab| tab.spec.title.clone());
        let line = |text: String, size: f32, color| {
            Container::new(
                Text::new_inline(text, font, size)
                    .with_color(color)
                    .finish(),
            )
            .with_margin_bottom(6.)
            .finish()
        };
        Align::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(line(
                    title,
                    size + 8.,
                    theme.main_text_color(theme.background()).into_solid(),
                ))
                .with_children(self.placeholder.iter().map(|text| {
                    line(
                        text.clone(),
                        size,
                        theme.sub_text_color(theme.background()).into_solid(),
                    )
                }))
                .with_child(line(
                    "Ctrl+K open a chat · Alt+↑/↓ next chat · Ctrl+W close · \
                     Ctrl+Shift+T reopen · Ctrl+Tab next tab · Alt+1…6 tool panel"
                        .to_string(),
                    size - 1.,
                    theme.hint_text_color(theme.background()).into_solid(),
                ))
                .finish(),
        )
        .finish()
    }
}

impl Entity for Workspace {
    type Event = WorkspaceEvent;
}

impl View for Workspace {
    fn ui_name() -> &'static str {
        "Workspace"
    }

    fn keymap_context(&self, _: &AppContext) -> Context {
        let mut context = Self::default_keymap_context();
        if self.right_panel_open {
            context.set.insert(RIGHT_PANEL_OPEN);
        }
        context
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let divider = internal_colors::fg_overlay_3(theme);

        let mut body = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        // As in Warp, the chooser opens beside the other panels instead of replacing them, so
        // the tool panel shows each previewed theme too.
        if self.theme_chooser_open {
            body.add_child(
                Container::new(
                    ConstrainedBox::new(ChildView::new(&self.theme_chooser).finish())
                        .with_width(THEME_CHOOSER_WIDTH)
                        .finish(),
                )
                .with_border(Border::right(1.).with_border_fill(divider))
                .finish(),
            );
        }
        if self.tool_panel_open {
            body.add_child(
                Container::new(ChildView::new(&self.tool_panel).finish())
                    .with_border(Border::right(1.).with_border_fill(divider))
                    .finish(),
            );
        }
        body.add_child(Shrinkable::new(1., self.render_tab_content(appearance)).finish());
        if self.right_panel_open {
            body.add_child(
                Container::new(ChildView::new(&self.right_panel).finish())
                    .with_border(Border::left(1.).with_border_fill(divider))
                    .finish(),
            );
        }

        let column = Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(render_tab_bar(
                &self.tabs,
                self.active_tab,
                self.tab_menu.as_ref(),
                &self.new_tab_mouse_state,
                self.account
                    .as_ref()
                    .map(|account| (account, &self.account_mouse_state)),
                appearance,
            ))
            .with_child(match &self.banner {
                Some(banner) => self.render_banner(banner, appearance),
                None => warpui::elements::Empty::new().finish(),
            })
            .with_child(Shrinkable::new(1., body.finish()).finish())
            .finish();

        Stack::new()
            .with_child(Rect::new().with_background(theme.background()).finish())
            .with_child(column)
            .finish()
    }
}

impl TypedActionView for Workspace {
    type Action = WorkspaceAction;

    fn handle_action(&mut self, action: &WorkspaceAction, ctx: &mut ViewContext<Self>) {
        match *action {
            WorkspaceAction::ShowToolPanelTab(tab) => self.show_tool_panel_tab(tab, ctx),
            WorkspaceAction::BannerAction => ctx.emit(WorkspaceEvent::BannerAction),
            WorkspaceAction::ToggleToolPanel => {
                self.set_tool_panel_open(!self.tool_panel_open, ctx);
            }
            WorkspaceAction::ToggleThemeChooser => self.toggle_theme_chooser(ctx),
            WorkspaceAction::ToggleRightPanel => {
                self.right_panel_open = !self.right_panel_open;
                ctx.notify();
            }
            WorkspaceAction::CloseRightPanel => {
                self.right_panel_open = false;
                ctx.notify();
            }
            WorkspaceAction::NewTab => ctx.emit(WorkspaceEvent::NewTab),
            WorkspaceAction::ActivateTab(index) => self.activate_tab(index, ctx),
            WorkspaceAction::ActivateTabByNumber(number) => {
                if let Some(last) = self.tabs.len().checked_sub(1) {
                    let index = if number >= 9 { last } else { number - 1 };
                    self.activate_tab(index, ctx);
                }
            }
            WorkspaceAction::ActivateNextTab => self.step_tab(true, ctx),
            WorkspaceAction::ActivatePrevTab => self.step_tab(false, ctx),
            WorkspaceAction::CloseTab(index) => {
                if index < self.tabs.len() {
                    ctx.emit(WorkspaceEvent::CloseTab(index));
                }
            }
            WorkspaceAction::CloseActiveTab => {
                if let Some(active) = self.active_tab {
                    ctx.emit(WorkspaceEvent::CloseTab(active));
                }
            }
            WorkspaceAction::ReopenClosedTab => ctx.emit(WorkspaceEvent::ReopenClosedTab),
            WorkspaceAction::OpenTabMenu(index) => {
                self.tab_menu = Some(TabMenu {
                    index,
                    ..TabMenu::default()
                });
                ctx.notify();
            }
            WorkspaceAction::CloseTabMenu => {
                self.tab_menu = None;
                ctx.notify();
            }
            WorkspaceAction::TabMenu { index, item } => {
                self.tab_menu = None;
                ctx.notify();
                ctx.emit(match item {
                    TabMenuItem::TogglePin => WorkspaceEvent::TogglePinTab(index),
                    TabMenuItem::Close => WorkspaceEvent::CloseTab(index),
                    TabMenuItem::CloseOthers => WorkspaceEvent::CloseOtherTabs(index),
                    TabMenuItem::Reopen => WorkspaceEvent::ReopenClosedTab,
                });
            }
            WorkspaceAction::OpenAccountMenu => ctx.emit(WorkspaceEvent::AccountMenu),
        }
    }
}

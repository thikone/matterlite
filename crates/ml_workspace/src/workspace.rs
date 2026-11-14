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
use crate::tab_bar::{ChatTab, render_tab_bar};
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
    /// Opens the theme chooser as a panel left of the tool panel, or closes it.
    ToggleThemeChooser,
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
    active_tab: usize,
    /// Counter for placeholder tab titles until tabs hold real chats.
    tabs_created: usize,
    new_tab_mouse_state: MouseStateHandle,
    tool_panel: ViewHandle<ToolPanel>,
    tool_panel_open: bool,
    right_panel: ViewHandle<RightPanel>,
    right_panel_open: bool,
    theme_chooser: ViewHandle<ThemeChooser>,
    theme_chooser_open: bool,
}

impl Workspace {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        let tool_panel = ctx.add_typed_action_view(ToolPanel::new);
        ctx.subscribe_to_view(&tool_panel, |_, _, event, ctx| match event {
            // Until chats have a composer, the workspace itself takes the focus back.
            ToolPanelEvent::ReturnFocus => ctx.focus_self(),
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
            tabs: vec![ChatTab::new("Welcome")],
            active_tab: 0,
            tabs_created: 1,
            new_tab_mouse_state: MouseStateHandle::default(),
            tool_panel,
            tool_panel_open: true,
            right_panel,
            right_panel_open: false,
            theme_chooser,
            theme_chooser_open: false,
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
        if index < self.tabs.len() && index != self.active_tab {
            self.active_tab = index;
            ctx.notify();
        }
    }

    fn close_tab(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.new_tab(ctx);
            return;
        }
        if self.active_tab > index || self.active_tab >= self.tabs.len() {
            self.active_tab = self.active_tab.saturating_sub(1);
        }
        ctx.notify();
    }

    fn new_tab(&mut self, ctx: &mut ViewContext<Self>) {
        self.tabs_created += 1;
        self.tabs
            .push(ChatTab::new(format!("Chat {}", self.tabs_created)));
        self.active_tab = self.tabs.len() - 1;
        ctx.notify();
    }

    fn render_tab_content(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let title = &self.tabs[self.active_tab].title;
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
                    title.clone(),
                    size + 8.,
                    theme.main_text_color(theme.background()).into_solid(),
                ))
                .with_child(line(
                    "The timeline and composer of this chat will be here.".to_string(),
                    size,
                    theme.sub_text_color(theme.background()).into_solid(),
                ))
                .with_child(line(
                    "Alt+1…6 tool panel · Ctrl+T new tab · Ctrl+W close · Ctrl+Tab next · \
                     Ctrl+Alt+Shift+T themes"
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
    type Event = ();
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
                &self.new_tab_mouse_state,
                appearance,
            ))
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
            WorkspaceAction::NewTab => self.new_tab(ctx),
            WorkspaceAction::ActivateTab(index) => self.activate_tab(index, ctx),
            WorkspaceAction::ActivateTabByNumber(number) => {
                let index = if number >= 9 {
                    self.tabs.len() - 1
                } else {
                    number - 1
                };
                self.activate_tab(index, ctx);
            }
            WorkspaceAction::ActivateNextTab => {
                self.activate_tab((self.active_tab + 1) % self.tabs.len(), ctx);
            }
            WorkspaceAction::ActivatePrevTab => {
                let len = self.tabs.len();
                self.activate_tab((self.active_tab + len - 1) % len, ctx);
            }
            WorkspaceAction::CloseTab(index) => self.close_tab(index, ctx),
            WorkspaceAction::CloseActiveTab => self.close_tab(self.active_tab, ctx),
        }
    }
}

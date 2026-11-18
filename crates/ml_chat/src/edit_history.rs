// SPDX-License-Identifier: AGPL-3.0-only

//! A post's edit history in the right panel (the webapp's `components/post_edit_history`):
//! the current version, then the earlier ones (newest first), each with when it was written;
//! a version expands to its message (the current one starts expanded). My post can be
//! restored to an earlier version, after the root view asks "Restore this version?".

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use mm_model::{Post, PostId};
use mm_sync::Command;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::elements::{
    Border, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, CornerRadius,
    CrossAxisAlignment, Empty, Expanded, Flex, Hoverable, MainAxisSize, MouseStateHandle,
    ParentElement, Radius, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent as _;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use ml_workspace::icon_button_with_color;
use mm_store::StoreDelta;

use crate::chats_view::{fade, icon};
use crate::message::{MessageRenderer, Palette, Target};
use crate::session::{Session, SessionEvent};
use crate::timeline::{self, ChatLookup, Clock};

const HEADER_HEIGHT: f32 = 44.;
const SIDE_PADDING: f32 = 20.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditHistoryEvent {
    /// The close button or Esc.
    Close,
    /// Restore in an earlier version: the root view asks first.
    Restore {
        post: PostId,
        version: PostId,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditHistoryAction {
    Toggle(PostId),
    Restore(PostId),
    Close,
    Click(Target),
}

pub(crate) fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;
    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        EditHistoryAction::Close,
        id!("EditHistoryView"),
    )]);
}

pub struct EditHistoryView {
    post: Option<PostId>,
    /// Expanded versions (by version id; the current one by the post's id).
    open: HashSet<PostId>,
    toggle_states: HashMap<PostId, MouseStateHandle>,
    restore_states: HashMap<PostId, MouseStateHandle>,
    close_state: MouseStateHandle,
    scroll_state: ClippedScrollStateHandle,
}

impl EditHistoryView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| {
            if let SessionEvent::Changed(deltas) = event {
                me.on_changes(deltas, ctx);
            }
        });
        Self {
            post: None,
            open: HashSet::new(),
            toggle_states: HashMap::new(),
            restore_states: HashMap::new(),
            close_state: MouseStateHandle::default(),
            scroll_state: ClippedScrollStateHandle::default(),
        }
    }

    /// Shows a post's history (loading it again), or nothing.
    pub fn show(&mut self, post: Option<PostId>, ctx: &mut ViewContext<Self>) {
        self.open.clear();
        self.toggle_states.clear();
        self.restore_states.clear();
        self.scroll_state.scroll_to(warpui::units::Pixels::zero());
        if let Some(post) = &post {
            self.open.insert(post.clone());
            Session::as_ref(ctx).send(Command::LoadEditHistory(post.clone()));
        }
        self.post = post;
        self.prepare_states(ctx);
        ctx.notify();
    }

    /// Hover states for the shown versions, so they last across renders.
    fn prepare_states(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(post) = self.post.clone() else {
            return;
        };
        let mut ids = vec![post.clone()];
        if let Some(store) = Session::as_ref(ctx).store().and_then(|s| s.read().ok())
            && let Some(Some(versions)) = store.edit_history.get(&post)
        {
            ids.extend(versions.iter().map(|version| version.id.clone()));
        }
        for id in ids {
            self.toggle_states.entry(id.clone()).or_default();
            self.restore_states.entry(id).or_default();
        }
    }

    pub fn post(&self) -> Option<&PostId> {
        self.post.as_ref()
    }

    fn on_changes(&mut self, deltas: &[StoreDelta], ctx: &mut ViewContext<Self>) {
        let Some(post) = self.post.clone() else {
            return;
        };
        for delta in deltas {
            match delta {
                StoreDelta::EditHistoryChanged(changed) if *changed == post => {
                    self.prepare_states(ctx);
                    ctx.notify();
                }
                // Edited (or restored) meanwhile: the history has a new entry.
                StoreDelta::PostsChanged(ids) if ids.contains(&post) => {
                    Session::as_ref(ctx).send(Command::LoadEditHistory(post.clone()));
                    ctx.notify();
                }
                StoreDelta::PostsRemoved(ids) if ids.contains(&post) => {
                    ctx.emit(EditHistoryEvent::Close)
                }
                _ => {}
            }
        }
    }

    /// The hover state of a version's toggle or restore button, kept across renders.
    fn state(states: &HashMap<PostId, MouseStateHandle>, id: &PostId) -> MouseStateHandle {
        states.get(id).cloned().unwrap_or_default()
    }

    fn render_header(&self, title: String, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let text = theme.main_text_color(theme.background()).into_solid();
        let muted = theme.sub_text_color(theme.background()).into_solid();
        let tooltip = appearance
            .ui_builder()
            .tool_tip("Close (Esc)".to_owned())
            .build()
            .finish();
        let close = icon_button_with_color(
            appearance,
            Icon::X,
            false,
            self.close_state.clone(),
            theme.sub_text_color(theme.background()),
        )
        .with_tooltip(move || tooltip)
        .build()
        .on_click(|ctx, _, _| ctx.dispatch_typed_action(EditHistoryAction::Close))
        .with_cursor(Cursor::PointingHand)
        .finish();
        let row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(
                Text::new_inline("Edit History", font, size + 2.)
                    .with_color(text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            )
            .with_child(
                Expanded::new(
                    1.,
                    Text::new_inline(title, font, size)
                        .with_color(muted)
                        .with_clip(ClipConfig::ellipsis())
                        .finish(),
                )
                .finish(),
            )
            .with_child(close);
        ConstrainedBox::new(
            Container::new(row.finish())
                .with_padding_left(SIDE_PADDING)
                .with_padding_right(8.)
                .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
                .finish(),
        )
        .with_height(HEADER_HEIGHT)
        .finish()
    }

    /// One version: the toggle with its time (and "Current Version"), Restore on earlier
    /// ones, and when expanded the author and the message.
    #[allow(clippy::too_many_arguments)]
    fn render_version(
        &self,
        version: &Post,
        id: &PostId,
        current: bool,
        restorable: bool,
        when: String,
        message: Option<(String, Rc<mm_render::MessageDoc>)>,
        palette: &Palette,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let open = self.open.contains(id);
        let (font, size, text, muted) = (palette.font, palette.size, palette.text, palette.muted);
        let link = appearance.theme().chat().link;
        let toggle_id = id.clone();
        let toggle = Hoverable::new(Self::state(&self.toggle_states, id), move |_| {
            let mut row = Flex::row()
                .with_spacing(6.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(icon(
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    muted,
                    size - 2.,
                ))
                .with_child(
                    Text::new_inline(when, font, size - 1.)
                        .with_color(text)
                        .finish(),
                );
            if current {
                row.add_child(
                    Container::new(
                        Text::new_inline("Current Version", font, size - 3.)
                            .with_color(link)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .with_horizontal_padding(6.)
                    .with_vertical_padding(1.)
                    .with_background_color(fade(link, 0.1))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                    .finish(),
                );
            }
            row.finish()
        })
        .with_cursor(Cursor::PointingHand)
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(EditHistoryAction::Toggle(toggle_id.clone()))
        })
        .finish();
        let mut title = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., toggle).finish());
        if restorable {
            let tooltip = appearance
                .ui_builder()
                .tool_tip("Restore this version".to_owned())
                .build()
                .finish();
            let restore_id = version.id.clone();
            title.add_child(
                icon_button_with_color(
                    appearance,
                    Icon::History,
                    false,
                    Self::state(&self.restore_states, id),
                    appearance
                        .theme()
                        .sub_text_color(appearance.theme().background()),
                )
                .with_tooltip(move || tooltip)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(EditHistoryAction::Restore(restore_id.clone()))
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            );
        }
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.)
            .with_child(title.finish());
        if let (true, Some((author, doc))) = (open, message) {
            let renderer = MessageRenderer {
                palette,
                custom_emoji: &Default::default(),
                on_click: Rc::new(|target, ctx| {
                    ctx.dispatch_typed_action(EditHistoryAction::Click(target))
                }),
                muted: false,
            };
            column.add_child(
                Text::new_inline(author, font, size - 1.)
                    .with_color(text)
                    .with_style(Properties::default().weight(Weight::Bold))
                    .finish(),
            );
            column.add_child(renderer.render(&doc));
        }
        let mut container = Container::new(column.finish())
            .with_uniform_padding(10.)
            .with_border(Border::all(1.).with_border_color(palette.border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
        if open {
            container = container.with_background_color(fade(text, 0.03));
        }
        container.finish()
    }
}

impl Entity for EditHistoryView {
    type Event = EditHistoryEvent;
}

impl View for EditHistoryView {
    fn ui_name() -> &'static str {
        "EditHistoryView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let palette = Palette::of(appearance);
        let session = Session::as_ref(app);
        let store = session.store().and_then(|store| store.read().ok());
        let (Some(post), Some(store)) = (self.post.as_ref(), store) else {
            return Empty::new().finish();
        };
        let current = store.posts.get(post).cloned();
        let title = current
            .as_ref()
            .and_then(|post| store.channel_name(&post.channel_id))
            .unwrap_or_default();
        let note = |text: &str| {
            Text::new(text.to_owned(), palette.font, palette.size - 1.)
                .with_color(palette.muted)
                .finish()
        };
        let body = match (&current, store.edit_history.get(post)) {
            (None, _) => note("This message was deleted."),
            (Some(_), None) => note("Loading the edit history…"),
            (Some(_), Some(None)) => note(
                "Unable to load edit history. There was an error loading the history for this \
                 message. Check your network connection or try again later.",
            ),
            (Some(current), Some(Some(versions))) => {
                let team = store
                    .channels
                    .get(&current.channel_id)
                    .map(|channel| channel.team_id.clone())
                    .filter(|team| !team.is_empty());
                let lookup = ChatLookup::new(&store, &current.channel_id, team.as_ref());
                let clock = Clock::of(&store);
                let today = clock.today(now_ms());
                let when = |version: &Post| {
                    let at = if version.edit_at == 0 {
                        version.create_at
                    } else {
                        version.edit_at
                    };
                    format!(
                        "{} at {}",
                        timeline::date_label(clock.zone.date(at), today),
                        clock.time(at)
                    )
                };
                let message = |version: &Post| {
                    let author = timeline::author(&store, version).name;
                    let doc = Rc::new(mm_render::render_post(version, &lookup.context()));
                    Some((author, doc))
                };
                let restorable = current.user_id == store.me;
                let mut column = Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(8.)
                    .with_child(self.render_version(
                        current,
                        &current.id,
                        true,
                        false,
                        when(current),
                        message(current),
                        &palette,
                        appearance,
                    ));
                for version in versions {
                    column.add_child(self.render_version(
                        version,
                        &version.id,
                        false,
                        restorable,
                        when(version),
                        message(version),
                        &palette,
                        appearance,
                    ));
                }
                column.finish()
            }
        };
        drop(store);
        let text = palette.text;
        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            Container::new(body)
                .with_horizontal_padding(SIDE_PADDING)
                .with_vertical_padding(12.)
                .finish(),
            warpui::elements::ScrollbarWidth::Auto,
            fade(text, 0.25).into(),
            fade(text, 0.45).into(),
            warpui::elements::Fill::None,
        )
        .finish();
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_header(title, appearance))
            .with_child(Shrinkable::new(1., scrollable).finish())
            .finish()
    }
}

impl TypedActionView for EditHistoryView {
    type Action = EditHistoryAction;

    fn handle_action(&mut self, action: &EditHistoryAction, ctx: &mut ViewContext<Self>) {
        match action {
            EditHistoryAction::Toggle(id) => {
                if !self.open.remove(id) {
                    self.open.insert(id.clone());
                }
                ctx.notify();
            }
            EditHistoryAction::Restore(version) => {
                let Some(post) = self.post.clone() else {
                    return;
                };
                let message = Session::as_ref(ctx).store().and_then(|store| {
                    let store = store.read().ok()?;
                    let versions = store.edit_history.get(&post)?.as_ref()?;
                    Some(versions.iter().find(|v| v.id == *version)?.message.clone())
                });
                if let Some(message) = message {
                    ctx.emit(EditHistoryEvent::Restore {
                        post,
                        version: version.clone(),
                        message,
                    });
                }
            }
            EditHistoryAction::Close => ctx.emit(EditHistoryEvent::Close),
            EditHistoryAction::Click(Target::Url(url)) => ctx.open_url(url),
            EditHistoryAction::Click(_) => {}
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

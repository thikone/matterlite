// SPDX-License-Identifier: AGPL-3.0-only

//! The tool panel's Channel tab (MatterLite.md §10.6): Info, Members and Pinned for the active
//! chat, following tab switches. Info shows the name, purpose and header and the actions:
//! favourite (moved between sidebar categories as the webapp does), mute, copy link and
//! leave (confirmed inline). Members are loaded when shown, online first. Pinned posts are
//! loaded when shown; a click shows the post in the chat.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use mm_model::{ChannelId, PostId, TeamId};
use mm_store::StoreDelta;
use mm_sync::Command;
use ui_components::{Component as _, Options as _, button};
use warp_core::ui::appearance::Appearance;
use warpui::r#async::Timer;
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    Border, ClippedScrollStateHandle, ClippedScrollable, Container, CrossAxisAlignment, Empty,
    Flex, Hoverable, MainAxisSize, MouseStateHandle, ParentElement, Shrinkable, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::platform::Cursor;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
};

use crate::channel_info::{self, MemberRow};
use crate::chats::{ChannelIcon, Presence};
use crate::chats_view::{channel_icon, fade};
use crate::message::{MessageRenderer, Palette, Target};
use crate::session::{Session, SessionEvent};
use crate::timeline::{ChatLookup, Clock};

const PADDING: f32 = 16.;
/// Pinned posts listed (the server returns them all; a channel rarely has more).
const MAX_PINNED: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Info,
    Members,
    Pinned,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChannelPaneAction {
    Show(Section),
    ToggleFavorite,
    ToggleMute,
    CopyLink,
    Leave,
    ConfirmLeave,
    CancelLeave,
    ShowPost(PostId),
    Click(Target),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelPaneEvent {
    /// Show this post in the chat.
    ShowPost(PostId),
}

#[derive(Default)]
struct Buttons {
    favorite: button::Button,
    mute: button::Button,
    copy: button::Button,
    leave: button::Button,
    confirm: button::Button,
    cancel: button::Button,
}

pub struct ChannelPane {
    channel: Option<ChannelId>,
    /// The team shown in the Chats tab (for DMs' categories and links).
    team: Option<TeamId>,
    section: Section,
    confirm_leave: bool,
    copied: bool,
    /// Members and pinned posts asked for this session, by chat.
    members_loaded: HashSet<ChannelId>,
    pinned_loaded: HashSet<ChannelId>,
    buttons: Buttons,
    section_mouse_states: Vec<MouseStateHandle>,
    pinned_mouse_states: Vec<MouseStateHandle>,
    scroll_state: ClippedScrollStateHandle,
}

impl ChannelPane {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |_, _, _, ctx| ctx.notify());
        ctx.subscribe_to_model(&Session::handle(ctx), |me, _, event, ctx| match event {
            SessionEvent::Changed(deltas) => {
                if deltas.iter().any(|delta| me.affected_by(delta)) {
                    ctx.notify();
                }
            }
            SessionEvent::Reloaded => ctx.notify(),
            _ => {}
        });
        Self {
            channel: None,
            team: None,
            section: Section::Info,
            confirm_leave: false,
            copied: false,
            members_loaded: HashSet::new(),
            pinned_loaded: HashSet::new(),
            buttons: Buttons::default(),
            section_mouse_states: (0..3).map(|_| MouseStateHandle::default()).collect(),
            pinned_mouse_states: (0..MAX_PINNED)
                .map(|_| MouseStateHandle::default())
                .collect(),
            scroll_state: ClippedScrollStateHandle::default(),
        }
    }

    pub fn set_channel(&mut self, channel: Option<ChannelId>, ctx: &mut ViewContext<Self>) {
        if channel != self.channel {
            self.channel = channel;
            self.confirm_leave = false;
            self.copied = false;
            self.scroll_state.scroll_to(warpui::units::Pixels::zero());
            self.load_section(ctx);
            ctx.notify();
        }
    }

    pub fn set_team(&mut self, team: Option<TeamId>, ctx: &mut ViewContext<Self>) {
        self.team = team;
        ctx.notify();
    }

    fn affected_by(&self, delta: &StoreDelta) -> bool {
        let Some(channel) = &self.channel else {
            return false;
        };
        match delta {
            StoreDelta::ChannelChanged(changed) | StoreDelta::MemberChanged(changed) => {
                changed == channel
            }
            StoreDelta::CategoriesChanged(_)
            | StoreDelta::StatusChanged(_)
            | StoreDelta::UserChanged(_)
            | StoreDelta::NamesChanged
            | StoreDelta::ConfigChanged => true,
            StoreDelta::PostsChanged(_) | StoreDelta::PostsRemoved(_) => {
                self.section == Section::Pinned
            }
            _ => false,
        }
    }

    /// Members and pinned posts are loaded the first time their section shows a chat.
    fn load_section(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let command = match self.section {
            Section::Members if self.members_loaded.insert(channel.clone()) => {
                Command::LoadMembers(channel)
            }
            Section::Pinned if self.pinned_loaded.insert(channel.clone()) => {
                Command::LoadPinned(channel)
            }
            _ => return,
        };
        Session::as_ref(ctx).send(command);
    }

    fn render_sections(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let text = theme.main_text_color(theme.background()).into_solid();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let accent = theme.accent().into_solid();
        let mut row = Flex::row().with_spacing(4.);
        for (index, (section, label)) in [
            (Section::Info, "Info"),
            (Section::Members, "Members"),
            (Section::Pinned, "Pinned"),
        ]
        .into_iter()
        .enumerate()
        {
            let active = section == self.section;
            let button = Hoverable::new(self.section_mouse_states[index].clone(), move |state| {
                let mut container = Container::new(
                    Text::new_inline(label, font, size)
                        .with_color(if active { text } else { fade(text, 0.7) })
                        .with_style(Properties::default().weight(if active {
                            Weight::Bold
                        } else {
                            Weight::Normal
                        }))
                        .finish(),
                )
                .with_horizontal_padding(8.)
                .with_vertical_padding(6.);
                if active {
                    container = container.with_border(Border::bottom(2.).with_border_color(accent));
                } else if state.is_hovered() {
                    container = container.with_background_color(fade(text, 0.06));
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(ChannelPaneAction::Show(section)))
            .finish();
            row.add_child(button);
        }
        Container::new(row.finish())
            .with_horizontal_padding(PADDING - 8.)
            .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
            .finish()
    }

    fn button(
        &self,
        button: &button::Button,
        label: String,
        primary: bool,
        action: ChannelPaneAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme: &dyn button::Theme = if primary {
            &button::themes::Primary
        } else {
            &button::themes::Secondary
        };
        button.render(
            appearance,
            button::Params {
                content: button::Content::Label(label.into()),
                theme,
                options: button::Options {
                    on_click: Some(Box::new(move |ctx, _, _| {
                        ctx.dispatch_typed_action(action.clone())
                    })),
                    ..button::Options::default(appearance)
                },
            },
        )
    }

    fn render_info(&self, channel: &ChannelId, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let palette = Palette::of(appearance);
        let session = Session::as_ref(app);
        let Some(store) = session.store().and_then(|store| store.read().ok()) else {
            return Empty::new().finish();
        };
        let Some(info) = store.channels.get(channel) else {
            return Empty::new().finish();
        };
        let name = store
            .channel_name(channel)
            .unwrap_or_else(|| info.display_name.clone());
        let lookup = ChatLookup::new(&store, channel, self.team.as_ref());
        let renderer = MessageRenderer {
            palette: &palette,
            custom_emoji: &Default::default(),
            on_click: click_handler(),
            muted: false,
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(12.);
        let chat = appearance.theme().chat();
        column.add_child(
            Flex::row()
                .with_spacing(8.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(channel_icon(
                    crate::chats::channel_icon(&store, channel),
                    palette.text,
                    &chat,
                    palette.font,
                ))
                .with_child(
                    Shrinkable::new(
                        1.,
                        Text::new(name.clone(), palette.font, palette.size + 3.)
                            .with_color(palette.text)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .finish(),
                )
                .finish(),
        );
        let mut field = |label: &str, value: &str| {
            if value.trim().is_empty() {
                return;
            }
            let doc = mm_render::render(value, &lookup.context());
            column.add_child(
                Flex::column()
                    .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                    .with_spacing(4.)
                    .with_child(
                        Text::new_inline(label.to_owned(), palette.font, palette.size - 2.)
                            .with_color(palette.muted)
                            .with_style(Properties::default().weight(Weight::Bold))
                            .finish(),
                    )
                    .with_child(renderer.render(&doc))
                    .finish(),
            );
        };
        field("PURPOSE", &info.purpose);
        field("HEADER", &info.header);
        if let Some(stats) = store.channel_stats.get(channel) {
            let mut counts = format!(
                "{} member{}",
                stats.member_count,
                if stats.member_count == 1 { "" } else { "s" }
            );
            if stats.pinnedpost_count > 0 {
                counts.push_str(&format!(" · {} pinned", stats.pinnedpost_count));
            }
            column.add_child(
                Text::new_inline(counts, palette.font, palette.size - 1.)
                    .with_color(palette.muted)
                    .finish(),
            );
        }

        let mut actions = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_spacing(8.);
        if let Some(favorite) = channel_info::favorite(&store, channel, self.team.as_ref()) {
            let label = if favorite.is_favorite {
                "Remove from Favorites"
            } else {
                "Add to Favorites"
            };
            actions.add_child(self.button(
                &self.buttons.favorite,
                label.to_owned(),
                false,
                ChannelPaneAction::ToggleFavorite,
                appearance,
            ));
        }
        let muted = channel_info::is_muted(&store, channel);
        actions.add_child(self.button(
            &self.buttons.mute,
            if muted { "Unmute" } else { "Mute" }.to_owned(),
            false,
            ChannelPaneAction::ToggleMute,
            appearance,
        ));
        actions.add_child(
            self.button(
                &self.buttons.copy,
                if self.copied {
                    "Link copied"
                } else {
                    "Copy link"
                }
                .to_owned(),
                false,
                ChannelPaneAction::CopyLink,
                appearance,
            ),
        );
        if channel_info::can_leave(&store, channel) {
            if self.confirm_leave {
                actions.add_child(
                    Text::new(format!("Leave {name}?"), palette.font, palette.size)
                        .with_color(palette.text)
                        .finish(),
                );
                actions.add_child(
                    Flex::row()
                        .with_spacing(8.)
                        .with_child(self.button(
                            &self.buttons.cancel,
                            "Cancel".to_owned(),
                            false,
                            ChannelPaneAction::CancelLeave,
                            appearance,
                        ))
                        .with_child(self.button(
                            &self.buttons.confirm,
                            "Leave".to_owned(),
                            true,
                            ChannelPaneAction::ConfirmLeave,
                            appearance,
                        ))
                        .finish(),
                );
            } else {
                actions.add_child(self.button(
                    &self.buttons.leave,
                    "Leave channel".to_owned(),
                    false,
                    ChannelPaneAction::Leave,
                    appearance,
                ));
            }
        }
        column.add_child(actions.finish());
        column.finish()
    }

    fn render_members(&self, channel: &ChannelId, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let palette = Palette::of(appearance);
        let chat = appearance.theme().chat();
        let Some(store) = Session::as_ref(app)
            .store()
            .and_then(|store| store.read().ok())
        else {
            return Empty::new().finish();
        };
        let Some(rows) = channel_info::members(&store, channel) else {
            return muted_line("Loading members…", &palette);
        };
        let total = store
            .channel_stats
            .get(channel)
            .map_or(rows.len() as i64, |stats| stats.member_count);
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(6.);
        column.add_child(
            Text::new_inline(
                format!("{total} member{}", if total == 1 { "" } else { "s" }),
                palette.font,
                palette.size - 1.,
            )
            .with_color(palette.muted)
            .finish(),
        );
        for row in &rows {
            column.add_child(member_row(row, &palette, &chat));
        }
        if (rows.len() as i64) < total {
            column.add_child(muted_line(
                &format!("and {} more", total - rows.len() as i64),
                &palette,
            ));
        }
        column.finish()
    }

    fn render_pinned(&self, channel: &ChannelId, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let palette = Palette::of(appearance);
        let session = Session::as_ref(app);
        let Some(store) = session.store().and_then(|store| store.read().ok()) else {
            return Empty::new().finish();
        };
        let posts = store.pinned_posts(channel);
        if posts.is_empty() {
            return muted_line(
                if store.pinned.contains_key(channel) {
                    "No pinned messages yet. Pin important messages from the chat."
                } else {
                    "Loading pinned messages…"
                },
                &palette,
            );
        }
        let lookup = ChatLookup::new(&store, channel, self.team.as_ref());
        let clock = Clock::of(&store);
        let text = palette.text;
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.);
        for (index, post) in posts.iter().enumerate() {
            let Some(mouse_state) = self.pinned_mouse_states.get(index) else {
                break;
            };
            let author = crate::timeline::author(&store, post);
            let date = clock.zone.local(post.create_at);
            let when = format!("{} {}", date.format("%b %-d"), clock.time(post.create_at));
            let doc = Rc::new(mm_render::render_post(post, &lookup.context()));
            let palette = palette.clone();
            let id = post.id.clone();
            let card = Hoverable::new(mouse_state.clone(), move |state| {
                let renderer = MessageRenderer {
                    palette: &palette,
                    custom_emoji: &Default::default(),
                    on_click: click_handler(),
                    muted: false,
                };
                let mut container = Container::new(
                    Flex::column()
                        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
                        .with_spacing(4.)
                        .with_child(
                            Flex::row()
                                .with_spacing(8.)
                                .with_child(
                                    Text::new_inline(
                                        author.name.clone(),
                                        palette.font,
                                        palette.size - 1.,
                                    )
                                    .with_color(palette.text)
                                    .with_style(Properties::default().weight(Weight::Bold))
                                    .finish(),
                                )
                                .with_child(
                                    Text::new_inline(when.clone(), palette.font, palette.size - 2.)
                                        .with_color(palette.muted)
                                        .finish(),
                                )
                                .finish(),
                        )
                        .with_child(renderer.render(&doc))
                        .finish(),
                )
                .with_uniform_padding(10.)
                .with_border(Border::all(1.).with_border_color(palette.border))
                .with_corner_radius(warpui::elements::CornerRadius::with_all(
                    warpui::elements::Radius::Pixels(6.),
                ));
                if state.is_hovered() {
                    container = container.with_background_color(fade(text, 0.04));
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(ChannelPaneAction::ShowPost(id.clone()))
            })
            .finish();
            column.add_child(card);
        }
        column.finish()
    }
}

fn click_handler() -> crate::message::OnClick {
    Rc::new(|target, ctx| ctx.dispatch_typed_action(ChannelPaneAction::Click(target)))
}

fn muted_line(text: &str, palette: &Palette) -> Box<dyn Element> {
    Text::new(text.to_owned(), palette.font, palette.size - 1.)
        .with_color(palette.muted)
        .finish()
}

fn member_row(
    row: &MemberRow,
    palette: &Palette,
    chat: &ml_themes::ChatColors,
) -> Box<dyn Element> {
    let mut name = row.name.clone();
    if row.me {
        name.push_str(" (you)");
    }
    let mut line = Flex::row()
        .with_spacing(8.)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(channel_icon(
            ChannelIcon::Direct(row.presence),
            palette.muted,
            chat,
            palette.font,
        ))
        .with_child(
            Text::new_inline(name, palette.font, palette.size - 1.)
                .with_color(if row.presence == Presence::Offline {
                    fade(palette.text, 0.75)
                } else {
                    palette.text
                })
                .finish(),
        );
    if row.username != row.name {
        line.add_child(
            Shrinkable::new(
                1.,
                Text::new_inline(
                    format!("@{}", row.username),
                    palette.font,
                    palette.size - 2.,
                )
                .with_color(palette.muted)
                .finish(),
            )
            .finish(),
        );
    }
    line.finish()
}

impl Entity for ChannelPane {
    type Event = ChannelPaneEvent;
}

impl View for ChannelPane {
    fn ui_name() -> &'static str {
        "ChannelPane"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let body = match &self.channel {
            None => muted_line("Open a chat to see its details.", &Palette::of(appearance)),
            Some(channel) => match self.section {
                Section::Info => self.render_info(channel, app),
                Section::Members => self.render_members(channel, app),
                Section::Pinned => self.render_pinned(channel, app),
            },
        };
        let text = theme.main_text_color(theme.background()).into_solid();
        let scrollable = ClippedScrollable::vertical(
            self.scroll_state.clone(),
            Container::new(body).with_uniform_padding(PADDING).finish(),
            warpui::elements::ScrollbarWidth::Auto,
            fade(text, 0.25).into(),
            fade(text, 0.45).into(),
            warpui::elements::Fill::None,
        )
        .finish();
        Flex::column()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(self.render_sections(appearance))
            .with_child(Shrinkable::new(1., scrollable).finish())
            .finish()
    }
}

impl TypedActionView for ChannelPane {
    type Action = ChannelPaneAction;

    fn handle_action(&mut self, action: &ChannelPaneAction, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let session = Session::as_ref(ctx);
        match action {
            ChannelPaneAction::Show(section) => {
                self.section = *section;
                self.scroll_state.scroll_to(warpui::units::Pixels::zero());
                self.load_section(ctx);
            }
            ChannelPaneAction::ToggleFavorite => {
                let favorite = session.store().and_then(|store| {
                    channel_info::favorite(&*store.read().ok()?, &channel, self.team.as_ref())
                });
                if let Some(favorite) = favorite {
                    session.send(Command::MoveToCategory {
                        team: favorite.team,
                        channel,
                        category: favorite.target,
                    });
                }
            }
            ChannelPaneAction::ToggleMute => {
                let muted = session
                    .store()
                    .and_then(|store| {
                        store
                            .read()
                            .ok()
                            .map(|store| channel_info::is_muted(&store, &channel))
                    })
                    .unwrap_or(false);
                session.send(Command::SetMuted {
                    channel,
                    muted: !muted,
                });
            }
            ChannelPaneAction::CopyLink => {
                let server = session
                    .active()
                    .map(|active| active.server.as_str().to_owned())
                    .unwrap_or_default();
                let link = session.store().and_then(|store| {
                    channel_info::link(&*store.read().ok()?, &channel, self.team.as_ref(), &server)
                });
                if let Some(link) = link {
                    ctx.clipboard().write(ClipboardContent::plain_text(link));
                    self.copied = true;
                    let _ = ctx.spawn(
                        async { Timer::after(Duration::from_secs(2)).await },
                        |me, _, ctx| {
                            me.copied = false;
                            ctx.notify();
                        },
                    );
                }
            }
            ChannelPaneAction::Leave => self.confirm_leave = true,
            ChannelPaneAction::CancelLeave => self.confirm_leave = false,
            ChannelPaneAction::ConfirmLeave => {
                self.confirm_leave = false;
                session.send(Command::LeaveChannel(channel));
            }
            ChannelPaneAction::ShowPost(post) => ctx.emit(ChannelPaneEvent::ShowPost(post.clone())),
            ChannelPaneAction::Click(Target::Url(url)) => ctx.open_url(url),
            ChannelPaneAction::Click(_) => {}
        }
        ctx.notify();
    }
}

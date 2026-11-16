// SPDX-License-Identifier: AGPL-3.0-only

//! The message composer under a chat's timeline (MatterLite.md §13): `ml_editor::EditorView`,
//! multi-line, growing with its text up to a limit. Enter sends and Shift+Enter adds a line
//! (or the other way round with `send_on_ctrl_enter`); inside an open code block Enter adds a
//! line. The text is kept as a draft per chat (saved to the cache half a second after the last
//! edit and when the chat changes) and restored when the chat comes back; typing is announced
//! at most once per `TimeBetweenUserTypingUpdatesMilliseconds`. Archived chats show a notice
//! instead, read-only ones a disabled field.

use std::time::{Duration, Instant};

use ml_editor::{
    EditOrigin, EditorOptions, EditorView, EnterAction, EnterSettings, Event as EditorEvent,
    InteractionState, TextOptions,
};
use mm_model::{ChannelId, PostId};
use mm_sync::Command;
use warp_core::ui::appearance::Appearance;
use warpui::r#async::Timer;
use warpui::elements::{
    Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Flex,
    ParentElement, Radius, Text,
};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity as _, View, ViewContext, ViewHandle,
};

use crate::chats_view::fade;
use crate::composer::{
    self, EnterKey, EnterPrefs, OnEnter, TypingThrottle, length_error, max_post_size,
};
use crate::session::Session;

/// The composer grows with its text up to this height, then scrolls.
const MAX_EDITOR_HEIGHT: f32 = 240.;
/// Drafts are saved this long after the last edit.
const DRAFT_DELAY: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerEvent {
    /// A post was handed to the engine.
    Sent,
    /// Esc in the composer.
    Escape,
}

/// Why the chat takes no posts.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Blocked {
    Archived,
    ReadOnly,
}

pub struct ComposerView {
    editor: ViewHandle<EditorView>,
    channel: Option<ChannelId>,
    blocked: Option<Blocked>,
    /// The text is too long to send.
    error: Option<String>,
    throttle: TypingThrottle,
    /// Bumped by every edit; a draft save only runs if no edit came after it.
    edits: u64,
    /// An edit since the draft was last saved.
    unsaved: bool,
}

impl ComposerView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let appearance = Appearance::as_ref(ctx);
        let options = EditorOptions {
            text: TextOptions::ui_text(Some(appearance.ui_font_size() + 1.), appearance),
            autogrow: true,
            soft_wrap: true,
            placeholder_soft_wrap: true,
            enter_settings: enter_settings(),
            ..EditorOptions::default()
        };
        let editor = ctx.add_typed_action_view(|ctx| EditorView::new(options, ctx));
        ctx.subscribe_to_view(&editor, |me, _, event, ctx| me.on_editor_event(event, ctx));
        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            let appearance = Appearance::as_ref(ctx);
            let (font, size) = (appearance.ui_font_family(), appearance.ui_font_size() + 1.);
            me.editor.update(ctx, |editor, ctx| {
                editor.set_font_family(font, ctx);
                editor.set_font_size(size, ctx);
            });
            ctx.notify();
        });
        Self {
            editor,
            channel: None,
            blocked: None,
            error: None,
            throttle: TypingThrottle::default(),
            edits: 0,
            unsaved: false,
        }
    }

    /// Shows the composer of a chat: its draft, its placeholder, and whether it takes posts.
    pub fn set_channel(&mut self, channel: Option<ChannelId>, ctx: &mut ViewContext<Self>) {
        if channel == self.channel {
            self.update_state(ctx);
            return;
        }
        self.save_draft(ctx);
        self.channel = channel;
        self.throttle.reset();
        self.error = None;
        let draft = self
            .channel
            .as_ref()
            .and_then(|channel| {
                Session::as_ref(ctx)
                    .active()?
                    .draft(channel, &PostId::default())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        self.editor.update(ctx, |editor, ctx| {
            editor.system_reset_buffer_text(&draft, ctx);
            editor.move_to_buffer_end(ctx);
        });
        self.update_state(ctx);
    }

    /// Placeholder, Enter keys and whether the chat takes posts, from the store (also after
    /// it changed: archived, permissions, preferences).
    pub fn update_state(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let Ok(store) = store.read() else {
            return;
        };
        let name = store
            .channel_name(&channel)
            .unwrap_or_else(|| channel.to_string());
        let archived = store
            .channels
            .get(&channel)
            .is_some_and(|channel| channel.delete_at != 0);
        let blocked = if archived {
            Some(Blocked::Archived)
        } else if !store.has_channel_permission(&channel, "create_post") {
            Some(Blocked::ReadOnly)
        } else {
            None
        };
        drop(store);
        let placeholder = match blocked {
            Some(Blocked::ReadOnly) => {
                "This channel is read-only. Only members with permission can post here.".to_owned()
            }
            _ => format!("Write to {name}"),
        };
        let state = if blocked.is_some() {
            InteractionState::Disabled
        } else {
            InteractionState::Editable
        };
        self.editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text(placeholder, ctx);
            if editor.interaction_state(ctx) != state {
                editor.set_interaction_state(state, ctx);
            }
        });
        if blocked != self.blocked {
            self.blocked = blocked;
            ctx.notify();
        }
    }

    fn on_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Edited(origin) => self.on_edited(*origin, ctx),
            EditorEvent::Enter => self.on_enter(EnterKey::Plain, ctx),
            EditorEvent::CtrlEnter | EditorEvent::CmdEnter => {
                self.on_enter(EnterKey::CtrlOrCmd, ctx)
            }
            EditorEvent::Escape => ctx.emit(ComposerEvent::Escape),
            _ => {}
        }
    }

    fn on_edited(&mut self, origin: EditOrigin, ctx: &mut ViewContext<Self>) {
        if !origin.is_user() {
            return;
        }
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let text = self.editor.as_ref(ctx).buffer_text(ctx);
        let (max, typing, interval) = match Session::as_ref(ctx)
            .store()
            .and_then(|store| store.read().ok())
        {
            Some(store) => (
                max_post_size(&store),
                composer::typing_enabled(&store, &channel),
                composer::typing_interval(&store),
            ),
            None => return,
        };
        let error = length_error(&text, max);
        if error != self.error {
            self.error = error;
            ctx.notify();
        }
        if typing && !text.is_empty() && self.throttle.due(Instant::now(), interval) {
            Session::as_ref(ctx).send(Command::Typing {
                channel,
                root: PostId::default(),
            });
        }
        // The draft is saved once the edits pause.
        self.unsaved = true;
        self.edits += 1;
        let edits = self.edits;
        let _ = ctx.spawn(
            async { Timer::after(DRAFT_DELAY).await },
            move |me, _, ctx| {
                if me.edits == edits {
                    me.save_draft(ctx);
                }
            },
        );
    }

    fn save_draft(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.unsaved {
            return;
        }
        self.unsaved = false;
        let Some(channel) = self.channel.clone() else {
            return;
        };
        let text = self.editor.as_ref(ctx).buffer_text(ctx);
        Session::handle(ctx).update(ctx, |session, _| {
            session.set_draft(&channel, &PostId::default(), text)
        });
    }

    fn on_enter(&mut self, key: EnterKey, ctx: &mut ViewContext<Self>) {
        let Some(channel) = self.channel.clone() else {
            return;
        };
        if self.blocked.is_some() {
            return;
        }
        let editor = self.editor.as_ref(ctx);
        let text = editor.buffer_text(ctx);
        let caret = editor.start_byte_index_of_first_selection(ctx).as_usize();
        let Some(store) = Session::as_ref(ctx).store().cloned() else {
            return;
        };
        let (prefs, max) = match store.read() {
            Ok(store) => (EnterPrefs::of(&store), max_post_size(&store)),
            Err(_) => return,
        };
        let message = match composer::on_enter(&text, caret, key, prefs) {
            OnEnter::NewLine => {
                self.editor
                    .update(ctx, |editor, ctx| editor.user_insert("\n", ctx));
                return;
            }
            OnEnter::Send(message) => message,
        };
        let message = message.trim_end().to_owned();
        if message.trim().is_empty() {
            return;
        }
        if let Some(error) = length_error(&message, max) {
            self.error = Some(error);
            ctx.notify();
            return;
        }
        Session::as_ref(ctx).send(Command::SendPost {
            channel: channel.clone(),
            root: PostId::default(),
            message,
        });
        self.editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx)
        });
        self.unsaved = false;
        self.edits += 1;
        Session::handle(ctx).update(ctx, |session, _| {
            session.set_draft(&channel, &PostId::default(), String::new())
        });
        self.throttle.reset();
        self.error = None;
        ctx.emit(ComposerEvent::Sent);
        ctx.notify();
    }
}

/// Enter and Ctrl+Enter emit and the composer decides (the preferences and open code blocks
/// change what they do); Shift+Enter and Alt+Enter add a line.
fn enter_settings() -> EnterSettings {
    EnterSettings {
        enter: EnterAction::Emit,
        ctrl_enter: EnterAction::Emit,
        shift_enter: EnterAction::InsertNewLineIfMultiLine,
        alt_enter: EnterAction::InsertNewLineIfMultiLine,
    }
}

impl Entity for ComposerView {
    type Event = ComposerEvent;
}

impl View for ComposerView {
    fn ui_name() -> &'static str {
        "ComposerView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let chat = theme.chat();
        let font = appearance.ui_font_family();
        let size = appearance.ui_font_size();
        let text = theme.main_text_color(theme.background()).into_solid();
        let muted = theme.sub_text_color(theme.background()).into_solid();
        let border = if self.error.is_some() {
            chat.error_text
        } else {
            fade(text, 0.2)
        };
        let field = match self.blocked {
            Some(Blocked::Archived) => Container::new(
                Text::new(
                    "You are viewing an archived channel. New messages cannot be posted.",
                    font,
                    size,
                )
                .with_color(muted)
                .finish(),
            )
            .with_uniform_padding(12.)
            .with_background_color(fade(text, 0.05))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish(),
            _ => Container::new(
                ConstrainedBox::new(ChildView::new(&self.editor).finish())
                    .with_max_height(MAX_EDITOR_HEIGHT)
                    .finish(),
            )
            .with_horizontal_padding(12.)
            .with_vertical_padding(10.)
            .with_border(Border::all(1.).with_border_color(border))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish(),
        };
        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(4.)
            .with_child(field);
        if let Some(error) = &self.error {
            column.add_child(
                Text::new(error.clone(), font, size - 1.)
                    .with_color(chat.error_text)
                    .finish(),
            );
        }
        column.finish()
    }
}

impl warpui::TypedActionView for ComposerView {
    type Action = ();
}

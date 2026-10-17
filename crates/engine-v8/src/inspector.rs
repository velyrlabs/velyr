//! `v8::inspector` wired to a transport-agnostic channel of CDP text messages (VELYR-120).
//!
//! Messages are dispatched on the isolate thread: blocking while paused at a breakpoint or while
//! waiting for the debugger, and between event-loop ticks otherwise (`run_platform_tasks`).

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use v8::inspector::{
    Channel, ChannelImpl, StringBuffer, StringView, V8Inspector, V8InspectorClient,
    V8InspectorClientImpl, V8InspectorClientTrustLevel, V8InspectorSession,
};

const CONTEXT_GROUP_ID: i32 = 1;
const CONTEXT_NAME: &[u8] = b"velyr";
const CONTEXT_AUX_DATA: &[u8] = br#"{"isDefault":true}"#;
const BREAK_REASON: &[u8] = b"Break on start";

/// What the transport (the CDP server) reports to the isolate.
#[derive(Debug)]
pub enum InspectorEvent {
    /// A client attached; responses and notifications for it go to this sender.
    Connected(Sender<String>),
    /// One CDP message from the client.
    Message(String),
    Disconnected,
}

#[derive(Debug)]
pub struct InspectorOptions {
    pub events: Receiver<InspectorEvent>,
    /// Run no JS until the client sends `Runtime.runIfWaitingForDebugger`.
    pub wait_for_session: bool,
    /// Pause on the first statement of the first module evaluated.
    pub break_on_start: bool,
}

/// Handed by the engine factory to the first isolate it creates.
pub(crate) type PendingInspector = Arc<Mutex<Option<InspectorOptions>>>;

pub(crate) struct Inspector {
    state: Rc<State>,
}

struct State {
    events: Receiver<InspectorEvent>,
    // Declared before `v8`: a session must be dropped before its inspector.
    session: RefCell<Option<V8InspectorSession>>,
    v8: OnceCell<V8Inspector>,
    paused: Cell<bool>,
    waiting: Cell<bool>,
    break_on_start: Cell<bool>,
}

impl Inspector {
    /// Registers `context` with a new inspector; with `wait_for_session`, blocks until the client
    /// lets the program run.
    pub(crate) fn attach(
        isolate: &mut v8::OwnedIsolate,
        context: &v8::Global<v8::Context>,
        options: InspectorOptions,
    ) -> Self {
        let state = Rc::new(State {
            events: options.events,
            session: RefCell::new(None),
            v8: OnceCell::new(),
            paused: Cell::new(false),
            waiting: Cell::new(options.wait_for_session),
            break_on_start: Cell::new(options.break_on_start),
        });
        let client = V8InspectorClient::new(Box::new(Client(Rc::downgrade(&state))));
        let inspector = V8Inspector::create(isolate, client);
        {
            v8::scope!(let scope, isolate);
            let context = v8::Local::new(scope, context);
            inspector.context_created(
                context,
                CONTEXT_GROUP_ID,
                StringView::from(CONTEXT_NAME),
                StringView::from(CONTEXT_AUX_DATA),
            );
        }
        let _ = state.v8.set(inspector);
        state.run_while(|state| state.waiting.get());
        Self { state }
    }

    /// Dispatches the messages that arrived since the last call, without blocking.
    pub(crate) fn pump(&self) {
        while let Ok(event) = self.state.events.try_recv() {
            self.state.handle(event);
        }
    }

    /// With `break_on_start`, the first call schedules a pause on the next statement.
    pub(crate) fn before_module_evaluation(&self) {
        if !self.state.break_on_start.take() {
            return;
        }
        if let Some(session) = self.state.session.borrow().as_ref() {
            let reason = StringView::from(BREAK_REASON);
            session.schedule_pause_on_next_statement(reason, reason);
        }
    }
}

impl State {
    /// Handles events, blocking, while `condition` holds or until the transport is gone.
    fn run_while(&self, condition: impl Fn(&Self) -> bool) {
        while condition(self) {
            match self.events.recv() {
                Ok(event) => self.handle(event),
                Err(_) => return,
            }
        }
    }

    fn handle(&self, event: InspectorEvent) {
        match event {
            InspectorEvent::Connected(outgoing) => self.connect(outgoing),
            InspectorEvent::Message(text) => self.dispatch(&text),
            InspectorEvent::Disconnected => self.disconnect(),
        }
    }

    fn connect(&self, outgoing: Sender<String>) {
        let Some(inspector) = self.v8.get() else {
            return;
        };
        // shortcut: a client attaching while a nested dispatch holds the session is ignored;
        // the transport allows one client at a time, so this only hits a reconnect mid-pause.
        let Ok(mut session) = self.session.try_borrow_mut() else {
            return;
        };
        *session = Some(inspector.connect(
            CONTEXT_GROUP_ID,
            Channel::new(Box::new(Outbox(outgoing))),
            StringView::empty(),
            V8InspectorClientTrustLevel::FullyTrusted,
        ));
    }

    fn dispatch(&self, text: &str) {
        // CDP is UTF-8 JSON; an 8-bit StringView would be read as Latin-1.
        let utf16: Vec<u16> = text.encode_utf16().collect();
        if let Some(session) = self.session.borrow().as_ref() {
            session.dispatch_protocol_message(StringView::from(&utf16[..]));
        }
    }

    /// The program never stays paused or waiting for a client that left.
    fn disconnect(&self) {
        if let Ok(mut session) = self.session.try_borrow_mut() {
            session.take();
        }
        self.paused.set(false);
        self.waiting.set(false);
    }
}

/// Holds the state weakly: the state owns the inspector, which owns this client.
struct Client(Weak<State>);

impl V8InspectorClientImpl for Client {
    fn run_message_loop_on_pause(&self, _context_group_id: i32) {
        let Some(state) = self.0.upgrade() else {
            return;
        };
        state.paused.set(true);
        state.run_while(|state| state.paused.get());
    }

    fn quit_message_loop_on_pause(&self) {
        if let Some(state) = self.0.upgrade() {
            state.paused.set(false);
        }
    }

    fn run_if_waiting_for_debugger(&self, _context_group_id: i32) {
        if let Some(state) = self.0.upgrade() {
            state.waiting.set(false);
        }
    }
}

struct Outbox(Sender<String>);

impl Outbox {
    fn send(&self, message: v8::UniquePtr<StringBuffer>) {
        if let Some(message) = message.as_ref() {
            // A closed transport means the client left; the message has nowhere to go.
            let _ = self.0.send(message.string().to_string());
        }
    }
}

impl ChannelImpl for Outbox {
    fn send_response(&self, _call_id: i32, message: v8::UniquePtr<StringBuffer>) {
        self.send(message);
    }

    fn send_notification(&self, message: v8::UniquePtr<StringBuffer>) {
        self.send(message);
    }

    fn flush_protocol_notifications(&self) {}
}

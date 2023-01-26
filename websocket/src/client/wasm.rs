use super::{
    error::Error,
    message::{Ack, Message},
    result::Result,
    Handshake, Options,
};
use futures::{select, select_biased, FutureExt};
use js_sys::{ArrayBuffer, Uint8Array};
use std::ops::Deref;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use triggered::{trigger, Listener, Trigger};
use wasm_bindgen::JsCast;
use web_sys::{
    CloseEvent as WsCloseEvent, ErrorEvent as WsErrorEvent, MessageEvent as WsMessageEvent,
    WebSocket as WebSysWebSocket,
};
use workflow_core::{
    channel::{oneshot, unbounded, Channel, DuplexChannel},
    task::spawn,
};
use workflow_log::*;
use workflow_wasm::callback::*;

impl TryFrom<WsMessageEvent> for Message {
    type Error = Error;

    fn try_from(event: WsMessageEvent) -> std::result::Result<Self, Self::Error> {
        match event.data() {
            data if data.is_instance_of::<ArrayBuffer>() => {
                let buffer = Uint8Array::new(data.unchecked_ref());
                Ok(Message::Binary(buffer.to_vec()))
            }
            data if data.is_string() => match data.as_string() {
                Some(text) => Ok(Message::Text(text)),
                None => Err(Error::DataEncoding),
            },
            _ => Err(Error::DataType),
        }
    }
}

#[derive(Clone)]
pub struct WebSocket(WebSysWebSocket);
unsafe impl Send for WebSocket {}
unsafe impl Sync for WebSocket {}
impl Deref for WebSocket {
    type Target = WebSysWebSocket;
    fn deref(&self) -> &WebSysWebSocket {
        &self.0
    }
}

impl WebSocket {
    #[allow(dead_code)]
    const CONNECTING: u16 = WebSysWebSocket::CONNECTING;
    #[allow(dead_code)]
    const OPEN: u16 = WebSysWebSocket::OPEN;
    #[allow(dead_code)]
    const CLOSING: u16 = WebSysWebSocket::CLOSING;
    #[allow(dead_code)]
    const CLOSED: u16 = WebSysWebSocket::CLOSED;

    pub fn new(url: &str) -> Result<Self> {
        let ws = WebSysWebSocket::new(url)?;
        Ok(WebSocket(ws))
    }
}

impl From<WebSysWebSocket> for WebSocket {
    fn from(ws: WebSysWebSocket) -> Self {
        WebSocket(ws)
    }
}

struct Settings {
    url: String,
}

#[allow(dead_code)]
struct Inner {
    ws: WebSocket,
    callbacks: CallbackMap,
}

unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

pub struct WebSocketInterface {
    inner: Arc<Mutex<Option<Inner>>>,
    settings: Arc<Mutex<Settings>>,
    reconnect: AtomicBool,
    is_open: AtomicBool,
    event_channel: Channel<Message>,
    sender_channel: Channel<(Message, Ack)>,
    receiver_channel: Channel<Message>,
    handshake: Option<Arc<dyn Handshake>>,
    dispatcher_shutdown: DuplexChannel,
}

impl WebSocketInterface {
    pub fn new(
        url: &str,
        sender_channel: Channel<(Message, Ack)>,
        receiver_channel: Channel<Message>,
        options: Options,
    ) -> Result<WebSocketInterface> {
        let settings = Settings {
            url: url.to_string(),
        };

        let iface = WebSocketInterface {
            inner: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(settings)),
            sender_channel,
            receiver_channel,
            event_channel: Channel::unbounded(),
            reconnect: AtomicBool::new(true),
            is_open: AtomicBool::new(false),
            handshake: options.handshake,
            dispatcher_shutdown: DuplexChannel::unbounded(),
        };

        Ok(iface)
    }

    pub fn url(self: &Arc<Self>) -> String {
        self.settings.lock().unwrap().url.clone()
    }

    pub fn set_url(self: &Arc<Self>, url: &str) {
        self.settings.lock().unwrap().url = url.into();
    }

    pub fn is_open(self: &Arc<Self>) -> bool {
        self.is_open.load(Ordering::SeqCst)
    }

    pub async fn connect(self: &Arc<Self>, block: bool) -> Result<Option<Listener>> {
        let (connect_trigger, connect_listener) = trigger();

        self.connect_impl(connect_trigger)?;

        match block {
            true => {
                connect_listener.await;
                Ok(None)
            }
            false => Ok(Some(connect_listener)),
        }
    }

    fn connect_impl(self: &Arc<Self>, connect_trigger: Trigger) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.is_some() {
            return Err(Error::AlreadyInitialized);
        }

        let connect_trigger = Arc::new(Mutex::new(Some(connect_trigger)));

        self.reconnect.store(true, Ordering::SeqCst);
        let ws = WebSocket::new(&self.url())?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        // - Message
        let event_sender_ = self.event_channel.sender.clone();
        let onmessage = callback!(move |event: WsMessageEvent| {
            let msg: Message = event.try_into().expect("MessageEvent Error");
            event_sender_.try_send(msg).unwrap_or_else(|err| {
                log_trace!("WebSocket unable to try_send() `message` to event channel: `{err}`")
            });
        });
        ws.set_onmessage(Some(onmessage.as_ref()));

        // - Error
        let onerror = callback!(move |_event: WsErrorEvent| {
            // log_trace!("error event: {:?}", event);
        });
        ws.set_onerror(Some(onerror.as_ref()));

        // - Open
        let event_sender_ = self.event_channel.sender.clone();
        let onopen = callback!(move || {
            event_sender_.try_send(Message::Open).unwrap_or_else(|err| {
                log_trace!("WebSocket unable to try_send() `open` to event channel: `{err}`")
            });
        });
        ws.set_onopen(Some(onopen.as_ref()));

        // - Close
        let event_sender_ = self.event_channel.sender.clone();
        let onclose = callback!(move |_event: WsCloseEvent| {
            // let event: CloseEvent = _event.into();
            event_sender_
                .try_send(Message::Close)
                .unwrap_or_else(|err| {
                    log_trace!("WebSocket unable to try_send() `open` to event channel: `{err}`")
                });
        });
        ws.set_onclose(Some(onclose.as_ref()));

        let callbacks = CallbackMap::new();
        callbacks.retain(onmessage)?;
        callbacks.retain(onerror)?;
        callbacks.retain(onopen)?;
        callbacks.retain(onclose)?;

        *inner = Some(Inner {
            ws: ws.clone(),
            callbacks,
            // dispatcher_task,
        });

        let self_ = self.clone();
        spawn(async move {
            self_
                .dispatcher_task(&ws, connect_trigger)
                .await
                .unwrap_or_else(|err| log_trace!("WebSocket dispatcher error: {err}"));

            if self_.reconnect.load(Ordering::SeqCst) {
                async_std::task::sleep(std::time::Duration::from_millis(1000)).await;
                self_.reconnect().await.ok();
            }
        });

        Ok(())
    }

    #[allow(dead_code)]
    fn ws(self: &Arc<Self>) -> Result<WebSocket> {
        Ok(self.inner.lock().unwrap().as_ref().unwrap().ws.clone())
    }

    #[allow(dead_code)]
    pub fn try_send(self: &Arc<Self>, message: &Message) -> Result<()> {
        let ws = self.ws()?;
        ws.try_send(message)?;
        Ok(())
    }

    async fn handshake(self: &Arc<Self>, ws: &WebSocket) -> Result<()> {
        if let Some(handshake) = self.handshake.as_ref().cloned() {
            let (sender_tx, sender_rx) = unbounded();
            let (receiver_tx, receiver_rx) = unbounded();
            let (accept_tx, accept_rx) = oneshot();

            spawn(async move {
                accept_tx
                    .send(handshake.handshake(&sender_tx, &receiver_rx).await)
                    .await
                    .unwrap_or_else(|err| {
                        log_trace!("WebSocket handshake unable to send completion: `{}`", err)
                    });
            });

            // let handshake_rx = self.handshake_channel.receiver.clone();
            loop {
                select_biased! {
                    result = accept_rx.recv().fuse() => {
                        return result?;
                    },
                    msg = sender_rx.recv().fuse() => {
                        if let Ok(msg) = msg {
                            ws.try_send(&msg)?;
                            // ws_sender.send(msg.into()).await?;
                        }
                    },
                    msg = self.event_channel.recv().fuse() => {
                        if let Ok(msg) = msg {
                            receiver_tx.send(msg).await?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn dispatcher_task(
        self: &Arc<Self>,
        ws: &WebSocket,
        connect_trigger: Arc<Mutex<Option<Trigger>>>,
    ) -> Result<()> {
        loop {
            select! {
                _ = self.dispatcher_shutdown.request.receiver.recv().fuse() => {
                    break;
                },
                msg = self.event_channel.recv().fuse() => {

                    if let Ok(msg) = msg {
                        match msg {
                            Message::Binary(_) | Message::Text(_) => {
                                self.receiver_channel.sender.send(msg).await.unwrap();
                            },
                            Message::Open => {
                                self.handshake(ws).await?;
                                self.is_open.store(true, Ordering::SeqCst);

                                if connect_trigger.lock().unwrap().is_some() {
                                    connect_trigger.lock().unwrap().take().unwrap().trigger();
                                }

                                self.receiver_channel.sender.send(msg).await.unwrap();
                            },
                            Message::Close => {
                                self.is_open.store(false, Ordering::SeqCst);
                                self.cleanup_ws();
                                self.receiver_channel.sender.send(msg).await.unwrap();
                                break;
                            }
                        }
                    }

                },
                msg = self.sender_channel.receiver.recv().fuse() => {

                    if let Ok((msg, ack)) = msg {

                        // if ws.ready_state() != WebSocket::OPEN {
                        //     return Err(Error::NotConnected);
                        // }

                        if let Some(ack) = ack {
                            let result = ws
                                .try_send(&msg)
                                .map(Arc::new)
                                .map_err(Arc::new);
                            ack.send(result).await.unwrap_or_else(|err| {
                                log_trace!("WebSocket error producing message ack {:?}", err)
                            });
                        } else {
                            ws.try_send(&msg).unwrap_or_else(|err| {
                                log_trace!("WebSocket unable to send `raw ws` message: `{err}`")
                            });
                        }
                    }
                }
            }
        }

        Ok(())
        // log_trace!("signaling SHUTDOWN...");
    }

    fn cleanup_ws(self: &Arc<Self>) {
        let ws = self.ws().unwrap();
        ws.set_onopen(None);
        ws.set_onclose(None);
        ws.set_onerror(None);
        ws.set_onmessage(None);
    }

    async fn _shutdown(self: &Arc<Self>) -> Result<()> {
        self.dispatcher_shutdown
            .signal(())
            .await
            .map_err(|_| Error::DispatcherSignal)?;

        Ok(())
    }

    pub async fn close(self: &Arc<Self>) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(inner_) = &mut *inner {
            inner_.ws.close()?;
            *inner = None;
        } else {
            log_trace!("WebSocket error: disconnecting from non-initialized connection");
        }

        Ok(())
    }
    async fn reconnect(self: &Arc<Self>) -> Result<()> {
        // log_trace!("... starting reconnect");

        self.close().await?;
        self.connect(false).await?;

        Ok(())
    }
    pub async fn disconnect(self: &Arc<Self>) -> Result<()> {
        self.reconnect.store(false, Ordering::SeqCst);
        self.close().await.ok();
        Ok(())
    }
}

impl Drop for WebSocketInterface {
    fn drop(&mut self) {}
}

trait TrySendMessage {
    fn try_send(&self, message: &Message) -> Result<()>;
}

impl TrySendMessage for WebSocket {
    fn try_send(&self, message: &Message) -> Result<()> {
        match message {
            Message::Binary(data) => self.send_with_u8_array(data).map_err(|e| e.into()),
            Message::Text(text) => self.send_with_str(text).map_err(|e| e.into()),
            _ => {
                panic!(
                    "WebSocket trying to convert unsupported message type: `{:?}`",
                    message
                );
            }
        }
    }
}